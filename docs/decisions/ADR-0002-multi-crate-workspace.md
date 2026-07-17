# ADR-0002: Multi-crate `locode-*` Cargo workspace

## Status
Accepted

## Date
2026-07-17

## Context
The cleanest studied Rust trees (Codex, Grok Build) separate a **portable tools layer** from the **session/loop layer**, and further separate *tool definition* from *dialect selection* and *wire protocol*. Crate boundaries make those separations enforceable by the compiler rather than by convention. The design-doc placeholder names were `agent-*`; the real project uses `locode-*`.

## Decision
Use a Cargo workspace of small `locode-*` crates. `locode-core` names the **workspace/repo**, not a crate.

| Crate | Role |
|---|---|
| `locode-protocol` | history model, tool call/result, report envelope (pure types, no I/O) |
| `locode-tools` | canonical `Tool` trait + registry + 6 tool impls (host-agnostic contracts) |
| `locode-dialects` | dialect packs (name/param/desc overrides + `EditEncoding`) over `locode-tools` |
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
- Dependency direction is explicit and acyclic: `protocol` is the shared base; `engine` composes `tools`/`dialects`/`provider`/`host`; `locode` re-exports; `locode-exec` depends only on `locode`.
- Tools **never** touch `std::fs`/`Command` directly — only through `locode-host` — making them trivially testable and sandbox-ready.
- More Cargo manifests and a `[workspace.lints]` table to maintain; accepted cost.
