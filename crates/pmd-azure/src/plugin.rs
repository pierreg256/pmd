use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use portmapd_discovery::{DiscoveryContext, DiscoveryPlugin};
use serde::Deserialize;
use tokio::time::{self, Duration};
use tracing::{debug, info, warn};

const DEFAULT_TAG_KEY: &str = "pmd-cluster";
const DEFAULT_PORT: u16 = 4369;
const DEFAULT_POLL_INTERVAL_SECS: u64 = 30;

/// Azure Instance Metadata Service endpoint (available on every Azure VM).
const IMDS_TOKEN_URL: &str = "http://169.254.169.254/metadata/identity/oauth2/token?api-version=2019-08-01&resource=https://management.azure.com/";
const IMDS_INSTANCE_URL: &str = "http://169.254.169.254/metadata/instance?api-version=2021-02-01";

/// Azure tag-based discovery plugin.
///
/// Discovers other PMD nodes by querying Azure Resource Graph for VMs (or
/// VM Scale Set instances) that share a common tag. This enables automatic
/// cluster formation in Azure without manual `pmd join`.
///
/// ## How it works
///
/// 1. On startup, queries the Azure Instance Metadata Service (IMDS) to get
///    the local VM's subscription ID, resource group, and private IP.
/// 2. Obtains a managed-identity OAuth2 token from IMDS.
/// 3. Periodically queries the Azure Resource Graph API to find all VMs in
///    the same subscription that carry the configured tag (default:
///    `pmd-cluster`).
/// 4. For each discovered VM, resolves its private IP and reports it as a
///    peer address to the daemon via the discovery channel.
///
/// ## Requirements
///
/// - The VM must have a **system-assigned or user-assigned managed identity**
///   with `Reader` role on the subscription (or resource group).
/// - The tag key and optional tag value must be set on all PMD VMs.
pub struct AzureTagPlugin {
    tag_key: String,
    tag_value: Option<String>,
    pmd_port: u16,
    poll_interval: Duration,
    running: Arc<AtomicBool>,
}

