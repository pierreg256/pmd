pub mod control;
pub mod membership;
pub mod peer;
pub mod server;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::{TcpListener, UnixListener};
use tokio::signal;
use tokio::sync::{Mutex, broadcast};
use tokio::time::{self, Duration};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::tls;

use self::membership::{Membership, MembershipChange};
use self::peer::PeerHandle;

/// Shared daemon state, protected by a mutex.
pub struct DaemonState {
    pub membership: Membership,
    pub listen_addr: SocketAddr,
    pub peers: HashMap<String, PeerHandle>,
    pub pending_joins: Vec<String>,
    pub pending_leaves: Vec<String>,
    pub shutdown: CancellationToken,
    /// Known peer addresses for reconnection.
    pub known_peers: HashMap<SocketAddr, ReconnectState>,
    /// Broadcast channel for membership event subscribers (Phase 7).
    pub event_tx: broadcast::Sender<MembershipChange>,
}

/// Reconnection state for a known peer.
pub struct ReconnectState {
    pub attempt: u32,
    pub next_try: tokio::time::Instant,
}

/// Run the PMD daemon.
pub async fn run(config: Config, node_id: String, metadata: HashMap<String, String>) -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls crypto provider");

    let config = Arc::new(config);
    config.ensure_dirs()?;

    let cookie = load_or_generate_cookie(&config)?;
    let cookie = Arc::new(cookie);

    let acceptor = tls::build_acceptor(&config)?;
    let connector = tls::build_connector(&config)?;

    let listen_addr: SocketAddr = format!("{}:{}", config.bind, config.port).parse()?;
    let listener = TcpListener::bind(listen_addr).await?;
    let actual_addr = listener.local_addr()?;
    info!(addr = %actual_addr, node_id = %node_id, "PMD daemon starting");

    let mut membership = Membership::new(&node_id);
    membership.add_node(&node_id, actual_addr, metadata);

    let shutdown = CancellationToken::new();
    let (event_tx, _) = broadcast::channel(256);

    let state = Arc::new(Mutex::new(DaemonState {
        membership,
        listen_addr: actual_addr,
        peers: HashMap::new(),
        pending_joins: Vec::new(),
        pending_leaves: Vec::new(),
        shutdown: shutdown.clone(),
        known_peers: HashMap::new(),
        event_tx,
    }));

    // Remove stale socket file
    let _ = std::fs::remove_file(&config.socket_path);

    let control_listener =
        UnixListener::bind(&config.socket_path).context("failed to bind control socket")?;

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
        if let Err(e) = server::run_server(
            listener,
            acceptor,
            server_config,
            server_cookie,
            server_state,
        )
        .await
        {
            error!(error = %e, "server error");
        }
    });

    // Main loop: process pending joins/leaves + reconnection + signal handling
    let mut tick = time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            // Graceful shutdown on SIGTERM/SIGINT
            _ = signal::ctrl_c() => {
                info!("received SIGINT, shutting down gracefully");
                shutdown.cancel();
                break;
            }
            _ = shutdown.cancelled() => {
                info!("shutdown requested");
                break;
            }
            _ = tick.tick() => {
                process_tick(&config, &cookie, &state, &connector).await;
            }
        }
    }

    // Graceful shutdown: remove self from membership and notify peers
    {
        let mut st = state.lock().await;
        let my_id = st.membership.node_id().to_string();
        st.membership.remove_node(&my_id);
        info!("removed self from membership");
    }

    // Cleanup
    let _ = std::fs::remove_file(&config.socket_path);
    let _ = std::fs::remove_file(&config.pid_path);
    info!("daemon stopped");
    Ok(())
}

