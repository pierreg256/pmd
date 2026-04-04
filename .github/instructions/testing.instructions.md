---
description: "Use when writing or modifying tests, test helpers, or test infrastructure."
applyTo: "**/tests/**"
---
# Testing Conventions

## Hard Rules

- **Nothing is pushed to GitHub unless all tests pass**: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
- **Every new function or module must have tests** — no exceptions
- **Every bug fix must include a regression test** that reproduces the bug before the fix

## Structure

- Unit tests in the same file with `#[cfg(test)] mod tests { ... }`
- Integration tests under `crates/<crate>/tests/` directory
- Test helpers go in a `tests/common/mod.rs` module (shared fixtures, test CAs, etc.)
- Use `#[tokio::test]` for async tests
- Test names: `test_<unit>_<scenario>_<expected>` e.g. `test_handshake_invalid_cookie_rejected`

## What Must Be Tested

### Protocol (`protocol.rs`)
- Roundtrip serialize/deserialize for **every** `Message` variant
- Frame encode/decode with valid and malformed inputs
- Oversized frame rejection (> 16 MiB)
- HMAC computation and verification (valid cookie, invalid cookie, empty cookie)

### Membership (`membership.rs`)
- Add node → appears in `members()`
- Remove node → disappears from `members()`
- Merge convergence: 2 replicas with concurrent mutations converge to same state
- Merge convergence: 3+ replicas in chain topology converge
- Join/leave detection: `merge_remote()` returns correct `MembershipChange` events
- Delta encoding: `delta_for_peer(peer_vv)` produces minimal delta
- Idempotent merge: applying same delta twice has no effect

### TLS (`tls.rs`)
- Self-signed cert generation produces valid PEM files
- `build_acceptor()` / `build_connector()` succeed with generated certs
- TLS handshake succeeds between server and client with matching certs
- Use test CAs — never disable verification in tests

### Control socket (`control.rs`)
- Each `ControlRequest` variant produces expected `ControlResponse`
- Malformed JSON is handled gracefully
- Concurrent clients don't corrupt state

### Peer (`peer.rs`) — Integration tests
- Two PMD instances connect, handshake with valid cookie, exchange membership
- Handshake with invalid cookie is rejected
- Heartbeat keeps connection alive
- Peer disconnect is detected

### Discovery plugins
- `BroadcastPlugin` sends and receives beacons on loopback
- Unknown beacon triggers `discovered_tx` channel send
- Own beacon is ignored (no self-discovery)

## Network Testing Rules

- Always bind to `127.0.0.1:0` for random port assignment
- Never hardcode ports — tests must run in parallel
- Use `tokio::time::timeout()` to prevent hanging tests (max 10s per test)
- Clean up temp files (sockets, PID files, certs) after each test
