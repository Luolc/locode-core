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
| [0012](ADR-0012-harness-packs.md) | Harness packs — faithful per-harness toolsets (supersedes 0006) | Accepted (fidelity boundary clarified by 0023) |
| [0013](ADR-0013-conversation-protocol.md) | Conversation protocol — 4-role (System/Developer/User/Assistant), Anthropic-shaped blocks | Accepted (`Developer` narrowed by 0023) |
| [0014](ADR-0014-streaming-event-protocol.md) | Streaming event protocol (`stream-json`) — self-sufficient trace source | Accepted |
| [0015](ADR-0015-custom-provider-injection.md) | Custom providers — `ProviderRegistry` + library-entry `locode-exec` | Accepted |
| [0016](ADR-0016-session-continuity.md) | Session continuity — multi-turn conversations in the engine | Accepted |
| [0017](ADR-0017-interactive-approval-seam.md) | Interactive approval seam at the engine's dispatch step | Accepted |
| [0018](ADR-0018-cancellation-and-cancelled-status.md) | Public cancellation — handle, semantics, `cancelled` status | Accepted |
| [0019](ADR-0019-tui-architecture.md) | TUI architecture — reducer loop, library-plus-thin-binary crates | Accepted (Rendering decision superseded by 0022) |
| [0020](ADR-0020-markdown-code-highlighting.md) | TUI Markdown code-block syntax highlighting (`syntect` + `two-face`) | Accepted |
| [0021](ADR-0021-live-token-streaming.md) | Live token streaming — provider SSE → engine deltas → TUI incremental render | Accepted |
| [0022](ADR-0022-vendored-terminal-relative-frame.md) | Dynamic composer via a vendored terminal + relative-frame rendering (supersedes 0019 §Rendering) | Accepted |
| [0023](ADR-0023-fidelity-boundary-and-agents-md-loading.md) | Fidelity boundary (packs = tools + prompt); shared `AGENTS.md` loading with `User`-role injection (amends 0012, 0013) | Accepted |
| [0024](ADR-0024-locode-home-settings-and-traces.md) | `~/.locode` — layered JSON settings + resumable JSONL session trace (cwd-keyed, open extension contract) | Accepted (skills §3 resolved by 0025; default `harness` → `grok`) |
| [0025](ADR-0025-agent-skills.md) | Agent Skills — shared discovery + whole-body-diffed `<system-reminder>` listing (rescanned post-run); no tool — the model reads `SKILL.md` (amends 0008, 0023, 0024) | Accepted |
| [0026](ADR-0026-slash-commands-core.md) | Slash commands — the **core** contract: a `SlashCommand` trait, a value-returning result, every `user-invocable` skill becomes a command; plain-text arguments (UI is a later plan) | Accepted |
| [0027](ADR-0027-parallel-tool-dispatch.md) | Parallel tool dispatch — batched approval then per-path locking (supersedes 0005's deferral; amends 0017) | **Draft — not approved**, P1 |
