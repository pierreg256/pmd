use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Result, bail};
use concordat::vv::VersionVector;
use rand::RngCore;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::sync::Mutex;
use tokio::time::{self, Duration};
use tokio_rustls::server::TlsStream as ServerTlsStream;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::daemon::DaemonState;
use crate::daemon::failure_detector::PhiAccrualDetector;
use crate::protocol::{
    Message, PROTOCOL_VERSION, compute_cookie_hmac, read_frame, verify_cookie_hmac, write_frame,
};

/// Unique identifier for a connected peer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerId {
    pub node_id: String,
    pub addr: SocketAddr,
}

/// Information tracked per connected peer.
pub struct PeerHandle {
    pub id: PeerId,
    pub last_vv: VersionVector,
    /// Phi accrual failure detector for this peer.
    pub phi_detector: PhiAccrualDetector,
    /// Channel to signal this peer's loop to perform a gossip sync.
    pub gossip_tx: tokio::sync::mpsc::Sender<()>,
}

// ---------------------------------------------------------------------------
// Inbound peer (we accepted the connection)
// ---------------------------------------------------------------------------

/// Handle a newly accepted inbound TLS connection.
pub async fn handle_inbound_peer(
    stream: ServerTlsStream<tokio::net::TcpStream>,
    config: Arc<Config>,
    cookie: Arc<Vec<u8>>,
    state: Arc<Mutex<DaemonState>>,
) {
    let (reader, writer) = tokio::io::split(stream);
    if let Err(e) = run_inbound(reader, writer, config, cookie, state).await {
        debug!(error = %e, "inbound peer session ended");
    }
}

