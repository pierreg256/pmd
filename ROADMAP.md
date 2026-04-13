# Roadmap

## Current Status

Phases 1–8 are complete. The daemon supports graceful shutdown, TOML config files, service registration/lookup, real-time event subscriptions, gossip protocol with phi accrual failure detection, mesh expansion, Prometheus metrics, and systemd integration.

## Short Term (completed)

### Phase 5: Polish & Robustness ✅

- [x] **Graceful shutdown** — SIGTERM/SIGINT handler, removes self from membership, cleans up files
- [x] **Automatic reconnection** — Exponential backoff reconnection to known peers
- [x] **Structured logging** — `tracing` with `EnvFilter`, respects `RUST_LOG`
- [x] **52 unit tests** — All passing, zero clippy warnings

## Medium Term (completed)

### Phase 6: Production Readiness ✅

- [x] **Configuration file** — TOML config at `~/.pmd/config.toml`
- [x] **Shared CA mode** — `ca_cert_path` config field for production mTLS
- [x] **Node metadata** — Arbitrary key-value metadata from config, replicated via CRDT
- [x] **Metrics** — Prometheus `/metrics` endpoint via `--metrics-port` or `metrics_port` config
- [x] **Systemd integration** — Unit file (`contrib/pmd.service`), `sd-notify` ready/stopping signals
- [ ] **Cross-platform** — Windows named pipe support (future)

### Phase 7: Extended Membership ✅

- [x] **Port mapping** — `pmd register/unregister/lookup` for named services
- [x] **Node metadata** — Queryable via `pmd nodes`, includes services
- [x] **Event subscriptions** — `pmd subscribe` streams join/leave events in real-time
- [ ] **Health checks** — Application-level health probes (future)

## Long Term

### Phase 8: Advanced Discovery

- [ ] **mDNS/DNS-SD plugin** — Zero-config discovery via multicast DNS
- [ ] **Cloud plugins** — Azure Tag-based, AWS EC2 tag-based, Kubernetes endpoint-based, Consul-based discovery
- [ ] **Dynamic plugin loading** — Load discovery plugins at runtime via `libloading` (shared libraries)

### Phase 9: Federation

- [ ] **Multi-cluster** — Federating multiple PMD clusters with cross-cluster membership views
- [ ] **Partition detection** — Detect and report network partitions between cluster segments
- [ ] **Split-brain resolution** — Configurable strategies for handling conflicting state after partitions heal
