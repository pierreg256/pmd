pub mod control;
pub mod membership;
pub mod peer;
pub mod server;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::Mutex;
use tokio::time::{self, Duration};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::tls;

use self::membership::Membership;
use self::peer::PeerHandle;

/// Shared daemon state, protected by a mutex.
pub struct DaemonState {
    pub membership: Membership,
    pub listen_addr: SocketAddr,
    pub peers: HashMap<String, PeerHandle>,
    pub pending_joins: Vec<String>,
    pub pending_leaves: Vec<String>,
    pub shutdown: CancellationToken,
}

/// Run the PMD daemon.
pub async fn run(config: Config, node_id: String) -> Result<()> {
    let config = Arc::new(config);
    config.ensure_dirs()?;

    // Load or generate cookie
    let cookie = load_or_generate_cookie(&config)?;
    let cookie = Arc::new(cookie);

    // Build TLS
    let acceptor = tls::build_acceptor(&config)?;
    let connector = tls::build_connector(&config)?;

    // Bind TCP listener
    let listen_addr: SocketAddr = format!("{}:{}", config.bind, config.port).parse()?;
    let listener = TcpListener::bind(listen_addr).await?;
    let actual_addr = listener.local_addr()?;
    info!(addr = %actual_addr, node_id = %node_id, "PMD daemon starting");

    // Initialize membership with self
    let mut membership = Membership::new(&node_id);
    membership.add_node(&node_id, actual_addr, HashMap::new());

    let shutdown = CancellationToken::new();

    let state = Arc::new(Mutex::new(DaemonState {
        membership,
        listen_addr: actual_addr,
        peers: HashMap::new(),
        pending_joins: Vec::new(),
        pending_leaves: Vec::new(),
        shutdown: shutdown.clone(),
    }));

    // Remove stale socket file
    let _ = std::fs::remove_file(&config.socket_path);

    // Bind control socket
    let control_listener = UnixListener::bind(&config.socket_path)
        .context("failed to bind control socket")?;

    // Write PID file
    std::fs::write(&config.pid_path, std::process::id().to_string())
        .context("failed to write PID file")?;

    // Spawn control socket task
    let control_state = Arc::clone(&state);
    tokio::spawn(async move {
        if let Err(e) = control::run_control_socket(control_listener, control_state).await {
            error!(error = %e, "control socket error");
        }
    });

    // Spawn TLS server task
    let server_state = Arc::clone(&state);
    let server_config = Arc::clone(&config);
    let server_cookie = Arc::clone(&cookie);
    tokio::spawn(async move {
        if let Err(e) =
            server::run_server(listener, acceptor, server_config, server_cookie, server_state).await
        {
            error!(error = %e, "server error");
        }
    });

    // Main loop: process pending joins/leaves
    let mut tick = time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("daemon shutting down");
                break;
            }
            _ = tick.tick() => {
                let mut st = state.lock().await;
                let joins: Vec<String> = st.pending_joins.drain(..).collect();
                let leaves: Vec<String> = st.pending_leaves.drain(..).collect();
                drop(st);

                for addr_str in joins {
                    match addr_str.parse::<SocketAddr>() {
                        Ok(addr) => {
                            let config = Arc::clone(&config);
                            let cookie = Arc::clone(&cookie);
                            let state = Arc::clone(&state);
                            let connector = connector.clone();
                            tokio::spawn(async move {
                                if let Err(e) = peer::connect_to_peer(
                                    addr, config, cookie, state, connector,
                                ).await {
                                    warn!(addr = %addr, error = %e, "failed to connect to peer");
                                }
                            });
                        }
                        Err(e) => {
                            warn!(addr = %addr_str, error = %e, "invalid peer address");
                        }
                    }
                }

                for addr_str in leaves {
                    let mut st = state.lock().await;
                    // Find and remove peer by address
                    let to_remove: Vec<String> = st
                        .peers
                        .iter()
                        .filter(|(_, h)| h.id.addr.to_string() == addr_str)
                        .map(|(k, _)| k.clone())
                        .collect();
                    for key in to_remove {
                        st.peers.remove(&key);
                        info!(node_id = %key, "peer removed via leave command");
                    }
                }
            }
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(&config.socket_path);
    let _ = std::fs::remove_file(&config.pid_path);
    info!("daemon stopped");
    Ok(())
}

/// Load the cookie from disk, or generate a new one.
fn load_or_generate_cookie(config: &Config) -> Result<Vec<u8>> {
    if config.cookie_path.exists() {
        let cookie = std::fs::read(&config.cookie_path)
            .context("failed to read cookie file")?;
        if cookie.is_empty() {
            anyhow::bail!("cookie file is empty: {}", config.cookie_path.display());
        }
        info!(path = %config.cookie_path.display(), "loaded cookie");
        Ok(cookie)
    } else {
        use rand::RngCore;
        let mut cookie = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut cookie);
        std::fs::write(&config.cookie_path, &cookie)
            .context("failed to write cookie file")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config.cookie_path, std::fs::Permissions::from_mode(0o600))?;
        }

        info!(path = %config.cookie_path.display(), "generated new cookie");
        Ok(cookie)
    }
}
