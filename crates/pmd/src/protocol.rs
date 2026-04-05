use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use concordat::vv::VersionVector;

// ---------------------------------------------------------------------------
// Wire protocol messages — all variants in one enum, bincode-serialized
// with u32 BE length-prefix framing.
// ---------------------------------------------------------------------------

/// Current wire protocol version. Bump when the `Message` enum changes in a
/// backwards-incompatible way.
pub const PROTOCOL_VERSION: u32 = 1;

/// Top-level wire message between PMD instances.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    /// Initiator sends this after TLS handshake.
    Handshake {
        node_id: String,
        listen_addr: SocketAddr,
        nonce: [u8; 32],
        cookie_hmac: Vec<u8>,
        /// Semantic version string of the sending node (e.g. "0.1.0").
        version: String,
        /// Wire protocol version — must match for communication.
        protocol_version: u32,
    },

    /// Responder replies with its own challenge + version vector.
    HandshakeAck {
        node_id: String,
        listen_addr: SocketAddr,
        nonce: [u8; 32],
        cookie_hmac: Vec<u8>,
        vv: VersionVector,
        /// Semantic version string of the responding node.
        version: String,
        /// Wire protocol version — must match for communication.
        protocol_version: u32,
    },

    /// Delta-state CRDT membership sync.
    MembershipSync {
        delta_bytes: Vec<u8>,
        sender_vv: VersionVector,
    },

    /// Membership change notification.
    Notification {
        event: MembershipEvent,
        node_id: String,
        addr: String,
    },

    /// Periodic liveness check.
    Heartbeat,

    /// Reply to a heartbeat.
    HeartbeatAck,
}

/// Membership event types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MembershipEvent {
    NodeJoined,
    NodeLeft,
}

// ---------------------------------------------------------------------------
// Control socket protocol (CLI ↔ daemon), JSON-encoded.
// ---------------------------------------------------------------------------

/// Commands sent from CLI to daemon over the Unix socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlRequest {
    Status,
    Nodes,
    Join {
        addr: String,
    },
    Leave {
        addr: String,
    },
    Shutdown,
    /// Register a named service on this node (Phase 7: port mapping).
    Register {
        name: String,
        port: u16,
        metadata: std::collections::HashMap<String, String>,
    },
    /// Unregister a named service.
    Unregister {
        name: String,
    },
    /// Look up a registered service by name across the cluster.
    Lookup {
        name: String,
    },
    /// Subscribe to membership events (long-lived connection).
    Subscribe,
}

/// Responses from daemon to CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlResponse {
    Ok,
    Error {
        message: String,
    },
    Status {
        node_id: String,
        listen_addr: String,
        peer_count: usize,
        node_count: usize,
    },
    Nodes {
        nodes: Vec<NodeInfoResponse>,
    },
    /// Response to Lookup: list of endpoints for a service name.
    Services {
        entries: Vec<ServiceEntry>,
    },
    /// Membership event pushed to subscribers.
    Event {
        event: MembershipEvent,
        node_id: String,
        addr: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfoResponse {
    pub node_id: String,
    pub addr: String,
    pub joined_at: u64,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub services: Vec<ServiceEntry>,
}

/// A registered service entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEntry {
    pub name: String,
    pub node_id: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Framing helpers: u32 BE length-prefix + bincode
// ---------------------------------------------------------------------------

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Write a length-prefixed bincode frame.
pub async fn write_frame<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &Message,
) -> anyhow::Result<()> {
    let payload = bincode::serialize(msg)?;
    let len = u32::try_from(payload.len())
        .map_err(|_| anyhow::anyhow!("message too large: {} bytes", payload.len()))?;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a length-prefixed bincode frame. Returns `None` on clean EOF.
pub async fn read_frame<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> anyhow::Result<Option<Message>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        anyhow::bail!("frame too large: {len} bytes (max 16 MiB)");
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    let msg = bincode::deserialize(&payload)?;
    Ok(Some(msg))
}

// ---------------------------------------------------------------------------
// Cookie HMAC helpers
// ---------------------------------------------------------------------------

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Compute HMAC-SHA256 of the nonce using the cookie as key.
pub fn compute_cookie_hmac(cookie: &[u8], nonce: &[u8; 32]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(cookie).expect("HMAC accepts any key size");
    mac.update(nonce);
    mac.finalize().into_bytes().to_vec()
}

