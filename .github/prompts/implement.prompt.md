---
description: "Implement a new feature end-to-end following the PLAN.md phases"
agent: "agent"
argument-hint: "Describe the feature to implement"
---
Implement the requested feature following the PMD project plan.

1. Read [PLAN.md](../../PLAN.md) to understand the architecture and conventions
2. Identify which phase and step the feature belongs to
3. Check existing code to understand current state
4. Implement the feature following the project conventions
5. Add tests (unit + integration as applicable)
6. Run `cargo build --workspace && cargo clippy --workspace -- -D warnings && cargo test --workspace`
7. Fix any compilation or lint errors before finishing
