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
cargo fmt --all -- --check
```

**Nothing gets pushed to GitHub without passing all four commands above.** This is a hard gate — no exceptions.

## Testing Policy

- **Every new function or module must have tests** — unit tests at minimum, integration tests for anything involving I/O
- **Unit tests** live in `#[cfg(test)] mod tests { ... }` in the same file
- **Integration tests** live under `crates/<crate>/tests/` for cross-module or network scenarios
- **Test before commit**: run `cargo test --workspace && cargo clippy --workspace -- -D warnings` before every commit. If tests fail, fix them — never push broken code
- **Protocol tests**: every `Message` variant must have a roundtrip serialize/deserialize test
- **Membership tests**: verify CRDT merge convergence with 2+ replicas, test join/leave detection
- **TLS tests**: use test CAs, never skip certificate verification
- **Network tests**: always bind to `127.0.0.1:0` for random ports — never hardcode ports
- **Cookie tests**: test valid HMAC accepted, invalid HMAC rejected, empty cookie rejected
- **No `#[ignore]` without a comment** explaining why and a tracking issue

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
- **Before every push**: run `cargo build --workspace && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check` — all four must pass with zero failures and zero warnings (dead-code warnings for future phases excepted)
- **Update `PLAN.md`** after every completed step: mark it with ✅, add a brief note of what was done. This keeps the plan as the single source of truth for project progress
