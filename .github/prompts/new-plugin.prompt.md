---
description: "Create a new discovery plugin for PMD"
agent: "discovery"
argument-hint: "Describe the discovery mechanism (e.g., mDNS, DNS-SD, cloud API)"
---
Create a new discovery plugin for the PMD daemon.

1. Read `crates/pmd-discovery/src/lib.rs` for the `DiscoveryPlugin` trait
2. Read `crates/pmd-broadcast/src/lib.rs` as a reference implementation
3. Create a new crate under `crates/pmd-<name>/`
4. Implement the `DiscoveryPlugin` trait
5. Add the crate to the workspace `Cargo.toml`
6. Add tests
7. Document the plugin configuration in its `lib.rs` doc comments
