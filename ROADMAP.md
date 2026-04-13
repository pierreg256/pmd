# Roadmap

## Current Status

Phases 1–7 are complete. The daemon supports graceful shutdown, automatic reconnection, TOML config files, service registration/lookup, and real-time event subscriptions.

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
- [ ] **Metrics** — Prometheus-compatible metrics (future)
- [ ] **Systemd integration** — Unit file, socket activation (future)
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
