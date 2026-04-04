---
description: "Use when writing or modifying Rust source files. Covers edition 2024 conventions, import ordering, error handling, and naming."
applyTo: "**/*.rs"
---
# Rust Code Conventions

- Edition 2024 — use latest stable features
- Imports: group `std` → external crates → internal modules, separated by blank lines
- Error handling: `anyhow::Result` in binaries, `thiserror` in library crates
- No `unwrap()` except in tests — use `?` or `.expect("reason")`
- Naming: snake_case for modules/functions, CamelCase for types
- Allowed abbreviations: `vv` (VersionVector), `tls`, `id`
- No `unsafe` unless justified with a `// SAFETY:` comment
- Public functions in library crates require doc comments (`///`)
- Use `tracing::{info, warn, error, debug}` — never `println!` or `eprintln!`