impl AzureTagPlugin {
    /// Create a new Azure tag discovery plugin with default settings.
    pub fn new() -> Self {
        Self {
            tag_key: DEFAULT_TAG_KEY.to_string(),
            tag_value: None,
            pmd_port: DEFAULT_PORT,
            poll_interval: Duration::from_secs(DEFAULT_POLL_INTERVAL_SECS),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set the tag key to match (default: `pmd-cluster`).
    pub fn with_tag_key(mut self, key: impl Into<String>) -> Self {
        self.tag_key = key.into();
        self
    }

    /// Set an optional tag value to match. If `None`, any value matches.
    pub fn with_tag_value(mut self, value: Option<String>) -> Self {
        self.tag_value = value;
        self
    }

    /// Set the PMD port to use for discovered peers (default: 4369).
    pub fn with_port(mut self, port: u16) -> Self {
        self.pmd_port = port;
        self
    }

    /// Set the polling interval (default: 30s).
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

impl Default for AzureTagPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscoveryPlugin for AzureTagPlugin {
    fn name(&self) -> &str {
        "azure-tag"
    }

    async fn start(
        &self,
        ctx: DiscoveryContext,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.running.store(true, Ordering::SeqCst);

        let client = reqwest::Client::new();

        // 1. Get instance metadata to know our own private IP
        let instance = fetch_instance_metadata(&client).await?;
        let subscription_id = &instance.compute.subscription_id;
        let local_ip = instance
            .network
            .interfaces
            .first()
            .and_then(|iface| iface.ipv4.ip_address.first())
            .map(|ip| ip.private_ip_address.clone())
            .unwrap_or_default();

        info!(
            subscription = %subscription_id,
            local_ip = %local_ip,
            tag = %self.tag_key,
            "azure-tag discovery started"
        );

        let mut interval = time::interval(self.poll_interval);
        let mut known_peers = HashSet::<String>::new();

        while self.running.load(Ordering::SeqCst) {
            interval.tick().await;

            // 2. Get a fresh access token
            let token = match fetch_imds_token(&client).await {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "failed to get IMDS token, will retry");
                    continue;
                }
            };

            // 3. Query Resource Graph for VMs with the tag
            let ips = match query_tagged_vms(
                &client,
                &token,
                subscription_id,
                &self.tag_key,
                self.tag_value.as_deref(),
            )
            .await
            {
                Ok(ips) => ips,
                Err(e) => {
                    warn!(error = %e, "resource graph query failed, will retry");
                    continue;
                }
            };

            // 4. Report new peers
            for ip in ips {
                if ip == local_ip {
                    continue; // skip ourselves
                }
                if known_peers.insert(ip.clone()) {
                    let addr_str = format!("{}:{}", ip, self.pmd_port);
                    if let Ok(addr) = addr_str.parse::<SocketAddr>() {
                        info!(addr = %addr, "discovered Azure peer via tag");
                        let _ = ctx.discovered_tx.send(addr).await;
                    }
                }
            }

            debug!(peers = known_peers.len(), "azure-tag poll complete");
        }

        Ok(())
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Azure API types and helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ImdsTokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct InstanceMetadata {
    compute: ComputeMetadata,
    network: NetworkMetadata,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComputeMetadata {
    subscription_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NetworkMetadata {
    #[serde(rename = "interface")]
    interfaces: Vec<NetworkInterface>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NetworkInterface {
    ipv4: Ipv4Info,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ipv4Info {
    ip_address: Vec<IpAddressEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IpAddressEntry {
    private_ip_address: String,
}

#[derive(Debug, Deserialize)]
struct ResourceGraphResponse {
    data: Vec<ResourceGraphRow>,
}

#[derive(Debug, Deserialize)]
struct ResourceGraphRow {
    #[serde(rename = "privateIp")]
    private_ip: Option<String>,
}

/// Fetch the OAuth2 token from IMDS (managed identity).
async fn fetch_imds_token(
    client: &reqwest::Client,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let resp: ImdsTokenResponse = client
        .get(IMDS_TOKEN_URL)
        .header("Metadata", "true")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(resp.access_token)
}

/// Fetch instance metadata from IMDS.
async fn fetch_instance_metadata(
    client: &reqwest::Client,
) -> Result<InstanceMetadata, Box<dyn std::error::Error + Send + Sync>> {
    let resp: InstanceMetadata = client
        .get(IMDS_INSTANCE_URL)
        .header("Metadata", "true")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(resp)
}

/// Query Azure Resource Graph for VMs matching a tag, returning their private IPs.
async fn query_tagged_vms(
    client: &reqwest::Client,
    token: &str,
    subscription_id: &str,
    tag_key: &str,
    tag_value: Option<&str>,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    // Build KQL query to find VMs with the tag and extract private IPs
    let tag_filter = if let Some(val) = tag_value {
        format!("where tags['{tag_key}'] == '{val}'")
    } else {
        format!("where tags has '{tag_key}'")
    };

    let query = format!(
        "Resources \
         | where type == 'microsoft.compute/virtualmachines' \
         | {tag_filter} \
         | project vmId = id \
         | join kind=inner ( \
             Resources \
             | where type == 'microsoft.network/networkinterfaces' \
             | mv-expand ipConfig = properties.ipConfigurations \
             | project vmId = tostring(properties.virtualMachine.id), \
                       privateIp = tostring(ipConfig.properties.privateIPAddress) \
         ) on vmId \
         | project privateIp"
    );

    let url = "https://management.azure.com/providers/Microsoft.ResourceGraph/resources?api-version=2021-03-01";

    let body = serde_json::json!({
        "subscriptions": [subscription_id],
        "query": query,
    });

    let resp: ResourceGraphResponse = client
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let ips: Vec<String> = resp
        .data
        .into_iter()
        .filter_map(|row| row.private_ip)
        .filter(|ip| !ip.is_empty())
        .collect();

    Ok(ips)
}
