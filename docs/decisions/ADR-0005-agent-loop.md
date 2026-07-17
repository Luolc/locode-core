# ADR-0005: Sample→dispatch→append loop; non-streaming, serial-first; explicit max-turns

## Status
Accepted

## Date
2026-07-17

## Context
Two loop patterns exist. Claude, Codex, and Grok **sample to done, then execute tools** and re-sample. OpenCode executes tools *inside* the model stream and pays for it by reconstructing tool-call/result state across 15+ event types plus a Promise/Effect bridge. For a headless engine whose output is a JSON document, the thing in-stream execution buys — tool work overlapping token generation — is irrelevant. Separately, Codex runs with **no turn cap** and relies on compaction as the only runaway guard; the other three keep an explicit ceiling.

## Decision
Use **sample → dispatch → append → re-sample**. Model calls are **non-streaming** in v0 (buffer each assistant turn fully before dispatching). Dispatch tools **serially** in v0. Enforce an **explicit `max_turns` ceiling** (default 30) that terminates with a `MaxTurns` status. Terminal conditions: no tool calls → `Completed`; `Fatal` tool error → `Error` (non-zero exit); provider error after bounded retry → `ModelError`; turn cap hit → `MaxTurns`.

## Alternatives Considered
### In-stream tool execution (OpenCode/AI-SDK style)
- Rejected: large event-stream reconstruction cost for a benefit a JSON-output agent doesn't use.

### No turn cap, rely on compaction (Codex style)
- Rejected: v0 has no compaction; a ceiling is the simpler, safer guard against runaway loops. Revisit once compaction lands.

### Parallel tool batches in v0
- Deferred: correctness before speed. When added, copy Codex's minimal-correct form — one `RwLock<()>` where read-only tools take `read()` and mutating tools take `write()`.

## Consequences
- The loop is small and testable with a mock provider (the highest-leverage test surface).
- Streaming, parallel dispatch, and compaction are additive extension points with reserved slots, not rewrites.
- A `max_turns` ceiling means the engine never hangs; callers get a structured terminal state every time.
