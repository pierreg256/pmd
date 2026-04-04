use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use portmapd_discovery::{DiscoveryContext, DiscoveryPlugin};
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::time::{self, Duration};
use tracing::{debug, info, warn};

const DEFAULT_BROADCAST_PORT: u16 = 4370;
const DEFAULT_INTERVAL_SECS: u64 = 10;

/// UDP broadcast discovery plugin.
///
/// Periodically sends a beacon on the broadcast address and listens
/// for beacons from other PMD instances on the same network segment.
pub struct BroadcastPlugin {
    port: u16,
    interval: Duration,
    running: Arc<AtomicBool>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Beacon {
    node_id: String,
    listen_addr: SocketAddr,
}

impl BroadcastPlugin {
    /// Create a new broadcast plugin with default settings.
    pub fn new() -> Self {
        Self {
            port: DEFAULT_BROADCAST_PORT,
            interval: Duration::from_secs(DEFAULT_INTERVAL_SECS),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set the UDP broadcast port.
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the beacon interval.
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

impl Default for BroadcastPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscoveryPlugin for BroadcastPlugin {
    fn name(&self) -> &str {
        "broadcast"
    }

    async fn start(
        &self,
        ctx: DiscoveryContext,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.running.store(true, Ordering::SeqCst);

        // Use socket2 to set SO_REUSEADDR + SO_REUSEPORT before binding,
        // allowing multiple instances to share the broadcast port.
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;
        socket.set_broadcast(true)?;
        socket.set_nonblocking(true)?;
        socket.bind(
            &std::net::SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, self.port).into(),
        )?;
        let sock = UdpSocket::from_std(socket.into())?;

        let beacon = Beacon {
            node_id: ctx.local_node.node_id.clone(),
            listen_addr: ctx.local_node.addr,
        };
        let beacon_bytes = serde_json::to_vec(&beacon)?;
        let broadcast_addr: SocketAddr = ([255, 255, 255, 255], self.port).into();

        let mut buf = vec![0u8; 1024];
        let mut interval = time::interval(self.interval);
        let mut known_peers = std::collections::HashSet::<String>::new();

        info!(port = self.port, "broadcast discovery started");

        while self.running.load(Ordering::SeqCst) {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = sock.send_to(&beacon_bytes, broadcast_addr).await {
                        warn!(error = %e, "failed to send broadcast beacon");
                    } else {
                        debug!("sent broadcast beacon");
                    }
                }
                result = sock.recv_from(&mut buf) => {
                    match result {
                        Ok((len, src)) => {
                            match serde_json::from_slice::<Beacon>(&buf[..len]) {
                                Ok(remote_beacon) => {
                                    if remote_beacon.node_id != ctx.local_node.node_id
                                        && known_peers.insert(remote_beacon.node_id.clone())
                                    {
                                        info!(
                                            node_id = %remote_beacon.node_id,
                                            addr = %remote_beacon.listen_addr,
                                            from = %src,
                                            "discovered new peer via broadcast"
                                        );
                                        let _ = ctx.discovered_tx.send(remote_beacon.listen_addr).await;
                                    }
                                }
                                Err(e) => {
                                    debug!(error = %e, from = %src, "ignoring malformed beacon");
                                }
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "broadcast recv error");
                        }
                    }
                }
            }
        }

        info!("broadcast discovery stopped");
        Ok(())
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
}
