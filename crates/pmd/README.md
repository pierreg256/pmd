# PMD — Port Mapper Daemon

A distributed node membership daemon inspired by Erlang's [EPMD](https://www.erlang.org/doc/apps/erts/epmd_cmd.html), written in Rust.

PMD maintains a cluster membership registry using delta-state CRDTs ([concordat](https://crates.io/crates/concordat)) with convergent state across nodes. All inter-node traffic is encrypted via mTLS ([rustls](https://crates.io/crates/rustls)). A plugin-based discovery system allows automatic peer detection.

## Features

- **Single binary** — `pmd` serves as both daemon and CLI
- **CRDT membership** — Nodes converge automatically via delta-state sync, no coordination needed
- **Gossip protocol** — Each node periodically picks a random peer and exchanges CRDT deltas, ensuring efficient O(1)-per-tick convergence
- **Phi Accrual Failure Detector** — Adaptive failure detection based on heartbeat inter-arrival statistics (Hayashibara et al.)
- **mTLS transport** — Auto-generated self-signed certificates, optional shared CA mode
- **Cookie authentication** — HMAC-SHA256 challenge/response handshake
- **Plugin discovery** — Trait-based system for automatic peer detection (ships with UDP broadcast plugin)
- **Service registration** — Register, unregister, and look up named services across the cluster
- **Event subscriptions** — Stream real-time join/leave events
- **Zero persistence** — Purely in-memory; restarting nodes rejoin fresh via delta sync

## Installation

```sh
cargo install portmapd
```

## Quick Start

```sh
# Start daemon in foreground
pmd start --foreground

# In another terminal — basic operations
pmd status                      # Show daemon status
pmd nodes                       # List cluster members
pmd join 10.0.0.2:4369          # Connect to a peer
pmd leave 10.0.0.2:4369         # Disconnect from a peer
pmd stop                        # Stop the daemon

# Service registration
pmd register myapp -P 8080      # Register a service
pmd lookup myapp                # Find service across cluster
pmd unregister myapp            # Remove service

# Event streaming
pmd subscribe                   # Stream join/leave events
```

## CLI Reference

```
pmd start [--port 4369] [--bind ADDR] [--foreground] [--config PATH] [--discovery broadcast]
pmd stop [--port 4369]
pmd status [--port 4369]
pmd nodes [--port 4369]
pmd join ADDR [--port 4369]
pmd leave ADDR [--port 4369]
pmd register NAME -P PORT [--port 4369]
pmd unregister NAME [--port 4369]
pmd lookup NAME [--port 4369]
pmd subscribe [--port 4369]
```

The `--port` flag selects which local daemon instance to talk to (default 4369).

## Configuration

PMD reads an optional TOML config file at `~/.pmd/config.toml`:

```toml
port = 4369
bind = "0.0.0.0"

# TLS — optional shared CA for production
# ca_cert_path = "/path/to/ca.pem"

# Gossip intervals
heartbeat_interval_secs = 2
sync_interval_secs = 5

# Phi accrual failure detector
phi_threshold = 8.0
phi_window_size = 1000
phi_min_std_deviation_ms = 500

# Node metadata (replicated across cluster)
[metadata]
role = "web"
datacenter = "us-east-1"
```

## Cookie Authentication

All nodes in a cluster must share the same cookie file (`~/.pmd/cookie`). A random 32-byte cookie is generated on first start. Copy it to other nodes:

```sh
scp ~/.pmd/cookie user@other-node:~/.pmd/cookie
```

## Related Crates

| Crate | Description |
|-------|-------------|
| [`portmapd-discovery`](https://crates.io/crates/portmapd-discovery) | `DiscoveryPlugin` trait definition |
| [`portmapd-broadcast`](https://crates.io/crates/portmapd-broadcast) | UDP broadcast discovery plugin |

## License

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE) at your option.
