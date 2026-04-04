---
description: "Use when working on protocol messages, wire format, serialization, or the handshake sequence. Covers protocol.rs, framing, and bincode encoding."
applyTo: "**/protocol.rs"
---
# Protocol Conventions

- All protocol messages live in `protocol.rs` — never spread across modules
- Messages are variants of a single `Message` enum with `#[derive(Serialize, Deserialize)]`
- Wire framing: u32 big-endian length prefix + bincode payload
- Handshake uses HMAC-SHA256 challenge/response with a shared cookie
- The `Handshake` initiator generates a random 32-byte nonce
- The responder verifies `HMAC-SHA256(cookie, nonce)` before proceeding
- `MembershipSync` carries opaque `Vec<u8>` from `concordat::codec::encode` — never decode inside protocol.rs
- Add new message variants at the end of the enum for forward compatibility
