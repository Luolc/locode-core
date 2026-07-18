# ADR-0002: Multi-crate `locode-*` Cargo workspace

## Status
Accepted

## Date
2026-07-17

## Context
The cleanest studied Rust trees (Codex, Grok Build) separate a **portable tools layer** from the **session/loop layer**, and further separate *tool definition* from *dialect selection* and *wire protocol*. Crate boundaries make those separations enforceable by the compiler rather than by convention. The design-doc placeholder names were `agent-*`; the real project uses `locode-*`.

## Decision
Use a Cargo workspace of small `locode-*` crates. `locode-core` names the **workspace/repo**, not a crate.

> **Note (superseded naming):** `locode-dialects` was renamed **`locode-packs`** by
> [ADR-0012](ADR-0012-harness-packs.md) — harness packs (faithful per-harness toolsets)
> supersede the dialect-packs/`EditEncoding` model of the superseded ADR-0006. The crate
> boundary this ADR establishes is unchanged; only the crate's name and contents did.

| Crate | Role |
|---|---|
| `locode-protocol` | conversation model (4-role, ADR-0013), tool call/result, report envelope (pure types, no I/O) |
| `locode-tools` | canonical `Tool` trait + registry + dispatch (host-agnostic framework) |
| `locode-packs` (was `locode-dialects`) | harness packs — faithful per-harness toolsets over `locode-tools` (ADR-0012) |
| `locode-provider` | `Provider` trait + API-agnostic `ConversationRequest` + Anthropic wire impl |
| `locode-host` | fs/shell/path-jail/truncation/rg-resolution (injectable side-effect seam) |
| `locode-engine` | sample→dispatch→append loop + `Session` driving API |
| `locode` | thin facade re-exporting the public surface |
| `locode-exec` | minimal headless binary |

## Alternatives Considered
### Single crate with modules
- Pros: fewer `Cargo.toml` files; simplest start.
- Rejected: boundaries become conventions only; the portable-tools vs orchestration split (the strongest structural lesson from the survey) erodes as the code grows.

### Fewer, coarser crates (3–4)
- Pros: middle ground.
- Rejected: softens exactly the boundaries we most want hard (host seam, provider wire, tools vs loop). We can always merge later; splitting later is harder.

## Consequences
- Dependency direction is explicit and acyclic: `protocol` is the shared base; `engine` composes `tools`/`packs`/`provider`/`host`; `locode` re-exports; `locode-exec` depends only on `locode`.
- Tools **never** touch `std::fs`/`Command` directly — only through `locode-host` — making them trivially testable and sandbox-ready.
- More Cargo manifests and a `[workspace.lints]` table to maintain; accepted cost.
