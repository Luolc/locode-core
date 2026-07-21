# Tasks 23–25: TUI core prerequisites (Workstream A)

The three engine seams the future TUI app needs, per ADR-0016/0017/0018
(all **Proposed — review before implementing**). Ordered so each slice is
independently shippable; together they release as **0.1.3**.

Source grounding for the designs lives in the ADRs; this plan holds the
implementation detail: touch points, edge cases, test matrix, and the open
questions to settle in review.

---

## Task 23: session continuity (ADR-0016)

**Touch points**
- `crates/locode-engine/src/session.rs:20-27` — add `history: Vec<Message>`,
  `turns_run: u32`; init history from preamble in `new` (`:43-50`); add
  `pub fn history(&self) -> &[Message]`.
- `crates/locode-engine/src/run.rs:15` — delete the local
  `let mut history = self.preamble.clone();`; operate on `self.history`.
  NOTE: `drive` takes `&mut self` and `dispatch_batch` takes `&self`
  (`run.rs:136`) — borrow-check requires either passing the batch slice data
  by value (already the case: `calls` is owned) or splitting borrows; expected
  to compile with `self.history` accessed only in `drive` itself.
- `run.rs:24-33` — gate `Event::Init` on `self.turns_run == 0`.
- `run.rs:46` — `repair_pairing(&mut self.history)` still runs per sample
  (unchanged invariant).
- Doc updates in the same PR: `session.rs:13` ("ephemeral" comment), SPEC
  driving-API paragraph, ADR-0014 dated note (multi-`Result` streams).

**Edge cases**
- Run 2 after a `ModelError`/`Error` terminal: history contains everything up
  to the failure; pairing repair heals any half-open tool_use on the next
  sample. Decide-in-review: allow continue-after-error (recommended: yes —
  claude/codex allow retrying after a failed turn) or require a fresh session.
- `MaxTurns` counts per run (`RunAcc.turns`, `run.rs:63`) — unchanged.
- Event streams: `stream-json` consumers (`reconstruct_conversation`,
  protocol) must tolerate `Init … Result Message … Result` — add a golden test.

**Tests**
1. Two-run continuity: mock provider asserts run 2's request contains run 1's
   messages (`MockProvider` scripted turns).
2. `Init` exactly once across two runs (FnSink capture).
3. Per-run report: run 2's `turns == 1` even though session total is 2.
4. Golden: `reconstruct_conversation` over a two-run event stream.

**Scope: S** (~150 line diff + tests). No dependencies.

---

## Task 24: cancellation + `cancelled` status + real SIGTERM (ADR-0018)

**Touch points**
- `session.rs:26` — `cancel` stays; add `pub fn cancel_handle(&self) -> CancellationToken`.
  **Token lifecycle (decide in review):** Option (a) one token for the session
  lifetime (cancel kills all future runs until `reset()`); Option (b) fresh
  token per run, `cancel_handle()` returns the current one (grok-style
  idempotent per-turn cancel; RECOMMENDED — an Esc during turn N must not kill
  turn N+1, and the TUI re-fetches the handle each turn).
- `run.rs:44` — iteration-top check; `run.rs:143` — between-calls check with
  `synthetic_error` pairing for the rest of the batch (mirror `run.rs:144-149`).
- `run.rs:212` and `:224` — `tokio::select!` (biased) on
  `cancel.cancelled()` vs the provider future / backoff sleep.
- `crates/locode-protocol` `Status` (`:216`) — add `Cancelled`; serde string
  `"cancelled"`; **ask-first item: report envelope evolution policy** (additive
  = non-breaking, `schema_version` stays 1) written into the protocol doc
  comment.
- `crates/locode-exec/src/output.rs:46-50` — `Cancelled => ExitCode::SUCCESS`
  (structured terminal state).
- `crates/locode-exec/src/lib.rs` (`main_with`) — install
  `tokio::signal::unix::signal(SignalKind::terminate())` task that fires the
  session's cancel handle; pre-run SIGTERM → clean exit 1 (Task 21 criteria).
  Windows: `tokio::signal::ctrl_c` equivalent only; SIGTERM arm is
  `#[cfg(unix)]`.
- `tasks/todo.md` Task 21 — uncheck the false `[x]` boxes, add a note
  ("verified unimplemented 2026-07-20; superseded by Task 24"), and keep the
  integration-test criterion (slow mock tool + SIGTERM mid-run → parseable
  report) as Task 24's test #4.

**Terminal semantics**
- Cancel during sample: no assistant message was appended — history unchanged
  since last append; report `final_message` = last assistant text (like
  `MaxTurns`, `run.rs:237`).
- Cancel during dispatch: current tool's own cooperative cancel (host) returns
  its result; remaining calls paired synthetically; then terminal.
- With Task 23: interrupted sessions continue on the next `run()` (fresh token).

**Tests**
1. Cancel mid-sample (mock provider with a delayed future) → `cancelled`
   report, valid pairing.