async fn process_tick(
    config: &Arc<Config>,
    cookie: &Arc<Vec<u8>>,
    state: &Arc<Mutex<DaemonState>>,
    connector: &tokio_rustls::TlsConnector,
) {
    let mut st = state.lock().await;
    let joins: Vec<String> = st.pending_joins.drain(..).collect();
    let leaves: Vec<String> = st.pending_leaves.drain(..).collect();

    // Reconnection: try known peers whose backoff has expired
    let now = tokio::time::Instant::now();
    let reconnects: Vec<SocketAddr> = st
        .known_peers
        .iter()
        .filter(|(addr, rs)| rs.next_try <= now && !st.peers.values().any(|p| p.id.addr == **addr))
        .map(|(addr, _)| *addr)
        .collect();

    drop(st);

    for addr_str in joins {
        match addr_str.parse::<SocketAddr>() {
            Ok(addr) => {
                // Track for reconnection
                {
                    let mut st = state.lock().await;
                    st.known_peers.entry(addr).or_insert(ReconnectState {
                        attempt: 0,
                        next_try: tokio::time::Instant::now(),
                    });
                }
                spawn_connect(addr, config, cookie, state, connector);
            }
            Err(e) => {
                warn!(addr = %addr_str, error = %e, "invalid peer address");
            }
        }
    }

    for addr_str in leaves {
        let mut st = state.lock().await;
        let to_remove: Vec<String> = st
            .peers
            .iter()
            .filter(|(_, h)| h.id.addr.to_string() == addr_str)
            .map(|(k, _)| k.clone())
            .collect();
        for key in &to_remove {
            st.peers.remove(key);
            info!(node_id = %key, "peer removed via leave command");
        }
        // Remove from known_peers so we don't reconnect
        if let Ok(addr) = addr_str.parse::<SocketAddr>() {
            st.known_peers.remove(&addr);
        }
    }

    // Reconnect to known peers
    for addr in reconnects {
        // Update backoff before attempting
        {
            let mut st = state.lock().await;
            if let Some(rs) = st.known_peers.get_mut(&addr) {
                rs.attempt += 1;
                let delay = std::cmp::min(
                    config.reconnect_base_secs * 2u64.saturating_pow(rs.attempt),
                    config.reconnect_max_secs,
                );
                rs.next_try = tokio::time::Instant::now() + Duration::from_secs(delay);
            }
        }
        spawn_connect(addr, config, cookie, state, &connector.clone());
    }
}

fn spawn_connect(
    addr: SocketAddr,
    config: &Arc<Config>,
    cookie: &Arc<Vec<u8>>,
    state: &Arc<Mutex<DaemonState>>,
    connector: &tokio_rustls::TlsConnector,
) {
    let config = Arc::clone(config);
    let cookie = Arc::clone(cookie);
    let state = Arc::clone(state);
    let connector = connector.clone();
    tokio::spawn(async move {
        match peer::connect_to_peer(addr, config, cookie, Arc::clone(&state), connector).await {
            Ok(_) => {
                // Reset backoff on success
                let mut st = state.lock().await;
                if let Some(rs) = st.known_peers.get_mut(&addr) {
                    rs.attempt = 0;
                    rs.next_try = tokio::time::Instant::now();
                }
            }
            Err(e) => {
                warn!(addr = %addr, error = %e, "failed to connect to peer");
            }
        }
    });
}

/// Broadcast membership change events to subscribers.
pub fn broadcast_events(state: &DaemonState, changes: &[MembershipChange]) {
    for change in changes {
        let _ = state.event_tx.send(change.clone());
    }
}

/// Load the cookie from disk, or generate a new one.
fn load_or_generate_cookie(config: &Config) -> Result<Vec<u8>> {
    if config.cookie_path.exists() {
        let cookie = std::fs::read(&config.cookie_path).context("failed to read cookie file")?;
        if cookie.is_empty() {
            anyhow::bail!("cookie file is empty: {}", config.cookie_path.display());
        }
        info!(path = %config.cookie_path.display(), "loaded cookie");
        Ok(cookie)
    } else {
        use rand::RngCore;
        let mut cookie = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut cookie);
        std::fs::write(&config.cookie_path, &cookie).context("failed to write cookie file")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config.cookie_path, std::fs::Permissions::from_mode(0o600))?;
        }

        info!(path = %config.cookie_path.display(), "generated new cookie");
        Ok(cookie)
    }
}
