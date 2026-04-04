# PMD — Port Mapper Daemon

## Project Overview

PMD is an Erlang EPMD-inspired distributed daemon written in Rust. It maintains a cluster membership registry using delta-state CRDTs (via the `concordat` crate) with convergent state across nodes. Connections between PMD instances are encrypted via mTLS (rustls). A plugin-based discovery system allows automatic peer detection.

## Architecture

- **Single binary** `pmd`: CLI subcommands dispatch between daemon mode and client commands
- **Workspace layout**: Cargo workspace with 3 crates under `crates/`
  - `pmd` — main binary (daemon + CLI)
  - `pmd-discovery` — `DiscoveryPlugin` trait definition
  - `pmd-broadcast` — UDP broadcast discovery plugin example
- **Daemon ↔ CLI**: communication via Unix domain socket (`~/.pmd/pmd.sock`), JSON-encoded
- **Inter-PMD**: TCP + TLS (port 4369 default), length-prefixed bincode frames
- **Membership state**: `concordat::CrdtDoc` storing nodes at `/nodes/<node_id>`
- **Cookie auth**: HMAC-SHA256 challenge/response during handshake

## Code Style

- **Rust edition**: 2024
- **Async runtime**: Tokio (full features)
- **Error handling**: `anyhow::Result` for binaries, `thiserror` for library crates
- **Naming**: snake_case for modules/functions, CamelCase for types. No abbreviations except `vv` (VersionVector), `tls`, `id`
- **Imports**: group std → external crates → internal modules, separated by blank lines
- **Unsafe**: forbidden unless justified with a `// SAFETY:` comment

## Build & Test

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `tokio` | Async runtime |
| `clap` (derive) | CLI parsing |
| `serde` + `bincode` | Wire protocol |
| `serde_json` | Control socket protocol |
| `rustls` + `tokio-rustls` | TLS |
| `rcgen` | Self-signed cert generation |
| `concordat` | Delta-state CRDT |
| `tracing` | Structured logging |
| `hmac` + `sha2` | Cookie authentication |

## Conventions

- Every public function in library crates must have a doc comment
- Protocol messages are defined in `protocol.rs` — never spread across modules
- All network I/O goes through `daemon/server.rs` (accept) or `daemon/peer.rs` (per-peer loop)
- The membership CRDT is the single source of truth — never maintain parallel state
- Discovery plugins communicate via `tokio::sync::mpsc` channel, never directly open connections

## Workflow

- After completing each step or phase from `PLAN.md`, **commit and push** (`git add -A && git commit -m "<message>" && git push`)
- Commit messages should reference the phase/step number, e.g. `Phase 2, step 7: TLS setup`
- Never leave uncommitted work at the end of a phase
