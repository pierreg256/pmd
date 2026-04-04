use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use concordat::codec;
use concordat::doc::CrdtDoc;
use concordat::vv::VersionVector;
use serde_json::json;
use tracing::{debug, info};

use crate::protocol::MembershipEvent;

/// Information about a node in the cluster.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub node_id: String,
    pub addr: SocketAddr,
    #[allow(dead_code)] // used when discovery plugins inspect metadata
    pub metadata: HashMap<String, String>,
    pub joined_at: u64,
}

/// CRDT-backed membership state.
///
/// Wraps a `concordat::CrdtDoc` with the data model:
/// ```text
/// /nodes/<node_id> → { "addr": "ip:port", "metadata": {...}, "joined_at": ts }
/// ```
pub struct Membership {
    doc: CrdtDoc,
    node_id: String,
}

/// A detected membership change.
#[derive(Debug, Clone)]
pub struct MembershipChange {
    pub event: MembershipEvent,
    pub node_id: String,
    pub addr: String,
}

impl Membership {
    /// Create a new membership state for the given node.
    pub fn new(node_id: &str) -> Self {
        Self {
            doc: CrdtDoc::new(node_id),
            node_id: node_id.to_string(),
        }
    }

    /// Register a node (including self) in the CRDT.
    pub fn add_node(&mut self, node_id: &str, addr: SocketAddr, metadata: HashMap<String, String>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let value = json!({
            "addr": addr.to_string(),
            "metadata": metadata,
            "joined_at": now,
        });

        let path = format!("/nodes/{node_id}");
        self.doc.set(&path, value);
        info!(node_id, addr = %addr, "node added to membership");
    }

    /// Remove a node from the CRDT.
    #[allow(dead_code)] // used by graceful shutdown in Phase 5
    pub fn remove_node(&mut self, node_id: &str) {
        let path = format!("/nodes/{node_id}");
        self.doc.remove(&path);
        info!(node_id, "node removed from membership");
    }

    /// Merge a remote delta and return detected membership changes.
    pub fn merge_remote(&mut self, delta_bytes: &[u8]) -> Result<Vec<MembershipChange>> {
        let before = self.member_ids();

        let delta = codec::decode(delta_bytes)
            .map_err(|e| anyhow::anyhow!("failed to decode delta: {e}"))?;
        self.doc.merge_delta(&delta);

        let after = self.member_ids();
        let changes = Self::diff_members(&before, &after, &self.doc);
        for change in &changes {
            debug!(event = ?change.event, node_id = %change.node_id, "membership change detected");
        }
        Ok(changes)
    }

    /// Encode a delta for a peer, given their last-known version vector.
    pub fn delta_for_peer(&self, peer_vv: &VersionVector) -> Vec<u8> {
        let delta = self.doc.delta_since(peer_vv);
        codec::encode(&delta)
    }

    /// Full delta from the beginning (for new peers).
    pub fn full_delta(&self) -> Vec<u8> {
        let empty_vv = VersionVector::new();
        self.delta_for_peer(&empty_vv)
    }

    /// Current version vector.
    pub fn version_vector(&self) -> &VersionVector {
        self.doc.version_vector()
    }

    /// Our node ID.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// List all known members.
    pub fn members(&self) -> Vec<NodeInfo> {
        let state = self.doc.materialize();
        let nodes = match state.get("nodes") {
            Some(serde_json::Value::Object(m)) => m,
            _ => return Vec::new(),
        };

        nodes
            .iter()
            .filter_map(|(id, val)| {
                let addr_str = val.get("addr")?.as_str()?;
                let addr: SocketAddr = addr_str.parse().ok()?;
                let joined_at = val.get("joined_at")?.as_u64().unwrap_or(0);
                let metadata = match val.get("metadata") {
                    Some(serde_json::Value::Object(m)) => m
                        .iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                        .collect(),
                    _ => HashMap::new(),
                };
                Some(NodeInfo {
                    node_id: id.clone(),
                    addr,
                    metadata,
                    joined_at,
                })
            })
            .collect()
    }

    // -- private helpers --

    fn member_ids(&self) -> HashMap<String, String> {
        let state = self.doc.materialize();
        match state.get("nodes") {
            Some(serde_json::Value::Object(m)) => m
                .iter()
                .filter_map(|(id, val)| {
                    let addr = val.get("addr")?.as_str()?.to_string();
                    Some((id.clone(), addr))
                })
                .collect(),
            _ => HashMap::new(),
        }
    }

    fn diff_members(
        before: &HashMap<String, String>,
        after: &HashMap<String, String>,
        _doc: &CrdtDoc,
    ) -> Vec<MembershipChange> {
        let mut changes = Vec::new();

        // Joined: in after but not in before
        for (id, addr) in after {
            if !before.contains_key(id) {
                changes.push(MembershipChange {
                    event: MembershipEvent::NodeJoined,
                    node_id: id.clone(),
                    addr: addr.clone(),
                });
            }
        }

        // Left: in before but not in after
        for (id, addr) in before {
            if !after.contains_key(id) {
                changes.push(MembershipChange {
                    event: MembershipEvent::NodeLeft,
                    node_id: id.clone(),
                    addr: addr.clone(),
                });
            }
        }

        changes
    }
}
