---
description: "Use when writing or modifying tests, test helpers, or test infrastructure."
applyTo: "**/tests/**"
---
# Testing Conventions

- Unit tests in the same file with `#[cfg(test)] mod tests { ... }`
- Integration tests under `tests/` directory at crate level
- Test helpers go in a `tests/common/` module
- Use `#[tokio::test]` for async tests
- Test names: `test_<unit>_<scenario>_<expected>` e.g. `test_handshake_invalid_cookie_rejected`
- Protocol tests: verify roundtrip serialize/deserialize for every message variant
- Membership tests: verify merge convergence with 2+ replicas
- Never use real network ports in unit tests — use `TcpListener::bind("127.0.0.1:0")` for random ports
