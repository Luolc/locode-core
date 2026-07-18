# Architecture Decision Records

Sequentially numbered records of significant, hard-to-reverse decisions for `locode-core`.
Don't delete superseded ADRs — write a new one that references and supersedes the old.

See [`../../SPEC.md`](../../SPEC.md) for the overall specification. Rationale and source study
live in `~/dev/coding-cli-survey`.

| ADR | Decision | Status |
|---|---|---|
| [0001](ADR-0001-headless-core-scope.md) | Headless-only core library; no TUI/interactive prompts | Accepted |
| [0002](ADR-0002-multi-crate-workspace.md) | Multi-crate `locode-*` Cargo workspace | Accepted |
| [0003](ADR-0003-typed-tool-contract.md) | Typed `Tool` contract, derived schemas, dual `output`/`prompt_text` | Accepted |
| [0004](ADR-0004-error-taxonomy-and-pairing.md) | Soft/fatal error taxonomy + strict tool_use/tool_result pairing | Accepted |
| [0005](ADR-0005-agent-loop.md) | Sample→dispatch→append loop; non-streaming, serial-first; max-turns | Accepted |
| [0006](ADR-0006-dialects-and-edit-encoding.md) | Dialect packs over one registry; `grok` default; `EditEncoding` enum | Superseded by 0012 |
| [0007](ADR-0007-provider-trait.md) | `Provider` trait over API-agnostic request; Anthropic Messages first | Accepted |
| [0008](ADR-0008-dispatch-door-and-path-jail.md) | One dispatch door + workspace path jail (v0 security) | Accepted |
| [0009](ADR-0009-headless-io-contract.md) | Single JSON report on stdout; diagnostics on stderr | Accepted |
| [0010](ADR-0010-rust-tooling-baseline.md) | Rust tooling/CI baseline (pinned toolchain, fmt+clippy-deny+test) | Accepted |
| [0011](ADR-0011-search-ripgrep-bundling.md) | Search uses ripgrep (host-resolved, bundled at packaging) | Accepted |
| [0012](ADR-0012-harness-packs.md) | Harness packs — faithful per-harness toolsets (supersedes 0006) | Accepted |
| [0013](ADR-0013-conversation-protocol.md) | Conversation protocol — 4-role (System/Developer/User/Assistant), Anthropic-shaped blocks | Accepted |
