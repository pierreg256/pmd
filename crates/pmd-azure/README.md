# portmapd-azure — Azure Tag Discovery Plugin

Azure tag-based discovery plugin for [PMD](https://github.com/pierreg256/pmd). Automatically discovers PMD peers by querying Azure Resource Graph for VMs sharing a common tag.

## How it works

1. On startup, the plugin queries the [Azure Instance Metadata Service (IMDS)](https://learn.microsoft.com/en-us/azure/virtual-machines/instance-metadata-service) to obtain the local VM's subscription ID and private IP.
2. Obtains an OAuth2 token via the VM's **managed identity**.
3. Periodically queries the [Azure Resource Graph](https://learn.microsoft.com/en-us/azure/governance/resource-graph/) for all VMs in the subscription carrying the configured tag.
4. Reports each discovered VM's private IP as a peer to the PMD daemon.

No manual `pmd join` needed — VMs tagged the same way automatically find each other.

## Prerequisites

- Each VM must have a **system-assigned or user-assigned managed identity**.
- The managed identity must have **Reader** role on the subscription (or resource group).
- All PMD VMs must carry the same tag (default key: `pmd-cluster`).

## Usage

### Via CLI

```sh
pmd start --discovery azure-tag
```

### Via config file (`~/.pmd/config.toml`)

```toml
discovery = ["azure-tag"]
```

### Tag your VMs

Using Azure CLI:

```sh
# Tag all VMs in the cluster
az vm update -g myResourceGroup -n vm1 --set tags.pmd-cluster=prod
az vm update -g myResourceGroup -n vm2 --set tags.pmd-cluster=prod
az vm update -g myResourceGroup -n vm3 --set tags.pmd-cluster=prod
```

Or via the Azure Portal: add a tag with key `pmd-cluster` and any value (e.g. `prod`).

## Configuration

The plugin uses sensible defaults. For advanced use, configure via the Rust API:

```rust
use portmapd_azure::AzureTagPlugin;
use std::time::Duration;

let plugin = AzureTagPlugin::new()
    .with_tag_key("my-cluster-tag")        // Custom tag key (default: "pmd-cluster")
    .with_tag_value(Some("prod".into()))    // Only match a specific value (default: any)
    .with_port(4369)                        // PMD port on discovered VMs (default: 4369)
    .with_poll_interval(Duration::from_secs(60)); // Polling interval (default: 30s)
```

## How peers are discovered

The plugin runs an Azure Resource Graph (KQL) query:

```kql
Resources
| where type == 'microsoft.compute/virtualmachines'
| where tags['pmd-cluster'] == 'prod'       // or: where tags has 'pmd-cluster'
| project vmId = id
| join kind=inner (
    Resources
    | where type == 'microsoft.network/networkinterfaces'
    | mv-expand ipConfig = properties.ipConfigurations
    | project vmId = tostring(properties.virtualMachine.id),
              privateIp = tostring(ipConfig.properties.privateIPAddress)
) on vmId
| project privateIp
```

This returns the private IP of every matching VM. The plugin then reports each IP (with the configured PMD port) to the daemon's discovery channel.

## Architecture

```
┌──────────────┐     IMDS      ┌─────────────────┐
│  AzureTag    │──────────────▷│ Instance         │
│  Plugin      │               │ Metadata (169.254│
│              │◁──────────────│                  │
│              │  token + info  └─────────────────┘
│              │
│              │     ARG API   ┌─────────────────┐
│              │──────────────▷│ Azure Resource   │
│              │               │ Graph            │
│              │◁──────────────│                  │
│              │  VM private IPs└─────────────────┘
│              │
│              │──▷ discovered_tx ──▷ PMD daemon
└──────────────┘                      (connects to peers)
```

## License

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE).
