---
description: "Add a new protocol message variant to the wire format"
agent: "protocol"
argument-hint: "Describe the new message (name, fields, purpose)"
---
Add a new message variant to the PMD wire protocol.

1. Read `crates/pmd/src/protocol.rs` to see existing messages
2. Add the new variant at the END of the `Message` enum
3. Derive `Serialize, Deserialize, Debug, Clone` on all fields
4. Add a roundtrip serialization test
5. Update framing if needed
6. Run `cargo test -p pmd` to verify
