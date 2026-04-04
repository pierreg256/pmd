# Roadmap

## Current Status

Phases 1–4 are complete. The core daemon is functional with CLI, TLS transport, CRDT membership, and plugin-based discovery.

## Short Term

### Phase 5: Polish & Robustness (in progress)

- [ ] **Graceful shutdown** — SIGTERM/SIGINT handler that notifies peers of departure, closes TLS connections, and cleans up PID/socket files
- [ ] **Automatic reconnection** — Exponential backoff reconnection to known peers after disconnect
- [ ] **Structured logging** — `tracing` subscriber configured for journald (daemon mode) or stdout (foreground mode)
- [ ] **Integration tests** — End-to-end tests: two-instance handshake, invalid cookie rejection, heartbeat liveness, broadcast plugin discovery on loopback

## Medium Term

### Phase 6: Production Readiness

- [ ] **Shared CA mode** — Support admin-distributed CA certificates for production mTLS (instead of self-signed)
- [ ] **Configuration file** — TOML config file (`~/.pmd/config.toml`) for all settings instead of CLI-only
- [ ] **Metrics** — Expose Prometheus-compatible metrics (peer count, sync latency, message rates)
- [ ] **State persistence (optional)** — Opt-in disk persistence of CRDT state for faster recovery after restart
- [ ] **Systemd integration** — Systemd unit file, socket activation, journald log integration
- [ ] **Cross-platform** — Windows named pipe support for the control socket

### Phase 7: Extended Membership

- [ ] **Port mapping** — Map `name → (host, port)` like EPMD, stored in `NodeInfo.metadata`
- [ ] **Node metadata** — Arbitrary key-value metadata per node, queryable via CLI
- [ ] **Health checks** — Configurable application-level health probes per registered service
- [ ] **Event subscriptions** — External processes subscribe to join/leave events via the control socket

## Long Term

### Phase 8: Advanced Discovery

- [ ] **mDNS/DNS-SD plugin** — Zero-config discovery via multicast DNS
- [ ] **Cloud plugins** — AWS EC2 tag-based, Kubernetes endpoint-based, Consul-based discovery
- [ ] **Dynamic plugin loading** — Load discovery plugins at runtime via `libloading` (shared libraries)

### Phase 9: Federation

- [ ] **Multi-cluster** — Federating multiple PMD clusters with cross-cluster membership views
- [ ] **Partition detection** — Detect and report network partitions between cluster segments
- [ ] **Split-brain resolution** — Configurable strategies for handling conflicting state after partitions heal
