---
description: "Use when working on TLS, certificates, mTLS configuration, rustls setup, or cert generation with rcgen."
applyTo: "**/tls.rs"
---
# TLS Conventions

- Use `rustls` + `tokio-rustls` — never `native-tls` or raw OpenSSL
- Self-signed certs generated via `rcgen` on first daemon start, stored in `~/.pmd/tls/`
- Both server and client configs require mTLS (mutual TLS) — `ClientCertVerifier` on server side
- Never disable certificate verification, even in tests — use test CAs instead
- Cert files: `~/.pmd/tls/cert.pem` + `~/.pmd/tls/key.pem`
- Optional shared CA mode: load CA cert from config for production clusters
