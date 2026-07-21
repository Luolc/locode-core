# ADR-0018: Public cancellation — handle, semantics, and the `cancelled` status

## Status
Proposed (under review)

## Date
2026-07-20

## Context
Esc-to-interrupt is a core interactive behavior (every studied harness has it),
and headless timeout handling (Task 21, SIGTERM) needs the same machinery. The
plumbing half-exists:

- `Session` owns a `CancellationToken` (`crates/locode-engine/src/session.rs:26`)
  — but it is `pub(crate)` with **no public accessor**: nothing outside the
  engine can trigger it.
- The token flows into every `ToolCtx` (`run.rs:151-156`) and the host honors it
  cooperatively — the shell captures "completed, timed out, or cancelled"
  (`crates/locode-host/src/shell.rs:26,39`).
- The **sampling step does not observe the token at all**: `sample_with_retry`
  awaits `provider.complete()` unguarded (`run.rs:212`), and the backoff sleep
  (`run.rs:224`) likewise. Mid-sample cancellation today would do nothing until
  the next tool call.
- `Status` has four variants — `Completed | MaxTurns | ModelError | Error`
  (`crates/locode-protocol/src/…:216-224`) — none of which honestly describes
  "the user stopped it." **The report envelope is an ask-first boundary**
  (AGENTS.md), so the status question is called out explicitly below.
- **Task 21 discrepancy (repair in this change-set):** `tasks/todo.md` shows
  Task 21's acceptance criteria checked `[x]`, but no signal-handling or
  cancellation code exists anywhere in `locode-exec` (verified by search: no
  `signal`/`SIGTERM`/`cancel` references; no implementing commit in history).
  The checkboxes are false and must be unchecked; Task 21's substance folds into
  this ADR's implementation task.

Studied-harness semantics: interrupt preserves partial work — Claude Code keeps
partially-streamed text as an assistant message on cancel
(`src/screens/REPL.tsx:2125-2129`); codex interrupts a turn but retains the
transcript (`turn_interrupt`, `tui/src/app_server_session.rs:866`); grok's
cancel is idempotent and re-sendable (`app/dispatch/turn.rs:66-90`). Nobody
discards the conversation on interrupt.

## Options considered

### The handle
**H1 — `Session::cancel_handle() -> CancellationToken` (RECOMMENDED).** A clone
of the existing token; `CancellationToken` is already public API surface via
`ToolCtx::new` (`crates/locode-tools/src/ctx.rs:15` + tokio-util workspace dep).
Callers move it into a signal handler / key handler / timeout freely.
**H2 — a bespoke `CancelHandle` newtype.** Insulates the API from tokio-util,
but tokio-util is already load-bearing in the tool contract; a wrapper adds
surface without removing the dependency. Rejected.

### What cancellation means mid-run
**S1 — cooperative-plus-select (RECOMMENDED).** Two additions in the loop:
(a) guard the provider await and the backoff sleep with
`tokio::select! { biased; _ = cancel.cancelled() => …, r = provider.complete(...) => … }`
— dropping the in-flight reqwest future aborts the HTTP request cleanly; and
(b) check `cancel.is_cancelled()` at the top of each loop iteration
(`run.rs:44`) and between calls in `dispatch_batch` (`run.rs:143`), pairing any
not-yet-run calls of the current batch via the existing `synthetic_error`
mechanism (`run.rs:288`) so the transcript stays valid (ADR-0004) — the exact
shape already used for post-fatal pairing (`run.rs:144-149`).
**S2 — cooperative only (no select on the sample).** Simpler, but a cancel
during a long sample (the common case — sampling dominates wall-clock) would
stall until the model finishes; grok/codex/claude all abort the in-flight
request. Rejected.
**S3 — hard abort (drop the whole run future).** The TUI *could* just drop the
future, but that severs mid-batch pairing and loses the report — precisely what
Task 21 exists to prevent. Rejected as the *primary* mechanism (dropping remains
safe-by-construction where S1 already paired everything).

### The terminal status — **ask-first decision**
**T1 — new `Status::Cancelled` (RECOMMENDED).** Honest, matches every studied
harness's distinct "interrupted" state (opencode `session.abort`, codex
`TurnAborted`, claude `user-cancel`). Wire form `"cancelled"`. This is an
**additive** enum variant: JSON consumers matching on strings see a new value
only when they cancel; `schema_version` stays `1` with a documented
additive-evolution policy (new variants/fields are not breaking; renames/removals
are). Exit-code mapping in `locode-exec`: cancelled = **structured** terminal
state → exit 0 with the report (Task 21's requirement: a timed-out eval run
yields a failure-case trace, not nothing).
**T2 — reuse `Error { error: "cancelled" }`.** No schema change, but it makes
"user pressed Esc" indistinguishable from a real fault in every downstream
aggregation, and exit-code mapping (`crates/locode-exec/src/output.rs:46-50`)
would report failure for an intentional stop. Rejected.
**T3 — `Completed` with a flag.** Lies about completion. Rejected.

## Decision (proposed)
H1 + S1 + T1, plus the deferred Task 21 delivery on top:

1. `Session::cancel_handle()` public accessor; cancellation is sticky and
   idempotent (grok's rule) — a cancelled session's next `run()` returns a
   `cancelled` report immediately unless the token is replaced (fresh token per
   run; the handle returns the *current* run's token — exact lifecycle detailed
   in the task plan).
2. Loop observes the token at: iteration top, provider await (select), backoff
   sleep, and between batch calls — with synthetic pairing for the unfinished
   batch (`run.rs:288`).
3. `Status::Cancelled` (+ report `error: None`, `final_message` = last
   assistant text like `MaxTurns`, `run.rs:237`); `Event::Result` carries it
   unchanged; exec maps it to exit 0.
4. `locode-exec` installs a SIGTERM handler (tokio `signal`) that triggers the
   handle — delivering Task 21's real acceptance criteria (report on stdout,
   valid stream tail, paired transcript).

## Consequences
- The TUI's Esc handler is one line; partial work is preserved by construction
  (history keeps all appended messages — with ADR-0016, the next turn continues
  the same conversation after an interrupt, the claude/codex behavior).
- Report envelope evolution policy (additive = non-breaking at
  `schema_version: 1`) is now written down — future variants get the same
  treatment.
- Task 21's false `[x]` marks are corrected in the same change; its
  implementation lands with this ADR's task, not as a separate slice.
