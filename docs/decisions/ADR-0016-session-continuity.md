# ADR-0016: Session continuity — multi-turn conversations in the engine

## Status
Proposed (under review)

## Date
2026-07-20

## Context
An interactive frontend (the planned TUI app) needs a **follow-up user message to
continue the same conversation**. Today the engine cannot do that:

- `Session` is documented as owning history "for the run (**ephemeral**)"
  (`crates/locode-engine/src/session.rs:13`).
- `drive()` rebuilds history from scratch on every call —
  `let mut history = self.preamble.clone();` (`crates/locode-engine/src/run.rs:15`) —
  and the vector is a local dropped when the run returns. A second `run()` on the
  same `Session` starts an unrelated conversation.
- The `Init` event (the stream's self-sufficient header, ADR-0014) is emitted at
  the top of every `drive()` (`run.rs:24-33`), so naive repeated runs would also
  produce a malformed multi-`Init` stream.

How the studied harnesses hold conversation state:

- **Grok Build** — state lives in the agent core, keyed by session id
  (`xai-chat-state`'s `ChatStateActor`; `crates/codegen/xai-chat-state/src/lib.rs:1-10`).
  A follow-up is another `PromptRequest` to the same `session_id`; the TUI keeps
  only view state.
- **opencode** — state lives server-side (`SessionV2` + `Database`); a follow-up is
  `POST /api/session/:id/prompt` (`packages/protocol/src/groups/session.ts:205`).
- **Claude Code** — the opposite: the loop is a stateless generator and the *UI*
  owns the transcript, resending the entire message array every turn
  (`src/screens/REPL.tsx:2794`, `messagesIncludingNewMessages` passed into a fresh
  `query()`).
- **Codex** — core-side threads; the TUI sends `turn_start` ops against a thread id.

Three of four keep conversation state on the core side. Claude Code's stateless
variant works because its only consumer is its own UI; our engine has several
consumers (locode-exec, the TUI app, downstream library users).

## Options considered

### Option A — stateful `Session`: history becomes a field (RECOMMENDED)
Move the conversation into the struct: `Session { history: Vec<Message>, … }`,
initialized from the preamble at construction; `drive()` appends to it and leaves
it in place; a second `run()` continues where the first ended.

- Pros:
  - A follow-up turn is literally `session.run_text(next_prompt)` — the exact
    call shape the TUI needs; `locode-exec` (one `run()` per process,
    `crates/locode-exec/src/run.rs`) is unaffected.
  - The engine-owned invariants stay engine-owned: pre-send pairing repair
    (`run.rs:46`, `repair_pairing`) and the verbatim-append rule for
    `Thinking{signature}` blocks (`run.rs:67-68`, ADR-0013) keep exactly one
    implementation. Callers cannot corrupt the transcript between turns.
  - Matches the majority pattern (grok/opencode/codex) and our own crate layout:
    the engine *is* our core-side.
- Cons:
  - `Session::run` semantics change from "independent run" to "next turn" —
    a behavioral change to a public API (documented; version-gated at 0.1.x).
  - Unbounded memory growth over a long interactive session — accepted; the
    compaction seam is already reserved (ADR-0005 consequences) and none of the
    studied cores bound history without compaction either.

### Option B — stateless engine, caller resends history (Claude Code style)
`run(history: Vec<Message>, user: Vec<ContentBlock>) -> (Vec<Message>, Report)`.

- Pros: purely functional; trivially testable; caller controls persistence.
- Cons: every consumer must re-implement history management, and the pairing /
  verbatim-append invariants become *caller obligations* — precisely the
  "correctness invariants, not style preferences" our working agreement pins to
  the engine. Claude Code affords this because UI state and loop state are the
  same React array; we would be exporting our hardest invariants across a crate
  boundary. Rejected.

### Option C — a separate `Conversation` handle
`session.run(&mut conversation, user)`: state in a caller-owned object, invariant
enforcement still engine-side.

- Pros: several conversations could share one configured `Session`; explicit
  ownership.
- Cons: more public API surface (a new type whose relationship to `session_id`,
  `Init`, and the `Report` envelope must all be specified); no consumer needs
  N-conversations-per-session today (the TUI's model is one session per view,
  like all four studied harnesses). Deferrable: Option A can evolve into C later
  by extracting the field — the reverse migration is the same code motion.
  Rejected for v1 as YAGNI.

## Decision (proposed)
Option A. Concretely:

1. `Session` gains `history: Vec<Message>` (initialized `= preamble.clone()` in
   `Session::new`) and `turns_run: u32`; `drive()` (`run.rs:14`) operates on
   `&mut self.history` instead of a local.
2. **`Init` is emitted once per session** — on the first `run()` only (guarded by
   `turns_run == 0`), keeping the event stream well-formed: `Init`, then an
   alternating sequence of `Message` events across all turns, with one `Result`
   per run. (ADR-0014 gets a dated note: a *session* stream may contain multiple
   `Result` events, one per run; `reconstruct_conversation` already folds
   `Message` events in order and is unaffected.)
3. **The `Report` stays per-run**: `turns`/`usage`/`tool_calls` (`RunAcc`,
   `run.rs:42`) count the current run only — the report is "what this run did,"
   which is what both the exec exit path and a TUI turn summary want. A
   cumulative view is derivable from the event stream; no envelope change.
4. Read access for frontends: `Session::history() -> &[Message]` (render the
   transcript after a run without replaying events).
5. `max_turns` (`config.rs:28`) continues to bound **a single run's** turns —
   unchanged semantics, now documented explicitly.

## Consequences
- The TUI's turn loop is `loop { let report = session.run_text(input).await; }`.
- `Session` doc comment (`session.rs:13-19`) and SPEC's driving-API description
  update in the same change; `locode-exec` needs no code change.
- Tests: follow-up run continues history (model sees turn-1 messages); `Init`
  emitted once; per-run report counters; pairing repair still runs per sample.