async fn run_inbound<R, W>(
    mut reader: ReadHalf<R>,
    mut writer: WriteHalf<W>,
    config: Arc<Config>,
    cookie: Arc<Vec<u8>>,
    state: Arc<Mutex<DaemonState>>,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    // 1. Read Handshake from initiator
    let msg = read_frame(&mut reader)
        .await?
        .ok_or_else(|| anyhow::anyhow!("connection closed before handshake"))?;

    let (remote_node_id, remote_addr, _remote_nonce) = match msg {
        Message::Handshake {
            node_id,
            listen_addr,
            nonce,
            cookie_hmac,
            protocol_version,
            ..
        } => {
            if protocol_version != PROTOCOL_VERSION {
                bail!(
                    "protocol version mismatch: remote={protocol_version}, local={PROTOCOL_VERSION}"
                );
            }
            if !verify_cookie_hmac(&cookie, &nonce, &cookie_hmac) {
                bail!("invalid cookie HMAC from {node_id}");
            }
            (node_id, listen_addr, nonce)
        }
        other => bail!("expected Handshake, got {other:?}"),
    };

    info!(node_id = %remote_node_id, addr = %remote_addr, "inbound peer authenticated");

    // Check + register atomically. If already connected, reject.
    let (our_nonce, our_hmac, vv, our_node_id, listen_addr, mut gossip_rx) = {
        let mut st = state.lock().await;
        if st.peers.contains_key(&remote_node_id) {
            info!(node_id = %remote_node_id, "rejecting duplicate inbound connection");
            return Ok(());
        }

        let mut nonce = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut nonce);
        let hmac = compute_cookie_hmac(&cookie, &nonce);
        let vv = st.membership.version_vector().clone();
        let our_node_id = st.membership.node_id().to_string();
        let listen_addr: SocketAddr = format!("{}:{}", config.bind, config.port).parse()?;

        let (gossip_tx, gossip_rx) = tokio::sync::mpsc::channel(1);
        st.peers.insert(
            remote_node_id.clone(),
            PeerHandle {
                id: PeerId {
                    node_id: remote_node_id.clone(),
                    addr: remote_addr,
                },
                last_vv: VersionVector::new(),
                phi_detector: PhiAccrualDetector::new(
                    config.phi_window_size,
                    config.phi_min_std_deviation_ms,
                ),
                gossip_tx,
            },
        );

        (nonce, hmac, vv, our_node_id, listen_addr, gossip_rx)
    };

    // Reply with HandshakeAck (peer is already registered, no race window)
    write_frame(
        &mut writer,
        &Message::HandshakeAck {
            node_id: our_node_id,
            listen_addr,
            nonce: our_nonce,
            cookie_hmac: our_hmac,
            vv: vv.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .await?;

    // 4. Send initial full delta
    {
        let st = state.lock().await;
        let delta_bytes = st.membership.full_delta();
        let sender_vv = st.membership.version_vector().clone();
        drop(st);
        write_frame(
            &mut writer,
            &Message::MembershipSync {
                delta_bytes,
                sender_vv,
            },
        )
        .await?;
    }

    // 5. Run message loop
    let result = peer_message_loop(
        &mut reader,
        &mut writer,
        &remote_node_id,
        &config,
        &cookie,
        &state,
        &mut gossip_rx,
    )
    .await;

    // 6. Cleanup
    {
        let mut st = state.lock().await;
        st.peers.remove(&remote_node_id);
    }
    info!(node_id = %remote_node_id, "inbound peer disconnected");

    result
}

// ---------------------------------------------------------------------------
// Outbound peer (we initiated the connection)
// ---------------------------------------------------------------------------

/// Connect to a remote peer and run the peer session.
pub async fn connect_to_peer(
    addr: SocketAddr,
    config: Arc<Config>,
    cookie: Arc<Vec<u8>>,
    state: Arc<Mutex<DaemonState>>,
    connector: tokio_rustls::TlsConnector,
) -> Result<()> {
    let tcp = tokio::net::TcpStream::connect(addr).await?;
    let server_name = rustls::pki_types::ServerName::try_from("pmd-node")?;
    let stream = connector.connect(server_name, tcp).await?;

    let (mut reader, mut writer) = tokio::io::split(stream);

    // 1. Send Handshake
    let mut nonce = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut nonce);
    let hmac = compute_cookie_hmac(&cookie, &nonce);

    let our_node_id = state.lock().await.membership.node_id().to_string();
    let listen_addr: SocketAddr = format!("{}:{}", config.bind, config.port).parse()?;

    write_frame(
        &mut writer,
        &Message::Handshake {
            node_id: our_node_id.clone(),
            listen_addr,
            nonce,
            cookie_hmac: hmac,
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .await?;

    // 2. Read HandshakeAck
    let msg = read_frame(&mut reader)
        .await?
        .ok_or_else(|| anyhow::anyhow!("connection closed before HandshakeAck"))?;

    let (remote_node_id, remote_addr, remote_vv) = match msg {
        Message::HandshakeAck {
            node_id,
            listen_addr,
            nonce: remote_nonce,
            cookie_hmac: remote_hmac,
            vv,
            protocol_version,
            ..
        } => {
            if protocol_version != PROTOCOL_VERSION {
                bail!(
                    "protocol version mismatch: remote={protocol_version}, local={PROTOCOL_VERSION}"
                );
            }
            if !verify_cookie_hmac(&cookie, &remote_nonce, &remote_hmac) {
                bail!("invalid cookie HMAC from {node_id}");
            }
            (node_id, listen_addr, vv)
        }
        other => bail!("expected HandshakeAck, got {other:?}"),
    };

    info!(node_id = %remote_node_id, addr = %remote_addr, "outbound peer authenticated");

    // Check + register atomically. If already connected (e.g. inbound won), drop.
    let mut gossip_rx = {
        let mut st = state.lock().await;
        if st.peers.contains_key(&remote_node_id) {
            info!(node_id = %remote_node_id, "dropping duplicate outbound connection");
            return Ok(());
        }
        let (gossip_tx, gossip_rx) = tokio::sync::mpsc::channel(1);
        st.peers.insert(
            remote_node_id.clone(),
            PeerHandle {
                id: PeerId {
                    node_id: remote_node_id.clone(),
                    addr: remote_addr,
                },
                last_vv: remote_vv,
                phi_detector: PhiAccrualDetector::new(
                    config.phi_window_size,
                    config.phi_min_std_deviation_ms,
                ),
                gossip_tx,
            },
        );
        gossip_rx
    };

    // 4. Send initial full delta
    {
        let st = state.lock().await;
        let delta_bytes = st.membership.full_delta();
        let sender_vv = st.membership.version_vector().clone();
        drop(st);
        write_frame(
            &mut writer,
            &Message::MembershipSync {
                delta_bytes,
                sender_vv,
            },
        )
        .await?;
    }

    // 5. Message loop
    let result = peer_message_loop(
        &mut reader,
        &mut writer,
        &remote_node_id,
        &config,
        &cookie,
        &state,
        &mut gossip_rx,
    )
    .await;

    // 6. Cleanup
    {
        let mut st = state.lock().await;
        st.peers.remove(&remote_node_id);
    }
    info!(node_id = %remote_node_id, "outbound peer disconnected");

    result
}

// ---------------------------------------------------------------------------
// Shared message loop (heartbeat + sync + read)
// ---------------------------------------------------------------------------

async fn peer_message_loop<R, W>(
    reader: &mut R,
    writer: &mut W,
    remote_node_id: &str,
    config: &Config,
    _cookie: &[u8],
    state: &Arc<Mutex<DaemonState>>,
    gossip_rx: &mut tokio::sync::mpsc::Receiver<()>,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut heartbeat_interval =
        time::interval(Duration::from_secs(config.heartbeat_interval_secs));
    // Phi check runs at a fraction of the heartbeat interval so we detect
    // failures promptly without burning CPU.
    let mut phi_check_interval =
        time::interval(Duration::from_secs(config.heartbeat_interval_secs));
    let shutdown = state.lock().await.shutdown.clone();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                debug!(peer = %remote_node_id, "shutdown signal, closing peer");
                return Ok(());
            }
            _ = heartbeat_interval.tick() => {
                write_frame(writer, &Message::Heartbeat).await?;
            }
            _ = phi_check_interval.tick() => {
                // Check phi accrual failure detector
                let st = state.lock().await;
                if let Some(peer) = st.peers.get(remote_node_id) {
                    let phi = peer.phi_detector.phi();
                    if phi > config.phi_threshold {
                        warn!(
                            peer = %remote_node_id,
                            phi = phi,
                            threshold = config.phi_threshold,
                            "phi accrual threshold exceeded, declaring peer dead"
                        );
                        drop(st);
                        return Ok(());
                    }
                }
            }
            Some(()) = gossip_rx.recv() => {
                // Gossip tick: the main loop selected us for a delta sync
                let st = state.lock().await;
                if let Some(peer) = st.peers.get(remote_node_id) {
                    let delta_bytes = st.membership.delta_for_peer(&peer.last_vv);
                    let sender_vv = st.membership.version_vector().clone();
                    drop(st);
                    write_frame(writer, &Message::MembershipSync { delta_bytes, sender_vv }).await?;
                }
            }
            result = read_frame(reader) => {
                match result? {
                    Some(Message::Heartbeat) => {
                        write_frame(writer, &Message::HeartbeatAck).await?;
                    }
                    Some(Message::HeartbeatAck) => {
                        // Update the phi accrual failure detector
                        let mut st = state.lock().await;
                        if let Some(peer) = st.peers.get_mut(remote_node_id) {
                            peer.phi_detector.heartbeat();
                        }
                        debug!(peer = %remote_node_id, "heartbeat ack");
                    }
                    Some(Message::MembershipSync { delta_bytes, sender_vv }) => {
                        let mut st = state.lock().await;
                        let changes = st.membership.merge_remote(&delta_bytes)?;
                        if let Some(peer) = st.peers.get_mut(remote_node_id) {
                            peer.last_vv = sender_vv;
                        }
                        // Broadcast notifications for changes
                        for change in &changes {
                            info!(
                                event = ?change.event,
                                node_id = %change.node_id,
                                addr = %change.addr,
                                "membership change"
                            );
                        }
                        crate::daemon::broadcast_events(&st, &changes);
                    }
                    Some(Message::Notification { event, node_id, addr }) => {
                        info!(?event, node_id = %node_id, addr = %addr, "peer notification");
                    }
                    Some(other) => {
                        warn!(peer = %remote_node_id, msg = ?other, "unexpected message in peer loop");
                    }
                    None => {
                        // Clean EOF
                        return Ok(());
                    }
                }
            }
        }
    }
}
