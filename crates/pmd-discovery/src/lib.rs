use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Information about a node in the PMD cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: String,
    pub addr: SocketAddr,
}

/// Context provided to discovery plugins by the daemon.
pub struct DiscoveryContext {
    /// Information about the local PMD node.
    pub local_node: NodeInfo,
    /// Channel to report discovered peer addresses.
    pub discovered_tx: mpsc::Sender<SocketAddr>,
}

/// Trait that all discovery plugins must implement.
///
/// Plugins run as async tasks and report discovered peers via
/// the `DiscoveryContext::discovered_tx` channel. They must never
/// open TCP connections directly.
#[allow(async_fn_in_trait)]
pub trait DiscoveryPlugin: Send + Sync {
    /// Human-readable name of this plugin.
    fn name(&self) -> &str;

    /// Start the discovery loop. Runs until cancelled or an error occurs.
    async fn start(
        &self,
        ctx: DiscoveryContext,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Signal the plugin to stop.
    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}
