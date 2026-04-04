---
description: "Review code for security, correctness, and adherence to PMD conventions"
agent: "agent"
argument-hint: "File or module to review"
---
Review the specified code for:

1. **Security**: No hardcoded secrets, proper TLS usage, cookie not logged, no `unwrap()` on network input
2. **Protocol safety**: Framing correctness, length limits to prevent DoS, proper error handling on malformed messages
3. **CRDT correctness**: Single source of truth (no parallel state), proper delta sync, idempotent merge
4. **Conventions**: Import ordering, error handling patterns, doc comments on public items
5. **Tests**: Coverage of happy path, error cases, and edge cases

Output findings as a list with severity (critical/warning/info) and specific file:line references.
