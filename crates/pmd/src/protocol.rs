use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use concordat::vv::VersionVector;

// ---------------------------------------------------------------------------
// Wire protocol messages — all variants in one enum, bincode-serialized
// with u32 BE length-prefix framing.
// ---------------------------------------------------------------------------

/// Top-level wire message between PMD instances.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    /// Initiator sends this after TLS handshake.
    Handshake {
        node_id: String,
        listen_addr: SocketAddr,
        nonce: [u8; 32],
        cookie_hmac: Vec<u8>,
    },

    /// Responder replies with its own challenge + version vector.
    HandshakeAck {
        node_id: String,
        listen_addr: SocketAddr,
        nonce: [u8; 32],
        cookie_hmac: Vec<u8>,
        vv: VersionVector,
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
    Join { addr: String },
    Leave { addr: String },
    Shutdown,
}

/// Responses from daemon to CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlResponse {
    Ok,
    Error { message: String },
    Status {
        node_id: String,
        listen_addr: String,
        peer_count: usize,
        node_count: usize,
    },
    Nodes {
        nodes: Vec<NodeInfoResponse>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfoResponse {
    pub node_id: String,
    pub addr: String,
    pub joined_at: u64,
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
    let mut mac = HmacSha256::new_from_slice(cookie)
        .expect("HMAC accepts any key size");
    mac.update(nonce);
    mac.finalize().into_bytes().to_vec()
}

/// Verify an HMAC-SHA256 cookie challenge.
pub fn verify_cookie_hmac(cookie: &[u8], nonce: &[u8; 32], expected: &[u8]) -> bool {
    let mut mac = HmacSha256::new_from_slice(cookie)
        .expect("HMAC accepts any key size");
    mac.update(nonce);
    mac.verify_slice(expected).is_ok()
}
