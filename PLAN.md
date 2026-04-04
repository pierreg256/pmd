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

### Phase 1 : Fondations (CLI + Daemon skeleton)

1. **Initialiser le workspace Cargo** — Créer `Cargo.toml` workspace + les 3 crates (`pmd`, `pmd-discovery`, `pmd-broadcast`)
2. **Définir la CLI** (`cli.rs`) — Sous-commandes clap :
   - `pmd start [--port 4369] [--config path] [--foreground]` — lance le daemon
   - `pmd stop` — arrête le daemon
   - `pmd status` — état du daemon + liste des nœuds
   - `pmd join <addr:port>` — connexion manuelle à un peer
   - `pmd leave <addr:port>` — déconnexion d'un peer  
   - `pmd nodes` — liste les nœuds connus
3. **Configuration** (`config.rs`) — Struct serde pour : port d'écoute, bind address, chemin PID file, chemin socket contrôle, chemins cert/key TLS, liste des plugins activés
4. **Daemonization** (`daemon/mod.rs`) — Fork + PID file + redirect stdout/stderr vers log, ou mode `--foreground` pour le dev
5. **Control socket** (`daemon/control.rs`) — Unix domain socket, le daemon écoute les commandes CLI encodées en JSON, répond avec le résultat

### Phase 2 : Protocole & Transport TLS

6. **Protocole wire** (`protocol.rs`) — Définir les messages serde :
   - `Handshake { node_id, listen_port, capabilities, nonce: [u8;32], cookie_hmac: [u8;32] }` — le nonce est généré par l'initiateur, le hmac = HMAC-SHA256(cookie, nonce)
   - `HandshakeAck { node_id, nonce, cookie_hmac, members_vv: VersionVector }` — le receveur répond avec son propre nonce+hmac + son version vector
   - `MembershipSync { delta_bytes: Vec<u8>, sender_vv: VersionVector }` — delta encodé via `concordat::codec::encode`
   - `Notification { event: NodeJoined|NodeLeft, node_id: String, addr: String }`
   - `Heartbeat` / `HeartbeatAck`
   - Framing : length-prefix (u32 BE) + bincode payload
7. **TLS setup** (`tls.rs`) — 
   - Au premier lancement : générer un certificat auto-signé par nœud via `rcgen`
   - Stocker cert + clé dans `~/.pmd/tls/`
   - Mode "shared CA" optionnel : l'admin distribue un CA, chaque nœud a un cert signé par ce CA
   - `rustls::ServerConfig` + `rustls::ClientConfig` avec vérification mutuelle (mTLS)
8. **Serveur TCP TLS** (`daemon/server.rs`) — 
   - `TcpListener::bind` sur le port configuré
   - Chaque connexion : TLS handshake → lire `Handshake` → répondre `HandshakeAck` → boucle de messages
   - Timeout + heartbeat pour détecter les déconnexions

### Phase 3 : Membership CRDT & Convergence

9. **Membership state** (`daemon/membership.rs`) — 
   - Struct `Membership` wrapping `CrdtDoc` (concordat). `replica_id` = node UUID
   - Modèle : `/nodes/<node_id>` → `{ "addr", "metadata", "joined_at" }`
   - `add_node(node_id, addr, metadata)` → `doc.set("/nodes/<id>", json!({...}))`
   - `remove_node(node_id)` → `doc.remove("/nodes/<id>")`
   - `merge_remote(delta_bytes)` → `doc.merge_delta(codec::decode(bytes)?)`, retourne `Vec<MembershipEvent>` (join/leave diff)
   - `delta_for_peer(peer_vv) → Vec<u8>` → `codec::encode(doc.delta_since(peer_vv))`
   - `members() → Vec<NodeInfo>` → matérialise et parse `/nodes/*`
   - `version_vector()` → expose le VV courant
10. **Peer management** (`daemon/peer.rs`) — 
    - Chaque peer = tâche Tokio avec read/write loop
    - À la connexion : échange `Handshake`/`HandshakeAck` avec vérification cookie HMAC
    - Stocke le `VersionVector` du peer (initialisé à `vv` reçu dans le HandshakeAck)
    - Anti-entropy périodique (5s) : envoie `MembershipSync { delta_bytes: membership.delta_for_peer(peer_vv), sender_vv }`
    - Sur réception d'un `MembershipSync` : `membership.merge_remote(delta_bytes)`, met à jour `peer_vv = sender_vv`
    - Sur changement local → push immédiat du delta aux peers
11. **Notifications** — Quand l'état CRDT diverge après un merge :
    - Calculer le diff (nouveaux nœuds = joined, nœuds disparus = left)
    - Envoyer `Notification` à tous les peers connectés
    - Logger l'événement

### Phase 4 : Système de Plugins Discovery

12. **Trait DiscoveryPlugin** (`pmd-discovery/src/lib.rs`) —
    ```rust
    trait DiscoveryPlugin: Send + Sync {
        fn name(&self) -> &str;
        fn start(&self, ctx: DiscoveryContext) -> BoxFuture<Result<()>>;
        fn stop(&self) -> BoxFuture<Result<()>>;
    }
    
    struct DiscoveryContext {
        local_node: NodeInfo,
        on_discovered: Sender<SocketAddr>,  // channel pour signaler un peer découvert
    }
    ```
    - Les plugins compilés statiquement sont enregistrés au démarrage du daemon
    - Le daemon écoute le channel `on_discovered` et initie une connexion vers les peers découverts
13. **Plugin Broadcast UDP** (`pmd-broadcast/src/lib.rs`) —
    - Envoie un beacon UDP broadcast (port 4370) à intervalle régulier (ex: 10s) contenant `{ node_id, listen_addr, listen_port }`
    - Écoute les beacons des autres instances PMD
    - Quand un beacon inconnu est reçu → envoie l'adresse via `on_discovered`
    - Configurable : intervalle, port broadcast, interfaces

### Phase 5 : Polish & Robustesse

14. **Graceful shutdown** — Sur SIGTERM/SIGINT : notifier les peers du départ, fermer les connexions TLS, supprimer le PID file
15. **Reconnexion automatique** — Si un peer connu se déconnecte, tenter de se reconnecter avec backoff exponentiel
16. **Logging structuré** — `tracing` avec subscriber journald (daemon) ou stdout (foreground)
17. **Tests** — Tests are a hard gate: **nothing is pushed until all tests pass**.
    - `cargo test --workspace && cargo clippy --workspace -- -D warnings` must pass before every push
    - **Unit tests** (in each module):
      - Protocol: roundtrip serde for every `Message` variant, frame encode/decode, oversized frame rejection, HMAC valid/invalid/empty
      - Membership: add/remove node, merge convergence (2 replicas, 3+ replicas), join/leave detection, delta encoding, idempotent merge
      - Config: default values, directory creation
      - TLS: cert generation, acceptor/connector construction
      - Control: each request/response variant, malformed JSON handling
    - **Integration tests** (`crates/pmd/tests/`):
      - Two PMD instances connect over TLS, handshake with valid cookie, exchange membership, verify convergence
      - Handshake with invalid cookie → rejected
      - Heartbeat keeps connection alive, timeout detects dead peer
      - Graceful shutdown notifies peers
      - Broadcast plugin discovers peers on loopback
    - Always use `127.0.0.1:0` for random ports, `tokio::time::timeout(10s)` to prevent hangs
    - Every bug fix must include a regression test

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