2. Cancel mid-batch (slow mock tool) → executed tool has real result, rest
   synthetic, all paired.
3. Idempotent double-cancel; cancel-then-next-run continues (with 23).
4. Exec integration: SIGTERM mid-run → one JSON report, `status: "cancelled"`,
   exit 0; stream-json tail stays valid JSONL ending in `result`.

**Scope: M.** Depends on Task 23 only for test #3 (can land before 23 with
that test deferred).

---

## Task 25: approval seam (ADR-0017)

**Touch points**
- New `crates/locode-engine/src/approve.rs` — `Approver` trait,
  `ApprovalRequest<'_>`, `Decision`, `AllowAll`; `#[async_trait]` (workspace
  dep already, `Cargo.toml:17`).
- `session.rs` — `approver: Arc<dyn Approver>` field (default `AllowAll` in
  `new`), `pub fn with_approver(self, …) -> Self`.
- `run.rs:143` (in `dispatch_batch`) — before `ToolCtx` construction:
  ```rust
  match self.approver.decide(&ApprovalRequest { … }).await {
      Decision::Allow => { /* existing dispatch path */ }
      Decision::Deny { reason } => {
          results.push(synthetic_error(&id, &format!("tool call denied: {reason}")));
          acc.tool_calls.push(/* denied record — see open Q3 */);
          continue;
      }
  }
  ```
- `ApprovalRequest.kind` from `Registry` — needs a spec/kind lookup by tool
  name: `Registry::specs()` exists (`run.rs:19`); check whether `ToolKind` is
  exposed per-tool (`crates/locode-tools/src/tool.rs:36`, `registry.rs`) or
  needs a small `Registry::kind_of(name)` addition (expected: yes, ~10 lines
  in locode-tools).
- Facade re-exports (`crates/locode-core/src/lib.rs:26-27` engine block):
  `Approver`, `ApprovalRequest`, `Decision`.

**Semantics**
- Deny is **soft**: paired `is_error` result, run continues (Claude Code's
  rejection model). "Deny and stop" = deny + cancel handle (Task 24), composed
  by the frontend.
- `dispatch_batch` is serial (`run.rs:143`), so approvals arrive at the
  frontend naturally one at a time in v1 — the TUI queue exists for
  future parallel dispatch, matching grok's FIFO.
- Default `AllowAll` ⇒ zero behavior change for exec/downstream (golden:
  existing exec integration tests must pass untouched).

**Tests**
1. Deny → paired `is_error` with reason; model sees it; run continues to
   `Completed`.
2. Deny-then-allow within one batch: order + pairing preserved.
3. `kind` is populated for a grok-pack tool (e.g. shell → `ToolKind::Shell`).
4. Async approver that actually awaits (oneshot) — proves the suspend-only-
   this-call property.
5. Golden: default approver ⇒ byte-identical exec behavior.

**Scope: M.** Independent of 23/24.

---

## Resolutions (user interview, 2026-07-20 — all questions closed)

1. **(23)** Continue-after-error: **allowed unconditionally** for both
   `ModelError` and `Error` (ADR-0016 Resolution).
2. **(24)** Token lifecycle: **per-run token, replaced at run end**; handle
   cloned pre-run; no public `reset()`; double-cancel idempotent (ADR-0018
   Decision 1, with the four-harness cross-reference).
3. **(25)** Denied calls: **recorded** with additive
   `ToolCallRecord.denial_reason: Option<String>`, set only from the
   approver-deny path; cancellation synthetics never carry it (ADR-0017
   Decision 5).
4. **(24)** **`Status::Cancelled` approved**; additive-evolution policy at
   `schema_version: 1` incl. unknown-value tolerance (ADR-0018 addenda).
5. **(25)** **`Event::Approval` ships in v1** — grok-shaped
   `{ tool_use_id, tool_name, decision, wait_ms }` (ADR-0017 Decision 4).
6. **(release)** **One 0.1.3** after all three slices. Implementation order:
   **23 → 25 → 24**.
7. **Broad `#[non_exhaustive]`** on `Status`/`Event`/`ApprovalRequest`/
   `Decision`; exec status match gains a wildcard arm → exit 1 (ADR-0018
   addenda). Lands with Task 24 (`Status`) and Task 25 (the rest).
8. **No stdin TTY hint** — declined, out of scope.

### Additional work items from the resolutions

- **Task 25** additionally: `Event::Approval` variant in `locode-protocol`
  (with `wait_ms` measured around the `decide()` await in `dispatch_batch`);
  `ToolCallRecord.denial_reason` field + serialization test; `#[non_exhaustive]`
  on `Event`/`ApprovalRequest`/`Decision`; test asserting an *allowed* call
  emits `Approval` with `decision: allow`.
- **Task 24** additionally: `#[non_exhaustive]` on `Status`; exec wildcard arm
  (unknown status → exit 1) + doc-comment stating the additive-evolution policy
  and unknown-value-tolerance guidance in the protocol crate.
