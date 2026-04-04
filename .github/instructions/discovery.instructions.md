---
description: "Use when working on the discovery plugin system, DiscoveryPlugin trait, or the UDP broadcast plugin."
applyTo: ["crates/pmd-discovery/**", "crates/pmd-broadcast/**"]
---
# Discovery Plugin Conventions

- `DiscoveryPlugin` trait is defined in `pmd-discovery` crate — keep it minimal
- Plugins never open TCP connections directly — they send discovered `SocketAddr` via `tokio::sync::mpsc`
- The daemon is responsible for initiating connections to discovered peers
- Each plugin gets a `DiscoveryContext` with `local_node` info and a `Sender<SocketAddr>` channel
- Plugins must be `Send + Sync` and run as async tasks
- The broadcast plugin is an example for developers — keep it simple and well-documented
