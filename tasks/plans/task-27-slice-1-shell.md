# Task 27 / Slice 1 — the shell: crates, terminal lifecycle, event loop, composer

Per [`docs/tui-dev-process.md`](../../docs/tui-dev-process.md). Grounding:
SPEC-TUI §Crates/§Architecture/§Robustness floor; study §3.2/§3.5/§3.7/§6.

## Phase 0 — status analysis

- **State**: no TUI code exists; SPEC-TUI + process doc merged; deps approved.
  0.1.4 seams unused as yet (this slice needs none of them).
- **Minimal next unit**: both crate scaffolds + a runnable empty app: terminal
  init/teardown/panic/signal discipline, input thread, biased event loop,
  multiline composer, quit/clear keys. No engine.
- **Why this first**: every later slice renders *into* this shell; terminal
  robustness must be slice 1 (process doc floor) because retrofitting restore
  discipline is how the studied harnesses accumulated their scars.
- **Prereqs**: none beyond merged spec (verified: workspace builds green).
- **Unblocks**: slice 2 (engine task renders blocks into this loop).
- **Risks**: (1) stock ratatui `Viewport::Inline` is fixed-height — live
  region sizing; (2) tui-textarea/ratatui version alignment; (3) quit/clear
  key state machines are easy to get subtly wrong — reducer tests carry them.

## Phase 1 — harness revisit (fresh reads 2026-07-21, same-day study + verification reads)

| Area | grok | codex | claude-code | opencode |
|---|---|---|---|---|
| Input source | dedicated thread `poll(100ms)+read` → mpsc; `is_closed()` exit check within one poll cycle (`event_loop.rs:1093-1130`, re-read today) | `EventStream` behind drop/recreate broker (`event_stream.rs:10-18`) | Ink stdin | OpenTUI |
| Teardown | one fn, shared by exit/panic/signal; sync-update end first (`app/mod.rs:1185-1245`, re-read) | 3-layer: panic hook chains previous (`tui.rs:504-510`, re-read), Drop guard, hard kitty reset | writeSync outside React + timeout | Effect acquireRelease |
| Quit keys | Ctrl+C two-step (draft-clear then cancel/quit escalation) | Ctrl+C arm+confirm, Ctrl+D on empty (`interaction.rs:360-445`) | Ctrl+C double-press exit | `exit` typed / commands |
| Esc at idle | double-Esc 800ms clears draft (`agent_view/prompt.rs:751-830`) | Esc primes backtrack on empty composer | double-Esc rewind picker | esc exits shell-mode first |

**Decisions**: input thread grok-style (implement now); panic hook codex-style
chaining + teardown-defined-once grok-style (now); Ctrl+C arm-quit +
Ctrl+D-empty-quit codex-style (now); double-Esc clear-draft 800 ms (now);
kitty protocol, mouse, EventStream broker, $EDITOR handoff, suspend/SIGTSTP
(**deferred**); alt-screen anything (**rejected**, spec).

**Live-region sizing decision**: fixed `Viewport::Inline(10)`, bottom-anchored
layout (blank rows read as margin). Dynamic viewport growth = deferred
(codex does it via custom terminal; not worth a fork). Flag for user: if the
fixed band feels wrong in smoke testing, the alternative is a smaller band
(6) or the custom-terminal route.

## Phase 2 — design

- Crates: `crates/locode-tui` (lib, `publish=false`) + `crates/locode-app`
  (bin **`locode`** — the user's `locode --yolo` ask resolves the naming
  question early; recorded as SPEC-TUI amendment in this PR).
- New workspace deps: ratatui 0.29, crossterm 0.28 (aligned with ratatui),
  tui-textarea 0.7, pulldown-cmark (added now, used from slice 5); clap
  reused for the CLI in `locode-tui`.
- Modules: `term` (init/teardown/panic/signals), `app` (App/Msg/Cmd/update),
  `ui` (draw, composer wrapper, footer), `cli`, `lib.rs::main_with`.
- Data flow: input thread → `mpsc<crossterm::Event>` → loop converts to
  `Msg::Key/Paste/Resize` → `update(&mut App, msg, now) -> Vec<Cmd>` →
  `Cmd::Quit` handled by loop; draw when `app.dirty`, ≥16 ms apart, deferred
  redraw timer when throttled.
- Signals: tokio task (SIGINT+SIGTERM, unix) → `Msg::SignalQuit` (graceful);
  panic hook does best-effort raw teardown then chains.

### Edge cases

Paste with `\r` (normalize to `\n`); Enter vs Alt+Enter; Ctrl+C with
non-empty draft (clears draft first — grok's two-step — then arms quit);
quit-arm expiry; Esc single vs double timing; resize storm (16 ms debounce);
input thread exit on channel close; double panic (hook must not panic).

### Test matrix / preset targets

1. [reducer] Ctrl+C: draft→clears draft; empty→arms; second within window→quit; expired→re-arm.
2. [reducer] Ctrl+D: quits only on empty composer; otherwise ignored.
3. [reducer] Esc double-press 800 ms clears draft; single press doesn't; expiry re-arms.
4. [reducer] Enter submits (Cmd::Submit placeholder in slice 1: clears composer, records to history); Alt+Enter inserts newline.
5. [reducer] Paste normalizes `\r\n`/`\r` to `\n`.
6. [unit] teardown sequence emitted once and idempotent (guard flag).
7. [TestBackend] draw renders composer bottom-anchored with footer hints; typed text visible.
8. [gates] fmt/clippy/test/doc green; both crates in workspace; `cargo run -p locode-app` compiles.
9. [manual smoke — listed, non-gating] run in a real terminal: type/paste/quit paths; panic!() test leaves terminal usable.

Deliverables also include: ADR-0019 (TUI architecture, short — records
inline-viewport/print-once, reducer, crate shape, pointing at spec+study);
SPEC.md pointer line; process-doc dep-rule amendment (user 2026-07-21:
reasonable deps allowed, recorded not asked); SPEC-TUI bin-name note.

## Open questions for the user (non-blocking)

- Fixed 10-row live region OK, or prefer smaller/dynamic? (Default: 10.)
- Any strong keybinding opinions beyond the spec table? (Default: spec.)

## Result (2026-07-21)

Shipped: `locode-tui` (lib: cli/term/app/ui/event_loop, 11 unit tests) +
`locode-app` (bin `locode`, 3-line main). ADR-0019 accepted; SPEC.md pointer;
process-doc dep-rule relaxation recorded; bin-name decision folded into
SPEC-TUI. All preset targets met: reducer table tests (quit/clear/submit/
paste), idempotent-restore unit test, TestBackend bottom-anchored draw test,
full gates green (301 workspace tests). PTY smoke (script + injected CPR
reply): init → draws → keys → triple-Ctrl+C quit → exit 0 with
paste-off/cursor-show teardown bytes verified in the log; no-TTY run fails
clean (error line, exit 1, no panic).

Deviations: none from plan. Notes: `script(1)` answers no CPR — automated
PTY smokes must inject `\x1b[R;CR`; recorded for later slices. Next: slice 2
(engine task + transcript blocks + insert_before).