/// Verify an HMAC-SHA256 cookie challenge.
pub fn verify_cookie_hmac(cookie: &[u8], nonce: &[u8; 32], expected: &[u8]) -> bool {
    let mut mac = HmacSha256::new_from_slice(cookie).expect("HMAC accepts any key size");
    mac.update(nonce);
    mac.verify_slice(expected).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    use concordat::vv::VersionVector;

    // -----------------------------------------------------------------------
    // Roundtrip serialize/deserialize for every Message variant
    // -----------------------------------------------------------------------

    fn roundtrip(msg: &Message) -> Message {
        let bytes = bincode::serialize(msg).expect("serialize");
        bincode::deserialize(&bytes).expect("deserialize")
    }

    #[test]
    fn test_message_roundtrip_handshake() {
        let msg = Message::Handshake {
            node_id: "node-1".into(),
            listen_addr: "127.0.0.1:4369".parse().unwrap(),
            nonce: [42u8; 32],
            cookie_hmac: vec![1, 2, 3],
            version: "0.1.0".into(),
            protocol_version: PROTOCOL_VERSION,
        };
        let decoded = roundtrip(&msg);
        match decoded {
            Message::Handshake {
                node_id,
                listen_addr,
                nonce,
                cookie_hmac,
                version,
                protocol_version,
            } => {
                assert_eq!(node_id, "node-1");
                assert_eq!(listen_addr, "127.0.0.1:4369".parse::<SocketAddr>().unwrap());
                assert_eq!(nonce, [42u8; 32]);
                assert_eq!(cookie_hmac, vec![1, 2, 3]);
                assert_eq!(version, "0.1.0");
                assert_eq!(protocol_version, PROTOCOL_VERSION);
            }
            other => panic!("expected Handshake, got {other:?}"),
        }
    }

    #[test]
    fn test_message_roundtrip_handshake_ack() {
        let msg = Message::HandshakeAck {
            node_id: "node-2".into(),
            listen_addr: "10.0.0.1:4369".parse().unwrap(),
            nonce: [7u8; 32],
            cookie_hmac: vec![9, 8, 7],
            vv: VersionVector::new(),
            version: "0.1.0".into(),
            protocol_version: PROTOCOL_VERSION,
        };
        let decoded = roundtrip(&msg);
        match decoded {
            Message::HandshakeAck {
                node_id,
                nonce,
                vv,
                version,
                protocol_version,
                ..
            } => {
                assert_eq!(node_id, "node-2");
                assert_eq!(nonce, [7u8; 32]);
                assert!(vv.is_empty());
                assert_eq!(version, "0.1.0");
                assert_eq!(protocol_version, PROTOCOL_VERSION);
            }
            other => panic!("expected HandshakeAck, got {other:?}"),
        }
    }

    #[test]
    fn test_message_roundtrip_membership_sync() {
        let msg = Message::MembershipSync {
            delta_bytes: vec![0xDE, 0xAD],
            sender_vv: VersionVector::new(),
        };
        let decoded = roundtrip(&msg);
        match decoded {
            Message::MembershipSync { delta_bytes, .. } => {
                assert_eq!(delta_bytes, vec![0xDE, 0xAD]);
            }
            other => panic!("expected MembershipSync, got {other:?}"),
        }
    }

    #[test]
    fn test_message_roundtrip_notification() {
        let msg = Message::Notification {
            event: MembershipEvent::NodeJoined,
            node_id: "n1".into(),
            addr: "1.2.3.4:5".into(),
        };
        let decoded = roundtrip(&msg);
        match decoded {
            Message::Notification {
                event,
                node_id,
                addr,
            } => {
                assert_eq!(event, MembershipEvent::NodeJoined);
                assert_eq!(node_id, "n1");
                assert_eq!(addr, "1.2.3.4:5");
            }
            other => panic!("expected Notification, got {other:?}"),
        }

        // Also test NodeLeft
        let msg2 = Message::Notification {
            event: MembershipEvent::NodeLeft,
            node_id: "n2".into(),
            addr: "5.6.7.8:9".into(),
        };
        match roundtrip(&msg2) {
            Message::Notification { event, .. } => assert_eq!(event, MembershipEvent::NodeLeft),
            other => panic!("expected Notification, got {other:?}"),
        }
    }

    #[test]
    fn test_message_roundtrip_heartbeat() {
        match roundtrip(&Message::Heartbeat) {
            Message::Heartbeat => {}
            other => panic!("expected Heartbeat, got {other:?}"),
        }
    }

    #[test]
    fn test_message_roundtrip_heartbeat_ack() {
        match roundtrip(&Message::HeartbeatAck) {
            Message::HeartbeatAck => {}
            other => panic!("expected HeartbeatAck, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Frame encode/decode
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_frame_roundtrip() {
        let msg = Message::Heartbeat;
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let decoded = read_frame(&mut cursor).await.unwrap();
        assert!(matches!(decoded, Some(Message::Heartbeat)));
    }

    #[tokio::test]
    async fn test_frame_roundtrip_complex_message() {
        let msg = Message::Handshake {
            node_id: "test-node".into(),
            listen_addr: "127.0.0.1:4369".parse().unwrap(),
            nonce: [99u8; 32],
            cookie_hmac: vec![1, 2, 3, 4, 5],
            version: "0.1.0".into(),
            protocol_version: PROTOCOL_VERSION,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let decoded = read_frame(&mut cursor).await.unwrap().unwrap();
        match decoded {
            Message::Handshake { node_id, .. } => assert_eq!(node_id, "test-node"),
            other => panic!("expected Handshake, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_frame_eof_returns_none() {
        let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
        let result = read_frame(&mut cursor).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_frame_oversized_rejected() {
        // Craft a length header claiming 20 MiB
        let len: u32 = 20 * 1024 * 1024;
        let mut buf = Vec::new();
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&[0u8; 64]); // some payload bytes

        let mut cursor = std::io::Cursor::new(buf);
        let result = read_frame(&mut cursor).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("frame too large"));
    }

    // -----------------------------------------------------------------------
    // Cookie HMAC
    // -----------------------------------------------------------------------

    #[test]
    fn test_hmac_valid_cookie() {
        let cookie = b"my-secret-cookie";
        let nonce = [1u8; 32];
        let hmac = compute_cookie_hmac(cookie, &nonce);
        assert!(verify_cookie_hmac(cookie, &nonce, &hmac));
    }

    #[test]
    fn test_hmac_invalid_cookie() {
        let cookie = b"my-secret-cookie";
        let wrong_cookie = b"wrong-cookie";
        let nonce = [2u8; 32];
        let hmac = compute_cookie_hmac(cookie, &nonce);
        assert!(!verify_cookie_hmac(wrong_cookie, &nonce, &hmac));
    }

    #[test]
    fn test_hmac_wrong_nonce() {
        let cookie = b"my-secret-cookie";
        let nonce = [3u8; 32];
        let other_nonce = [4u8; 32];
        let hmac = compute_cookie_hmac(cookie, &nonce);
        assert!(!verify_cookie_hmac(cookie, &other_nonce, &hmac));
    }

    #[test]
    fn test_hmac_empty_cookie() {
        let cookie = b"";
        let nonce = [5u8; 32];
        let hmac = compute_cookie_hmac(cookie, &nonce);
        // Empty cookie still produces a valid HMAC — but wrong cookie won't match
        assert!(verify_cookie_hmac(cookie, &nonce, &hmac));
        assert!(!verify_cookie_hmac(b"other", &nonce, &hmac));
    }

    #[test]
    fn test_hmac_tampered_hmac() {
        let cookie = b"my-secret-cookie";
        let nonce = [6u8; 32];
        let mut hmac = compute_cookie_hmac(cookie, &nonce);
        // Flip a bit
        hmac[0] ^= 0xFF;
        assert!(!verify_cookie_hmac(cookie, &nonce, &hmac));
    }

    // -----------------------------------------------------------------------
    // Control protocol JSON roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn test_control_request_json_roundtrip() {
        let requests = vec![
            ControlRequest::Status,
            ControlRequest::Nodes,
            ControlRequest::Join {
                addr: "1.2.3.4:5".into(),
            },
            ControlRequest::Leave {
                addr: "5.6.7.8:9".into(),
            },
            ControlRequest::Shutdown,
        ];
        for req in &requests {
            let json = serde_json::to_string(req).unwrap();
            let decoded: ControlRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(format!("{decoded:?}"), format!("{req:?}"));
        }
    }

    #[test]
    fn test_control_response_json_roundtrip() {
        let responses = vec![
            ControlResponse::Ok,
            ControlResponse::Error {
                message: "something failed".into(),
            },
            ControlResponse::Status {
                node_id: "n1".into(),
                listen_addr: "0.0.0.0:4369".into(),
                peer_count: 2,
                node_count: 3,
            },
            ControlResponse::Nodes {
                nodes: vec![NodeInfoResponse {
                    node_id: "n1".into(),
                    addr: "1.2.3.4:4369".into(),
                    joined_at: 123456,
                    metadata: std::collections::HashMap::new(),
                    services: vec![],
                }],
            },
            ControlResponse::Services {
                entries: vec![ServiceEntry {
                    name: "web".into(),
                    node_id: "n1".into(),
                    host: "1.2.3.4".into(),
                    port: 8080,
                    metadata: std::collections::HashMap::new(),
                }],
            },
            ControlResponse::Event {
                event: MembershipEvent::NodeJoined,
                node_id: "n1".into(),
                addr: "1.2.3.4:4369".into(),
            },
        ];
        for resp in &responses {
            let json = serde_json::to_string(resp).unwrap();
            let decoded: ControlResponse = serde_json::from_str(&json).unwrap();
            assert_eq!(format!("{decoded:?}"), format!("{resp:?}"));
        }
    }
}
