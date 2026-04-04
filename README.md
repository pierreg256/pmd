# PMD — Port Mapper Daemon

A distributed node membership daemon inspired by Erlang's [EPMD](https://www.erlang.org/doc/apps/erts/epmd_cmd.html), written in Rust.

PMD maintains a cluster membership registry using delta-state CRDTs ([concordat](https://crates.io/crates/concordat)) with convergent state across nodes. Connections between PMD instances are encrypted via TLS ([rustls](https://crates.io/crates/rustls)). A plugin-based discovery system allows automatic peer detection.

## Features

- **Single binary** — `pmd` serves as both daemon and CLI
- **CRDT membership** — Nodes converge automatically via delta-state sync, no coordination needed
- **Encrypted transport** — All inter-node traffic uses TLS with auto-generated self-signed certificates
- **Cookie authentication** — HMAC-SHA256 challenge/response prevents unauthorized cluster joins
- **Plugin discovery** — Trait-based system for automatic peer detection (ships with a UDP broadcast plugin)
- **Zero persistence** — Purely in-memory; a restarting node rejoins the cluster fresh and receives state via delta sync

## Quick Start

```sh
# Build
cargo build --workspace

# Start daemon in foreground (dev mode)
pmd start --foreground

# In another terminal
pmd status          # Show daemon status
pmd nodes           # List cluster members
pmd join 10.0.0.2:4369   # Connect to a peer
pmd leave 10.0.0.2:4369  # Disconnect from a peer
pmd stop            # Stop the daemon
```

### Options

```
pmd start [--port 4369] [--bind 0.0.0.0] [--foreground]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--port` | `4369` | TCP listen port for inter-PMD connections |
| `--bind` | `0.0.0.0` | Bind address |
| `--foreground` | off | Run in foreground instead of daemonizing |

## Architecture

```
┌─────────────────────────────────────────────┐
│                  pmd binary                  │
│                                             │
│  CLI (clap)  ──Unix socket──▷  Daemon       │
│                                │            │
│                    ┌───────────┴──────────┐ │
│                    │    TCP + TLS (4369)  │ │
│                    │    ┌──────────────┐  │ │
│                    │    │  Peer tasks  │  │ │
│                    │    │  (per-conn)  │  │ │
│                    │    └──────┬───────┘  │ │
│                    │           │          │ │
│                    │    ┌──────▽───────┐  │ │
│                    │    │  Membership  │  │ │
│                    │    │  (CrdtDoc)   │  │ │
│                    │    └──────────────┘  │ │
│                    └─────────────────────┘ │
│                                             │
│  Discovery plugins (mpsc channel)           │
│    └─ BroadcastPlugin (UDP 4370)            │
└─────────────────────────────────────────────┘
```

### Workspace Crates

| Crate | Description |
|-------|-------------|
| `pmd` | Main binary — daemon, CLI, protocol, TLS, membership |
| `pmd-discovery` | `DiscoveryPlugin` trait definition |
| `pmd-broadcast` | UDP broadcast discovery plugin example |

## Cookie Authentication

PMD uses a shared cookie file (`~/.pmd/cookie`) for cluster authentication:

```sh
# All nodes in a cluster must share the same cookie
scp ~/.pmd/cookie user@other-node:~/.pmd/cookie
```

A random 32-byte cookie is generated on first start. During the handshake, each side proves knowledge of the cookie via HMAC-SHA256 over a random nonce.

## Writing a Discovery Plugin

Implement the `DiscoveryPlugin` trait from `pmd-discovery`:

```rust
use pmd_discovery::{DiscoveryPlugin, DiscoveryContext};

struct MyPlugin;

impl DiscoveryPlugin for MyPlugin {
    fn name(&self) -> &str { "my-plugin" }

    async fn start(&self, ctx: DiscoveryContext) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Discover peers and send their addresses
        ctx.discovered_tx.send("10.0.0.5:4369".parse()?).await?;
        Ok(())
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}
```

See [`pmd-broadcast`](crates/pmd-broadcast/) for a complete example.

## Development

```sh
# Build
cargo build --workspace

# Test (43 unit tests)
cargo test --workspace

# Lint
cargo clippy --workspace -- -D warnings
```

All three must pass before any push to GitHub. See [PLAN.md](PLAN.md) for implementation details and [ROADMAP.md](ROADMAP.md) for future plans.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
