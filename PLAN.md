# Plan: PMD — Port Mapper Daemon (Erlang-style, in Rust)

## TL;DR

Créer un daemon réseau en Rust inspiré d'EPMD d'Erlang, qui maintient un registre distribué des machines connectées au cluster via un état CRDT convergent (crate `concordat`, fourni par l'utilisateur). Un seul binaire `pmd` sert à la fois de daemon et de CLI. Les connexions inter-PMD sont chiffrées via TLS (rustls). Un système de plugins trait-based permet la découverte automatique, avec un plugin broadcast UDP comme exemple.

## Architecture

### Composants principaux
- **Daemon** : écoute sur TCP (port configurable, défault 4369), gère les connexions inter-PMD chiffrées TLS
- **CLI** : sous-commandes du même binaire, communique avec le daemon local via Unix domain socket
- **Membership** : état CRDT (via `concordat`) convergent entre tous les nœuds
- **Plugin Discovery** : trait `DiscoveryPlugin` dans un crate séparé, compilé statiquement
- **Broadcast Plugin** : exemple de plugin utilisant UDP broadcast (port 4370)

### Stack technique
- **Runtime** : Tokio
- **CLI** : clap (derive)
- **Sérialisation** : serde + bincode (protocole inter-PMD), serde_json (contrôle local)
- **TLS** : rustls + tokio-rustls + rcgen (génération de certificats auto-signés)
- **Daemon** : daemonize ou fork manuel + PID file
- **CRDT** : concordat (crate privé/local, fourni par l'utilisateur)
- **Logging** : tracing + tracing-subscriber (journald/fichier)

## Structure du projet

```
pmd/
├── Cargo.toml                    # workspace root
├── crates/
│   ├── pmd/                      # binaire principal (daemon + CLI)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs           # point d'entrée, dispatch CLI vs daemon
│   │       ├── cli.rs            # définition clap des commandes
│   │       ├── config.rs         # configuration (port, TLS, plugins, etc.)
│   │       ├── daemon/
│   │       │   ├── mod.rs        # orchestration du daemon
│   │       │   ├── server.rs     # TCP listener TLS, accepte les peers
│   │       │   ├── peer.rs       # gestion d'une connexion peer (read/write loop)
│   │       │   ├── control.rs    # Unix socket pour CLI→daemon
│   │       │   └── membership.rs # état CRDT, merge, notifications join/leave
│   │       ├── protocol.rs       # messages wire format (serde + bincode)
│   │       └── tls.rs            # setup TLS, chargement/génération certs
│   │
│   ├── pmd-discovery/            # crate trait plugin discovery
│   │   ├── Cargo.toml
│   │   └── src/lib.rs            # trait DiscoveryPlugin + types
│   │
│   └── pmd-broadcast/            # plugin exemple : broadcast UDP
│       ├── Cargo.toml
│       └── src/lib.rs            # implémentation UDP broadcast/listen
```

## Steps

### Phase 1 : Fondations (CLI + Daemon skeleton) ✅

1. ✅ **Initialiser le workspace Cargo** — Workspace avec 3 crates (`pmd`, `pmd-discovery`, `pmd-broadcast`). `concordat` via crates.io.
2. ✅ **Définir la CLI** (`cli.rs`) — Sous-commandes clap : `start`, `stop`, `status`, `join`, `leave`, `nodes`. Options `--port`, `--bind`, `--foreground`.
3. ✅ **Configuration** (`config.rs`) — Struct `Config` : port, bind, paths (~/.pmd/), intervals. Tests: defaults, custom port, paths, ensure_dirs.
4. ✅ **Daemonization** (`daemon/mod.rs`) — `daemonize` crate + PID file + `--foreground` mode. Cookie load/generate. Orchestration des tâches.
5. ✅ **Control socket** (`daemon/control.rs`) — Unix domain socket, JSON request/response. Dispatch: Status, Nodes, Join, Leave, Shutdown. Tests: 5 tests e2e via socket.

### Phase 2 : Protocole & Transport TLS ✅

6. ✅ **Protocole wire** (`protocol.rs`) — `Message` enum (Handshake, HandshakeAck, MembershipSync, Notification, Heartbeat, HeartbeatAck). Framing u32 BE + bincode. Cookie HMAC-SHA256. Tests: 16 tests (roundtrip serde, frame encode/decode/EOF/oversized, HMAC valid/invalid/empty/tampered, control JSON roundtrip).
7. ✅ **TLS setup** (`tls.rs`) — Self-signed cert via `rcgen`, stocké dans `~/.pmd/tls/`. `build_acceptor()` / `build_connector()` avec rustls. Tests: 5 tests (cert gen, acceptor, connector, TLS handshake, key permissions 0600).
8. ✅ **Serveur TCP TLS** (`daemon/server.rs`) — Accept loop avec `TlsAcceptor`, spawn per-peer tasks, graceful shutdown via `CancellationToken`.

### Phase 3 : Membership CRDT & Convergence ✅

9. ✅ **Membership state** (`daemon/membership.rs`) — `Membership` wrapping `CrdtDoc`. Modèle `/nodes/<node_id>`. add_node, remove_node, merge_remote (avec diff detection), delta_for_peer, full_delta, members(), version_vector(). Tests: 12 tests (add/remove, multi-nodes, convergence 2 & 3 replicas, join/leave detection, delta encoding, idempotent merge, VV advance, empty state).
10. ✅ **Peer management** (`daemon/peer.rs`) — Inbound/outbound handshake avec cookie HMAC verification. Message loop via `tokio::select!` (heartbeat + sync + read). Per-peer `VersionVector` tracking.
11. ✅ **Notifications** — `MembershipChange` events détectés par diff de `materialize()` avant/après `merge_delta()`. Log via tracing.

### Phase 4 : Système de Plugins Discovery ✅

12. ✅ **Trait DiscoveryPlugin** (`pmd-discovery/src/lib.rs`) — Trait `DiscoveryPlugin` avec `name()`, `start(ctx)`, `stop()`. `DiscoveryContext` avec `local_node: NodeInfo` et `discovered_tx: Sender<SocketAddr>`. Implémenté avec `async fn` natif.
13. ✅ **Plugin Broadcast UDP** (`pmd-broadcast/src/plugin.rs`) — `BroadcastPlugin` avec beacon JSON, configurable port/interval, `tokio::select!` pour send/recv simultané. Ignore ses propres beacons.

### Phase 5 : Polish & Robustesse

14. **Graceful shutdown** — Sur SIGTERM/SIGINT : notifier les peers du départ, fermer les connexions TLS, supprimer le PID file
15. **Reconnexion automatique** — Si un peer connu se déconnecte, tenter de se reconnecter avec backoff exponentiel
16. **Logging structuré** — `tracing` avec subscriber journald (daemon) ou stdout (foreground)
17. ✅ **Tests unitaires** — 43 tests pass, 0 échec, 0 warning clippy :
    - Protocol (16): roundtrip serde (6 variantes), frame encode/decode/EOF/oversized, HMAC (5 scénarios), control JSON roundtrip
    - Membership (12): add/remove, multi-nodes, convergence 2 & 3 replicas, join/leave detection, delta/idempotent merge, VV advance
    - Config (4): defaults, custom port, paths, ensure_dirs
    - TLS (5): cert gen, acceptor, connector, handshake, permissions
    - Control (5): status, nodes, join, leave, shutdown via Unix socket
    - **Integration tests** : à faire (2 instances connectées, handshake invalid cookie, heartbeat, broadcast discovery)

## Dépendances clés (Cargo.toml)

| Crate | Usage |
|-------|-------|
| `tokio` (full) | Runtime async |
| `clap` (derive) | CLI |
| `serde` + `bincode` | Protocole wire inter-PMD |
| `serde` + `serde_json` | Protocole contrôle CLI↔daemon + concordat interop |
| `rustls` + `tokio-rustls` | TLS |
| `rcgen` | Génération certificats auto-signés |
| `concordat` (crates.io) | Delta-state CRDT JSON pour membership |
| `tracing` + `tracing-subscriber` | Logging |
| `daemonize` | Daemonization (fork + PID) |
| `uuid` | Génération node_id |
| `hmac` + `sha2` | Cookie auth dans le handshake |
| `rand` | Nonce génération pour cookie challenge |

## Verification

**Hard gate: nothing is pushed to GitHub unless ALL of the following pass:**

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Additional manual verification:
1. `pmd start --foreground` → `pmd status` → vérifier réponse JSON
2. Test 2 instances : `pmd start --port 4369` puis `pmd start --port 4370`, `pmd join 127.0.0.1:4369` depuis la 2e → vérifier que `pmd nodes` montre les 2 nœuds des deux côtés
3. Test broadcast : activer le plugin broadcast sur 2 instances → vérifier découverte automatique
4. Test TLS : vérifier avec `openssl s_client` que le port PMD parle bien TLS
5. Test graceful shutdown : `pmd stop` → vérifier que l'autre instance détecte le départ

## Decisions

- **Binaire unique** `pmd` : la sous-commande détermine le mode (daemon vs client CLI)
- **concordat** (crates.io) — delta-state CRDT JSON
- **Cookie partagé** — Fichier `~/.pmd/cookie`. HMAC du cookie sur un nonce vérifié dans le Handshake
- **Pas de persistance disque** — Un nœud qui redémarre rejoint le cluster vierge et reçoit l'état via delta sync
- **Plugins compilés statiquement** V1. Architecture trait-based compatible dynamic loading plus tard
- **mTLS auto-signé** par défaut. Mode CA partagé optionnel
- **Anti-entropy delta-state** — utilise `delta_since(peer_vv)` de concordat, pas de full-state
- **Scope V1** : membership uniquement, pas de port mapping. Extensible via metadata

## Intégration concordat (crates.io)

Le crate expose une API delta-state CRDT JSON :
- `CrdtDoc::new(replica_id: &str)` — un doc par nœud, `replica_id` = PMD `node_id` (UUID)
- `doc.set(path, value)` / `doc.remove(path)` — mutations
- `doc.delta_since(&vv) → Delta` — delta depuis un version vector
- `doc.merge_delta(&delta)` — merge commutatif, idempotent
- `concordat::codec::encode(&delta) → Vec<u8>` / `decode(&bytes) → Result<Delta>` — sérialisation
- `doc.version_vector() → &VersionVector` — contexte causal
- `doc.materialize() → serde_json::Value` — snapshot JSON

**Modèle membership** :
```
/nodes/<node_id> → { "addr": "192.168.1.10:4369", "metadata": {...}, "joined_at": 1712345678 }
```
- **Join** : `doc.set("/nodes/<node_id>", json!({...}))`
- **Leave** : `doc.remove("/nodes/<node_id>")`
- **Sync** : chaque peer stocke le `VersionVector` du dernier sync. Envoi `delta_since(peer_last_vv)` — efficace
- **Détection join/leave** : diff de `doc.materialize()["/nodes"]` avant/après merge_delta

**Protocole sync** :
- Anti-entropy (5s) : `MembershipSync { delta: encode(doc.delta_since(peer_vv)), vv: vv.clone() }`
- Receveur : `merge_delta(decode(delta))`, met à jour `peer_last_vv`
- Sur changement local : push immédiat à tous les peers
