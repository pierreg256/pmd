pub mod control;
pub mod failure_detector;
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
    /// Set by `pmd stop` — the main loop will perform graceful shutdown.
    pub shutdown_requested: bool,
    /// Broadcast channel for membership event subscribers.
    pub event_tx: broadcast::Sender<MembershipChange>,
}

/// Run the PMD daemon.
pub async fn run(
    config: Config,
    node_id: String,
    metadata: HashMap<String, String>,
    discovery_plugins: Vec<String>,
) -> Result<()> {
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
        shutdown_requested: false,
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

    // Spawn discovery plugins
    let (discovered_tx, mut discovered_rx) = tokio::sync::mpsc::channel::<SocketAddr>(64);
    start_discovery_plugins(
        &discovery_plugins,
        &node_id,
        actual_addr,
        discovered_tx,
        shutdown.clone(),
    );

    // Task: forward discovered peers into pending_joins (deduplicated)
    let discovery_state = Arc::clone(&state);
    tokio::spawn(async move {
        while let Some(addr) = discovered_rx.recv().await {
            let mut st = discovery_state.lock().await;
            let already_connected = st.peers.values().any(|p| p.id.addr == addr);
            let already_pending = st.pending_joins.iter().any(|a| a == &addr.to_string());
            if !already_connected && !already_pending {
                info!(addr = %addr, "discovery: queueing peer connection");
                st.pending_joins.push(addr.to_string());
            }
        }
    });

    // Main loop: process pending joins/leaves + gossip tick
    let mut tick = time::interval(Duration::from_millis(500));
    let mut gossip_tick = time::interval(Duration::from_secs(config.sync_interval_secs));
    // Mesh expansion: periodically connect to an indirect node to build
    // a richer peer topology instead of staying in a pure star.
    let mut mesh_tick = time::interval(Duration::from_secs(config.sync_interval_secs * 2));
    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("received SIGINT, shutting down gracefully");
                break;
            }
            _ = tick.tick() => {
                // Check for shutdown request from `pmd stop`
                {
                    let st = state.lock().await;
                    if st.shutdown_requested {
                        info!("shutdown requested");
                        break;
                    }
                }
                process_tick(&config, &cookie, &state, &connector).await;
            }
            _ = gossip_tick.tick() => {
                gossip_sync_random_peer(&state).await;
            }
            _ = mesh_tick.tick() => {
                mesh_expand_random_peer(&state).await;
            }
        }
    }

    // Graceful shutdown: remove self from membership, push to all peers,
    // then cancel the shutdown token so peer tasks exit.
    {
        let mut st = state.lock().await;
        let my_id = st.membership.node_id().to_string();
        st.membership.remove_node(&my_id);
        info!("removed self from membership");
        // Signal all connected peers to sync immediately so they receive
        // the removal delta before we tear down connections.
        for peer in st.peers.values() {
            let _ = peer.gossip_tx.try_send(());
        }
    }

    // Give peer tasks time to send the final sync before shutting down.
    tokio::time::sleep(Duration::from_millis(500)).await;
    shutdown.cancel();

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
    drop(st);

    for addr_str in joins {
        match addr_str.parse::<SocketAddr>() {
            Ok(addr) => {
                let config = Arc::clone(config);
                let cookie = Arc::clone(cookie);
                let state = Arc::clone(state);
                let connector = connector.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        peer::connect_to_peer(addr, config, cookie, state, connector).await
                    {
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
    }
}

/// Broadcast membership change events to subscribers.
pub fn broadcast_events(state: &DaemonState, changes: &[MembershipChange]) {
    for change in changes {
        let _ = state.event_tx.send(change.clone());
    }
}

/// Gossip: pick one random connected peer and signal it to perform a delta sync.
async fn gossip_sync_random_peer(state: &Arc<Mutex<DaemonState>>) {
    use rand::seq::SliceRandom;

    let st = state.lock().await;
    let peer_ids: Vec<String> = st.peers.keys().cloned().collect();
    if peer_ids.is_empty() {
        return;
    }
    let chosen = peer_ids.choose(&mut rand::thread_rng());
    if let Some(id) = chosen
        && let Some(peer) = st.peers.get(id)
    {
        let tx = peer.gossip_tx.clone();
        drop(st);
        // Non-blocking send — if the channel is full, skip this tick.
        let _ = tx.try_send(());
    }
}

/// Mesh expansion: pick one random node known via CRDT that we're NOT directly
/// connected to, and queue a connection attempt. Over time this turns the star
/// topology into a partial mesh, improving gossip dissemination and resilience.
///
/// Only expands when the node has fewer direct peers than the cluster size
/// would suggest (target: sqrt(n) peers for good gossip dissemination).
async fn mesh_expand_random_peer(state: &Arc<Mutex<DaemonState>>) {
    use rand::seq::SliceRandom;

    let mut st = state.lock().await;
    let my_id = st.membership.node_id().to_string();
    let members = st.membership.members();
    let cluster_size = members.len();
    let direct_peers = st.peers.len();

    // Target: at least sqrt(n) peers for O(log n) gossip convergence.
    // Don't expand if we already have enough peers.
    let target = (cluster_size as f64).sqrt().ceil() as usize;
    if direct_peers >= target || direct_peers >= cluster_size.saturating_sub(1) {
        return;
    }

    // Find nodes we know about but aren't directly connected to.
    let indirect: Vec<_> = members
        .iter()
        .filter(|n| n.node_id != my_id && !st.peers.contains_key(&n.node_id))
        .collect();

    if indirect.is_empty() {
        return;
    }

    if let Some(node) = indirect.choose(&mut rand::thread_rng()) {
        let addr = node.addr.to_string();
        if !st.pending_joins.contains(&addr) {
            info!(addr = %addr, node_id = %node.node_id, peers = direct_peers, target, "mesh expansion: connecting to indirect peer");
            st.pending_joins.push(addr);
        }
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

/// Instantiate and start discovery plugins based on their names.
fn start_discovery_plugins(
    plugins: &[String],
    node_id: &str,
    listen_addr: SocketAddr,
    discovered_tx: tokio::sync::mpsc::Sender<SocketAddr>,
    shutdown: CancellationToken,
) {
    use portmapd_discovery::{DiscoveryContext, DiscoveryPlugin, NodeInfo};

    for name in plugins {
        match name.as_str() {
            "broadcast" => {
                let plugin = portmapd_broadcast::BroadcastPlugin::new();
                let ctx = DiscoveryContext {
                    local_node: NodeInfo {
                        node_id: node_id.to_string(),
                        addr: listen_addr,
                    },
                    discovered_tx: discovered_tx.clone(),
                };
                let plugin_name = plugin.name().to_string();
                let shutdown = shutdown.clone();
                info!(plugin = %plugin_name, "starting discovery plugin");
                tokio::spawn(async move {
                    tokio::select! {
                        result = plugin.start(ctx) => {
                            if let Err(e) = result {
                                error!(plugin = %plugin_name, error = %e, "discovery plugin error");
                            }
                        }
                        _ = shutdown.cancelled() => {
                            let _ = plugin.stop().await;
                            info!(plugin = %plugin_name, "discovery plugin stopped");
                        }
                    }
                });
            }
            other => {
                warn!(plugin = %other, "unknown discovery plugin, skipping");
            }
        }
    }
}
