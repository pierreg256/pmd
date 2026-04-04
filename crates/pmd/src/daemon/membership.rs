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
    pub metadata: HashMap<String, String>,
    pub joined_at: u64,
    pub services: Vec<ServiceInfo>,
}

/// A service registered on a node.
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    pub name: String,
    pub port: u16,
    pub metadata: HashMap<String, String>,
}

/// CRDT-backed membership state.
///
/// Data model inside the `CrdtDoc`:
/// ```text
/// /nodes/<node_id> → { "addr", "metadata", "joined_at" }
/// /services/<node_id>/<name> → { "port", "metadata" }
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
    pub fn remove_node(&mut self, node_id: &str) {
        let path = format!("/nodes/{node_id}");
        self.doc.remove(&path);
        // Also remove all services for this node
        let services = self.node_services(node_id);
        for svc in services {
            let svc_path = format!("/services/{node_id}/{}", svc.name);
            self.doc.remove(&svc_path);
        }
        info!(node_id, "node removed from membership");
    }

    /// Register a service on this node.
    pub fn register_service(&mut self, name: &str, port: u16, metadata: HashMap<String, String>) {
        let path = format!("/services/{}/{name}", self.node_id);
        let value = json!({
            "port": port,
            "metadata": metadata,
        });
        self.doc.set(&path, value);
        info!(service = name, port, "service registered");
    }

    /// Unregister a service from this node.
    pub fn unregister_service(&mut self, name: &str) {
        let path = format!("/services/{}/{name}", self.node_id);
        self.doc.remove(&path);
        info!(service = name, "service unregistered");
    }

    /// Look up all instances of a service across the cluster.
    pub fn lookup_service(
        &self,
        name: &str,
    ) -> Vec<(String, SocketAddr, u16, HashMap<String, String>)> {
        let state = self.doc.materialize();
        let mut results = Vec::new();

        let services = match state.get("services") {
            Some(serde_json::Value::Object(m)) => m,
            _ => return results,
        };

        let nodes = match state.get("nodes") {
            Some(serde_json::Value::Object(m)) => m,
            _ => return results,
        };

        for (nid, svc_map) in services {
            if let serde_json::Value::Object(svcs) = svc_map
                && let Some(entry) = svcs.get(name)
            {
                let port = entry.get("port").and_then(|p| p.as_u64()).unwrap_or(0) as u16;
                let meta = parse_metadata(entry.get("metadata"));
                if let Some(addr) = nodes
                    .get(nid)
                    .and_then(|nv| nv.get("addr"))
                    .and_then(|a| a.as_str())
                    .and_then(|a| a.parse().ok())
                {
                    results.push((nid.clone(), addr, port, meta));
                }
            }
        }

        results
    }

    /// Get services for a specific node.
    fn node_services(&self, node_id: &str) -> Vec<ServiceInfo> {
        let state = self.doc.materialize();
        let services = match state.get("services") {
            Some(serde_json::Value::Object(m)) => m,
            _ => return Vec::new(),
        };

        match services.get(node_id) {
            Some(serde_json::Value::Object(svcs)) => svcs
                .iter()
                .filter_map(|(name, val)| {
                    let port = val.get("port")?.as_u64()? as u16;
                    let metadata = parse_metadata(val.get("metadata"));
                    Some(ServiceInfo {
                        name: name.clone(),
                        port,
                        metadata,
                    })
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Merge a remote delta and return detected membership changes.
    pub fn merge_remote(&mut self, delta_bytes: &[u8]) -> Result<Vec<MembershipChange>> {
        let before = self.member_ids();

        let delta = codec::decode(delta_bytes)
            .map_err(|e| anyhow::anyhow!("failed to decode delta: {e}"))?;
        self.doc.merge_delta(&delta);

        let after = self.member_ids();
        let changes = Self::diff_members(&before, &after);
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

    /// List all known members with their services.
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
                let metadata = parse_metadata(val.get("metadata"));
                let services = self.node_services(id);
                Some(NodeInfo {
                    node_id: id.clone(),
                    addr,
                    metadata,
                    joined_at,
                    services,
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
    ) -> Vec<MembershipChange> {
        let mut changes = Vec::new();

        for (id, addr) in after {
            if !before.contains_key(id) {
                changes.push(MembershipChange {
                    event: MembershipEvent::NodeJoined,
                    node_id: id.clone(),
                    addr: addr.clone(),
                });
            }
        }

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

fn parse_metadata(val: Option<&serde_json::Value>) -> HashMap<String, String> {
    match val {
        Some(serde_json::Value::Object(m)) => m
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect(),
        _ => HashMap::new(),
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
        assert_eq!(m.members().len(), 2);
    }

    #[test]
    fn test_merge_convergence_two_replicas() {
        let mut m1 = Membership::new("node-a");
        let mut m2 = Membership::new("node-b");
        m1.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        m2.add_node("node-b", addr("127.0.0.1:4370"), HashMap::new());

        let delta1 = m1.full_delta();
        m2.merge_remote(&delta1).unwrap();
        let delta2 = m2.full_delta();
        m1.merge_remote(&delta2).unwrap();

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

        let d1 = m1.full_delta();
        m2.merge_remote(&d1).unwrap();
        let d2 = m2.full_delta();
        m3.merge_remote(&d2).unwrap();
        let d3 = m3.full_delta();
        m1.merge_remote(&d3).unwrap();

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
    }

    #[test]
    fn test_merge_remote_detects_leave() {
        let mut m1 = Membership::new("node-a");
        let mut m2 = Membership::new("node-b");
        m1.add_node("node-c", addr("127.0.0.1:4371"), HashMap::new());
        let delta = m1.full_delta();
        m2.merge_remote(&delta).unwrap();

        m1.remove_node("node-c");
        let delta = m1.full_delta();
        let changes = m2.merge_remote(&delta).unwrap();
        assert!(
            changes
                .iter()
                .any(|c| c.event == MembershipEvent::NodeLeft && c.node_id == "node-c")
        );
    }

    #[test]
    fn test_delta_for_peer_produces_delta() {
        let mut m = Membership::new("node-a");
        m.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        let delta = m.delta_for_peer(&VersionVector::new());
        assert!(!delta.is_empty());
    }

    #[test]
    fn test_idempotent_merge() {
        let mut m1 = Membership::new("node-a");
        let mut m2 = Membership::new("node-b");
        m1.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        let delta = m1.full_delta();
        let c1 = m2.merge_remote(&delta).unwrap();
        let c2 = m2.merge_remote(&delta).unwrap();
        assert_eq!(c1.len(), 1);
        assert_eq!(c2.len(), 0);
    }

    #[test]
    fn test_version_vector_advances() {
        let mut m = Membership::new("node-a");
        let vv_before = m.version_vector().clone();
        m.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        let vv_after = m.version_vector().clone();
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

    // -- Phase 7: Service registration tests --

    #[test]
    fn test_register_service() {
        let mut m = Membership::new("node-a");
        m.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        m.register_service("myapp", 8080, HashMap::new());

        let members = m.members();
        assert_eq!(members[0].services.len(), 1);
        assert_eq!(members[0].services[0].name, "myapp");
        assert_eq!(members[0].services[0].port, 8080);
    }

    #[test]
    fn test_unregister_service() {
        let mut m = Membership::new("node-a");
        m.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        m.register_service("myapp", 8080, HashMap::new());
        assert_eq!(m.members()[0].services.len(), 1);

        m.unregister_service("myapp");
        assert_eq!(m.members()[0].services.len(), 0);
    }

    #[test]
    fn test_lookup_service_across_cluster() {
        let mut m1 = Membership::new("node-a");
        let mut m2 = Membership::new("node-b");
        m1.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        m2.add_node("node-b", addr("127.0.0.1:4370"), HashMap::new());
        m1.register_service("web", 8080, HashMap::new());
        m2.register_service("web", 9090, HashMap::new());

        // Sync
        let d1 = m1.full_delta();
        let d2 = m2.full_delta();
        m1.merge_remote(&d2).unwrap();
        m2.merge_remote(&d1).unwrap();

        let results = m1.lookup_service("web");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_service_syncs_between_replicas() {
        let mut m1 = Membership::new("node-a");
        let mut m2 = Membership::new("node-b");
        m1.add_node("node-a", addr("127.0.0.1:4369"), HashMap::new());
        m1.register_service(
            "api",
            3000,
            HashMap::from([("version".into(), "2.0".into())]),
        );

        let delta = m1.full_delta();
        m2.merge_remote(&delta).unwrap();

        let results = m2.lookup_service("api");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].2, 3000);
        assert_eq!(results[0].3.get("version").unwrap(), "2.0");
    }

    #[test]
    fn test_node_metadata() {
        let mut m = Membership::new("node-a");
        let meta = HashMap::from([("role".into(), "worker".into())]);
        m.add_node("node-a", addr("127.0.0.1:4369"), meta);

        let members = m.members();
        assert_eq!(members[0].metadata.get("role").unwrap(), "worker");
    }
}
