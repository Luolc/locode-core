# Task 27 / Slice 3 — cancel: Esc/Ctrl+C interrupt, cancelled separator

Per [`docs/tui-dev-process.md`](../../docs/tui-dev-process.md). Grounding:
SPEC-TUI §Interaction contract; study §5; ADR-0018.

## Phase 0 — status analysis

- **State**: slice 2 merged (#78) — runs drive end-to-end; Esc while running
  is inert; the `cancelled` separator path exists in blocks but is unreachable.
- **Minimal next unit**: Esc (and Ctrl+C) during a run fires the run's cancel
  handle; the UI shows "cancelling…" and settles ONLY on the run's terminal
  report; a cancelled run renders the calm `cancelled` separator.
- **Why now**: cancel is the last piece of the core turn loop before approvals
  (slice 4) layer on top; the study is unanimous that cancel is where TUIs
  accumulate scars, so it lands on its own slice with dedicated tests.
- **Prereqs**: slice 2 run lifecycle (exists); `Session::cancel_handle()` +
  `Status::Cancelled` (ADR-0018, shipped 0.1.4); `CancellationToken` facade
  re-export (verified present).
- **Unblocks**: slice 4 ("deny and stop" = deny + cancel, composed); slice 5.
- **Risks**: (1) the engine task is blocked awaiting `run()` mid-run, so it
  can't process a `UiCommand::Cancel` — the handle must be handed to the UI at
  RunStarted; (2) firing the token is a memory op but must stay loop-side to
  keep the reducer sans-IO/testable; (3) idempotent re-fire on a stuck run.

## Phase 1 — harness revisit (fresh reads 2026-07-21)

- **grok** `dispatch_cancel_turn` (`src/app/dispatch/turn.rs:54-131`, re-read):
  cancel is idempotent and **retryable** — a second cancel on an already-
  cancelling turn re-sends rather than no-opping, "so Ctrl+C is never a dead
  key on a stuck Cancelling… spinner". → **Adopt**: a second Esc/Ctrl+C while
  cancelling re-fires the token (harmless; the token is idempotent).
- **codex** interrupt (`chatwidget/protocol.rs:253-264` + `input_restore.rs:
  118` + `turn_runtime.rs:507`, re-read): the UI settles **only** on the
  server's `TurnStatus::Interrupted`; never fakes completion; the active cell
  finalizes as failed; an error/notice cell is added. → **Adopt**: settle only
  on `RunFinished` (our authoritative terminal event); the `cancelled`
  separator IS the settle marker; pending tools flushed (already done in
  slice 2's `on_run_finished`).
- **claude-code**: Esc preserves partial streamed text + auto-restores prompt
  on fast interrupt (`REPL.tsx:2121-2129,2996-3022`). → Partial-text preserve
  is automatic (blocks already printed). Prompt auto-restore on
  pristine-cancel = **deferred** (print-once can't un-print; the cancel-rewind
  polish grok pays a cross-cutting tax for — spec defers it).
- **opencode**: double-Esc arm to interrupt (`prompt/index.tsx:391-420`). →
  **Rejected** for the running case: our Esc cancels on the first press (spec
  table); the double-press is reserved for clear-draft at idle.

**Decisions**: Esc-while-running → immediate cancel (spec). Ctrl+C-while-
running → cancel + arm quit (spec: "first press cancels run and arms quit").
Handle handed to the UI in `RunStarted { cancel }`, stored loop-side, fired on
`Cmd::CancelRun`, cleared on `RunFinished`. Reducer stays token-free (tests
need no real token). Prompt auto-restore + cancel-rewind **deferred**.

## Phase 2 — design

- `engine.rs`: `EngineMsg::RunStarted { cancel: CancellationToken }`; the task
  clones `session.cancel_handle()` BEFORE `run_text` (ADR-0018 mandate) and
  ships it. (ADR-0018: per-run token, retired at run end — a late cancel is a
  harmless no-op by construction.)
- `app.rs`: `RunState::Running { started, cancelling: bool }`; new
  `Cmd::CancelRun`. Esc while running → `cancelling = true`, hint `Cancelling`,
  `vec![Cmd::CancelRun]` (idempotent on repeat). Ctrl+C while running →
  `Cmd::CancelRun` + arm quit. `on_run_finished` resets to Idle (cancelling
  cleared with the state).
- `event_loop.rs`: `current_cancel: Option<CancellationToken>`; peek
  `RunStarted` to capture it before dispatch; `Cmd::CancelRun` → `cancel()`
  (idempotent); `RunFinished` → clear. New `Hint::Cancelling`.

### Edge cases

Second Esc while cancelling (re-fire); Ctrl+C twice while running (cancel then
quit); cancel arriving after the run already finished (no-op — token retired,
`current_cancel` cleared); cancel with pending tools (flushed by
`on_run_finished`, already tested); typing while cancelling (allowed — compose
next prompt); cancelled report renders `cancelled` separator.

### Test matrix / preset targets

1. [reducer] Esc while running → `Cmd::CancelRun` + `cancelling` + hint; second
   Esc → `Cmd::CancelRun` again (idempotent); Esc while idle unchanged.
2. [reducer] Ctrl+C while running → `Cmd::CancelRun` + quit armed; second
   Ctrl+C within window → `Cmd::Quit`.
3. [reducer] `RunFinished(Cancelled)` → Idle, cancelling cleared, `cancelled`
   TurnEnd block.
4. [integration] engine task + scripted mock emitting a `run_terminal_cmd
   {command:"sleep 30"}` tool turn; capture the `RunStarted` handle; fire it;
   assert `RunFinished(Cancelled)` within a few seconds (real host cooperative
   cancel).
5. [PTY smoke] `locode --api-schema mock` with a slow tool script via
   `LOCODE_MOCK_SCRIPT`: submit → Esc mid-run → `cancelled` separator; exit 0.
6. [gates] fmt/clippy/test/doc green.

## Open questions for the user (non-blocking)

- None new. (Prompt auto-restore on pristine cancel deferred as above; revisit
  if smoke testing makes the lack felt.)

## Result (2026-07-21)

Shipped: `EngineMsg::RunStarted { cancel }` (handle cloned before run per
ADR-0018), `Cmd::CancelRun` + `RunState::Running { cancelling }` +
`Hint::Cancelling` in the reducer, loop-owned `current_cancel` captured at
start / fired on CancelRun / cleared at finish, status row + footer cancelling
copy. Esc-while-running cancels (idempotent re-fire); Ctrl+C-while-running
cancels + arms quit; settles ONLY on the run's terminal report; cancelled
separator renders calm (no error). Reducer stays token-free.

All preset targets met: 318 workspace tests (4 new reducer cancel tests + 1
unix integration test that fires the handle mid `sleep 30` and asserts a
`Cancelled` report). Full gates + doc green. PTY smoke: slow-tool run → Esc →
`sleep 30` killed (exit -1) → `cancelled` separator, exit 0.

Deviations: none from plan. Next: slice 4 (approvals — TuiApprover, FIFO
overlay; "deny and stop" composes with this cancel handle).
