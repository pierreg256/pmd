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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::MembershipEvent;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn test_add_node_appears_in_members() {
        let mut m = Membership::new("node-a");
        m.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());

        let members = m.members();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].node_id, "node-a");
        assert_eq!(members[0].addr, addr("127.0.0.1:4369"));
    }

    #[test]
    fn test_remove_node_disappears_from_members() {
        let mut m = Membership::new("node-a");
        m.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        assert_eq!(m.members().len(), 1);

        m.remove_node("node-a");
        assert_eq!(m.members().len(), 0);
    }

    #[test]
    fn test_add_multiple_nodes() {
        let mut m = Membership::new("node-a");
        m.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        m.add_node("node-b", addr("127.0.0.1:4370"), HashMap::new());

        let members = m.members();
        assert_eq!(members.len(), 2);
        let ids: Vec<&str> = members.iter().map(|n| n.node_id.as_str()).collect();
        assert!(ids.contains(&"node-a"));
        assert!(ids.contains(&"node-b"));
    }

    #[test]
    fn test_merge_convergence_two_replicas() {
        let mut m1 = Membership::new("node-a");
        let mut m2 = Membership::new("node-b");

        m1.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        m2.add_node("node-b", addr("127.0.0.1:4370"), HashMap::new());

        // Sync m1 → m2
        let delta1 = m1.full_delta();
        m2.merge_remote(&delta1).unwrap();

        // Sync m2 → m1
        let delta2 = m2.full_delta();
        m1.merge_remote(&delta2).unwrap();

        // Both should see 2 nodes
        assert_eq!(m1.members().len(), 2);
        assert_eq!(m2.members().len(), 2);
    }

    #[test]
    fn test_merge_convergence_three_replicas_chain() {
        let mut m1 = Membership::new("node-a");
        let mut m2 = Membership::new("node-b");
        let mut m3 = Membership::new("node-c");

        m1.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        m2.add_node("node-b", addr("127.0.0.1:4370"), HashMap::new());
        m3.add_node("node-c", addr("127.0.0.1:4371"), HashMap::new());

        // Chain: m1 → m2 → m3 → m1
        let d1 = m1.full_delta();
        m2.merge_remote(&d1).unwrap();

        let d2 = m2.full_delta();
        m3.merge_remote(&d2).unwrap();

        let d3 = m3.full_delta();
        m1.merge_remote(&d3).unwrap();

        // Final sync pass so everyone converges
        let d1 = m1.full_delta();
        let d2 = m2.full_delta();
        let d3 = m3.full_delta();
        m1.merge_remote(&d2).unwrap();
        m1.merge_remote(&d3).unwrap();
        m2.merge_remote(&d1).unwrap();
        m2.merge_remote(&d3).unwrap();
        m3.merge_remote(&d1).unwrap();
        m3.merge_remote(&d2).unwrap();

        assert_eq!(m1.members().len(), 3);
        assert_eq!(m2.members().len(), 3);
        assert_eq!(m3.members().len(), 3);
    }

    #[test]
    fn test_merge_remote_detects_join() {
        let mut m1 = Membership::new("node-a");
        let mut m2 = Membership::new("node-b");

        m2.add_node("node-b", addr("127.0.0.1:4370"), HashMap::new());

        let delta = m2.full_delta();
        let changes = m1.merge_remote(&delta).unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].event, MembershipEvent::NodeJoined);
        assert_eq!(changes[0].node_id, "node-b");
    }

    #[test]
    fn test_merge_remote_detects_leave() {
        let mut m1 = Membership::new("node-a");
        let mut m2 = Membership::new("node-b");

        // Both know about node-c
        m1.add_node("node-c", addr("127.0.0.1:4371"), HashMap::new());
        let delta = m1.full_delta();
        m2.merge_remote(&delta).unwrap();

        // m1 removes node-c
        m1.remove_node("node-c");

        // Sync the removal
        let delta = m1.full_delta();
        let changes = m2.merge_remote(&delta).unwrap();

        // node-c should be gone
        let has_leave = changes.iter().any(|c| {
            c.event == MembershipEvent::NodeLeft && c.node_id == "node-c"
        });
        assert!(has_leave, "expected NodeLeft for node-c, got {changes:?}");
    }

    #[test]
    fn test_delta_for_peer_produces_delta() {
        let mut m = Membership::new("node-a");
        m.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());

        let empty_vv = VersionVector::new();
        let delta_bytes = m.delta_for_peer(&empty_vv);
        assert!(!delta_bytes.is_empty());
    }

    #[test]
    fn test_idempotent_merge() {
        let mut m1 = Membership::new("node-a");
        let mut m2 = Membership::new("node-b");

        m1.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        let delta = m1.full_delta();

        // Apply same delta twice
        let changes1 = m2.merge_remote(&delta).unwrap();
        let changes2 = m2.merge_remote(&delta).unwrap();

        // First merge detects join, second should detect nothing new
        assert_eq!(changes1.len(), 1);
        assert_eq!(changes2.len(), 0);
        assert_eq!(m2.members().len(), 1);
    }

    #[test]
    fn test_version_vector_advances() {
        let mut m = Membership::new("node-a");
        let vv_before = m.version_vector().clone();

        m.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        let vv_after = m.version_vector().clone();

        // VV should have advanced
        assert_ne!(format!("{vv_before:?}"), format!("{vv_after:?}"));
    }

    #[test]
    fn test_node_id_accessor() {
        let m = Membership::new("my-node");
        assert_eq!(m.node_id(), "my-node");
    }

    #[test]
    fn test_empty_membership_has_no_members() {
        let m = Membership::new("node-a");
        assert!(m.members().is_empty());
    }
}
