# ADR-0027: Parallel tool dispatch — batched approval, per-path locking

## Status

**Draft — not approved.** Written to capture the source study while it is fresh;
scheduled **P1**, revisited when there is time. Nothing in this ADR is
implemented, and the serial dispatch of ADR-0005 remains the shipped behavior
until this is accepted.

## Date

2026-07-26

## Scope

How one assistant turn's `tool_use` batch is executed: what may run
concurrently, what serializes, and where approval sits relative to execution.
Supersedes ADR-0005's *"Parallel tool batches in v0 — Deferred"* alternative and
its prescription. Amends ADR-0017 (the approval seam moves ahead of dispatch).

Out of scope: streaming, compaction, subagents. Concurrency *within* one tool
(background bash) is a separate, already-deferred item.

## Context

`Session::dispatch_batch` runs a turn's calls serially. ADR-0005 deferred
parallelism with a specific instruction:

> Deferred: correctness before speed. When added, copy Codex's minimal-correct
> form — one `RwLock<()>` where read-only tools take `read()` and mutating tools
> take `write()`.

That instruction was written from a partial reading. Re-studying the four
harnesses finds **three** distinct designs, not one, and they differ by an order
of magnitude in how much parallelism they actually permit.

**Codex — one global `RwLock<()>`** (`tools/parallel.rs:48,133-137`). Exactly as
ADR-0005 described, and still current:

```rust
let _guard = if supports_parallel {
    Either::Left(lock.read().await)
} else {
    Either::Right(lock.write().await)
};
```

Coarsest of the three: any mutating call excludes *every* other call, related or
not.

**Claude Code — a binary per-call predicate plus a scheduler.**
`isConcurrencySafe(input)` and `isReadOnly(input)` (`Tool.ts:402-404`), both
defaulting to `false` (`Tool.ts:759-760`). `FileRead`, `Grep`, `Glob`,
`WebFetch`, `WebSearch`, `LSP` return `true`; `Bash`, `FileWrite` and
`FileEdit` define no override and are therefore unsafe. The scheduler
(`StreamingToolExecutor.ts:129-148`) admits a safe tool only when everything
in flight is safe, and on an unsafe tool **breaks the loop** rather than
skipping it, so relative order is preserved. Note the predicate takes `input`:
safety is a property of the *call*, not the tool.

**Grok Build — per-path mutexes** (`tool_dispatch.rs:44-58`,
`tool_calls.rs:387-404`). The finest-grained, and the one ADR-0005 missed:

```rust
/// Returning the same string for two calls in a batch causes them to share a
/// `tokio::sync::Mutex` and therefore run sequentially in model-emitted order.
/// Returning `None` lets the call run fully concurrently with everything else.
fn lock_path_for_args(args) -> Option<&str>   // file_path | path | target_file
```

At dispatch it collects `write_paths` from the non-read-only calls, then creates
a mutex **only for a path some write in this batch touches** — so a read of a
file nobody writes takes no lock at all, and two edits to unrelated files run
concurrently. `target_directory` is deliberately excluded: "a directory listing
isn't an edit and must not bucket into a file lock."

The decisive comparison: on a turn that edits three unrelated files, Codex
serializes all three; grok runs all three at once. Both are correct. Only one is
worth the complexity budget.

Separately, the Messages API makes the batch shape load-bearing: results for a
parallel batch must come back in **one** user message. We already do this
(`run.rs` step (f) appends one `User` message), so nothing regresses — but it is
why a partial-batch design is not on the table.

## Decision

**1. Approval becomes a batch phase that completes before any execution.**

Today the loop interleaves per call: cancel-check → approve → dispatch → next.
Grok's dispatcher instead operates on an already-`approved` set. We adopt that
split. Two reasons, only one of which is about parallelism:

- Concurrent dispatch under the current shape would fire N simultaneous
  approval prompts at an interactive frontend. ADR-0017's note that approvals
  are consulted serially "so an interactive frontend naturally receives one
  prompt at a time" is a property of the *interleaving*, and it must survive.
- Independently useful: a frontend can present "3 tools want approval" as one
  decision instead of three sequential modals.

Approvals stay serial *within* the phase. A denial remains soft (ADR-0004): the
denied call gets its paired `is_error` result and the rest of the batch still
runs.

**2. Locking is per path, following grok — with one deliberate divergence.**

A call's lock key is its `file_path`-family argument. Calls sharing a key
serialize in model-emitted order; calls with no key and no conflict run
concurrently. Mutexes are created only for paths a write in this batch touches.
`ToolKind` (`Shell·Read·Write·Edit·Glob·Grep·Other`) supplies the read-only
axis, so no new per-tool metadata is needed.

**The divergence: `ToolKind::Shell` is exclusive.** A shell call declares no
path, so per-path locking would give it no lock and let two `bash` calls run
concurrently — which is what grok does. Two shell commands can obviously
conflict through the filesystem, the environment, or a port. Claude Code refuses
to run Bash concurrently at all, and we follow Claude Code here. `ToolKind::Other`
is likewise exclusive (unknown blast radius, the ADR-0003 default-deny posture).

**3. Model-emitted order is an invariant, not an accident.**

Results in the transcript and records in `tool_calls[]` are ordered by the
model's emission order regardless of completion order. Cheap to guarantee
(collect by index), and load-bearing: eval A/B comparisons must not acquire
run-to-run noise from scheduling.

**4. Cancellation moves inside each future.** ADR-0018's between-calls check
becomes a per-future check, and the "pair the remainder synthetically" path must
handle a partially-completed batch.

## Alternatives Considered

### Codex's global `RwLock<()>` (what ADR-0005 prescribed)
Rejected. Minimal and obviously correct, but it serializes edits to unrelated
files — the single most common multi-tool turn a coding agent emits. The
per-path scheme is a modest amount of extra code for most of the available win.

### Claude Code's binary safe/unsafe predicate
Rejected as the primary mechanism, borrowed for `Shell`. Simpler than per-path,
but it cannot express "these two writes are independent", so every write batch
degrades to serial. Its per-*call* (not per-tool) granularity is the right
instinct and is what the path key gives us.

### Parallelism without the approval split
Rejected: it makes an interactive frontend show N modals at once, and there is
no way to reconcile that with ADR-0017 after the fact.

### Keep serial dispatch
Still defensible, and remains in force while this is a draft. Serial is correct,
and the win here is latency on multi-file turns, not capability. This ADR exists
so the decision is made from the sources rather than from memory when the time
comes.

## Consequences

- Multi-file turns get materially faster; single-tool turns are unchanged.
- The approval seam changes shape (ADR-0017 amendment), which is the largest
  single piece of work and the one with real design content.
- `dispatch_batch` grows a scheduler; its serial path must stay reachable and
  tested, because it is what every unsafe batch degrades to.
- Determinism is preserved by construction, so existing eval outputs stay
  comparable.

## Open Questions

- **Denial semantics under batching.** If call 2 of 4 is denied, do 3 and 4 run?
  This ADR says yes (soft, ADR-0004), but a "deny and stop" frontend composes
  deny + cancel, and the interaction with an already-dispatched batch needs
  stating.
- **Concurrency ceiling.** Neither grok nor Claude Code caps the fan-out that we
  found. A turn emitting 20 reads would open 20 file handles at once; whether we
  want a bound is unresolved.
- **`Other`/MCP tools.** Treated as exclusive here. When MCP lands, some servers
  will declare their own safety, and that metadata should probably feed the key.
