# Task 27 / Slice 2 — drive a run: engine task, transcript blocks, status row

Per [`docs/tui-dev-process.md`](../../docs/tui-dev-process.md). Grounding:
SPEC-TUI §Architecture/§Rendering model; study §3.3/§3.6; ADR-0016/0014.

## Phase 0 — status analysis

- **State**: slice 1 merged (#77) — shell runs, submit produces `Cmd::Submit`
  that goes nowhere. No engine wiring; no blocks.
- **Minimal next unit**: submit → real `Session` run (mock or live wire) →
  transcript blocks printed once via `insert_before` → status row while
  running → per-run `TurnEnd` separator.
- **Why now**: the first vertical slice that exercises all four core seams'
  happy path; slices 3–5 only add interrupts/gates/polish around this flow.
- **Prereqs**: slice 1 loop + outbox-capable draw path (exists); locode-core
  0.1.4 facade (exists).
- **Unblocks**: slice 3 (cancel needs a run to cancel), slice 4 (approvals
  fire during dispatch), slice 5 (queueing needs run state).
- **Risks**: (1) sink is sync — event channel shape; (2) echoing the wrapped
  `<user_query>` text would be ugly; (3) pending tool calls vs print-once.

## Phase 1 — harness revisit (fresh reads 2026-07-21)

- **grok** turn status row: single row `⠧ Run command 0.2s … 1m20s ⇣12k
  [stop]`, hidden at 0 height when idle; spinner slowed ~7.5 fps
  (`views/turn_status.rs:1-33`, re-read today). → **Adopt** shape minus
  buttons/tokens (no mid-run token events in our core): spinner + activity +
  elapsed. Animation tick only while running.
- **codex** `FinalMessageSeparator`: full-width rule, elapsed label
  (`history_cell/separators.rs:11-31`, re-read today); user cells `"› "`
  prefixed; exec cells "Ran <cmd>" with head/tail output truncation
  (study §6). → **Adopt**: `TurnEnd` separator per run (status + turns +
  tokens + elapsed — richer than codex because our per-run Report carries
  usage); tool body truncation by rows with head/tail keep.
- **claude-code**: replace-don't-append progress; partial tool_use rendered
  early. → Progress replacement N/A (no deltas); early tool rendering
  **adapted**: pending tool shows in the *status row* (print-once forbids
  provisional blocks in scrollback); block prints when the result pairs.
- **opencode**: two tool shapes; per-tool pending verbs. → **Deferred**: one
  compact shape in v1 (spec); per-tool verbs deferred with it.

**Decisions**: UI-side echo of the submitted prompt (skip the engine's
wrapped `<user_query>` user message; tool_result user messages consumed for
pairing) — implement now. Unbounded event channel accepted for v1 with a
recorded audit note: event volume is bounded by turn count (whole messages,
no deltas; ADR-0005 non-streaming); revisit at the streaming extension.
Session assembly duplicated from exec (~60 lines) — flagged below.

## Phase 2 — design

- `engine.rs`: `spawn(cli, registry) -> (UiCommandSender, EngineMsg receiver)`.
  Engine task owns the `Session` (built like exec run.rs: canonicalized cwd →
  Host/jail (+`--yolo` ⇒ Unrestricted) → grok pack + preamble → registry
  provider → EngineConfig). `UiCommand::Submit(String)`; `EngineMsg::{Ready
  {model, harness}, BuildFailed(String), RunStarted, Event(locode Event),
  RunFinished(Report)}`. Sink = `FnSink` → unbounded sender. Prompt shaped
  with `grok::prompt::user_query` (pack-faithful, as exec).
- `App` gains `run: RunState {Idle, Running{started, active_tool}}`,
  `outbox: Vec<Block>`, transcript translation from events, and a spinner
  frame counter ticked by the loop only while running.
- `ui/blocks.rs`: `Block::{UserPrompt, AssistantText, ToolCall, TurnEnd,
  Notice}`; `render(width) -> Vec<Line>`; tool body truncated to 6 rows
  head/tail with `… +N lines` marker (codex numbers scaled down).
- Loop: drains `app.outbox` → `terminal.insert_before` before each draw;
  engine arm in the biased select **gated on input queue empty**, batch-drain
  ≤32 (the grok rule, now real).
- Event→block mapping: assistant Text → `AssistantText`; assistant ToolUse →
  pending (status row); user ToolResult → finalize pending → `ToolCall`
  block; engine `Error` event → `Notice`; `Init`/`Approval` ignored this
  slice; user text messages skipped (UI echoed at submit).
- Enter during a run: ignored this slice (footer hint "run in progress");
  queueing is slice 5 per spec.

### Edge cases

Multiple tool_use in one assistant turn (pair by id, order preserved);
tool_result arriving with `is_error`; run finishing with pending tools
(cancel path, slice 3 — pending flushed as errored); `BuildFailed` pre-run
(engine says why, app shows Notice and stays usable for `/quit`); submit of
multi-line text; report with `usage.input_tokens = 0` (mock) — separator
still renders.

### Test matrix / preset targets

1. [unit] Event→block translation: text turn, tool turn + result (ok and
   is_error), error event, wrapped-user-message skip.
2. [reducer] submit → RunState::Running + UserPrompt in outbox; Enter during
   run ignored; RunFinished → TurnEnd block + Idle.
3. [blocks] TestBackend/line assertions: `❯ ` prompt, tool line + truncation
   marker, `─ completed · … ─` separator shape.
4. [integration] real engine task, mock wire, tempdir cwd: submit → collect
   EngineMsgs → Ready, RunStarted, ≥2 Events, RunFinished(Completed);
   two sequential submits share history (continuity seam observed).
5. [integration] unknown api-schema → BuildFailed, app stays alive.
6. [PTY smoke] `locode --api-schema mock`: submit "hi" → log contains the
   mock reply text and a completed separator; triple-Ctrl+C exit 0.
7. [gates] fmt/clippy/test/doc green.

## Open questions for the user (non-blocking)

- Session assembly (~60 lines) now exists in exec AND tui; factor into the
  facade as `assemble_session(...)`? (Default: duplicate for v1; dedupe is a
  core change → hard-stop review later.)

## Result (2026-07-21)

Shipped: engine task (`engine.rs`, owns Session, typed UiCommand/EngineMsg
channels), transcript `Block` enum (`ui/blocks.rs`), event→block translation +
run lifecycle in the reducer, single-row status widget, and `insert_before`
print-once flushing in the loop with the engine arm gated on empty input +
bounded batch drain. All preset targets met: 313 workspace tests (incl. 8 new
reducer tests + 2 engine-task integration tests over a scripted mock, covering
continuity and build-failure); full gates + doc green. PTY smoke on the real
binary: `say hi` → user prompt block + assistant reply + `completed · 1 turn`
separator all render, exit 0.

Deviations from plan:
- **ratatui `scrolling-regions` feature required** (not planned): stock
  `insert_before` uses a cursor-position (CPR) query that *deadlocks* against
  our dedicated input-reader thread (the reader owns crossterm's global
  read-lock; the CPR reply can't be read). The `scrolling-regions` path writes
  history via DECSTBM with NO cursor query — the only viable inline-history
  path with a separate reader thread. Found via the slice-2 smoke; recorded as
  a design fact in ADR-0019 territory (the feature is enabled in Cargo.toml
  with a comment). Widened the slice-1 finding: automated PTY smokes must also
  set a winsize (`stty rows/cols`) — `script`'s pty is 0×0 otherwise and every
  draw renders nothing.
- Added `LOCODE_TUI_DEBUG_LOG` env-gated message log (the `insert_before`
  transcript leaves nothing greppable in a captured pty) — reusable smoke
  instrumentation, resolved once via OnceLock.

Open question still open (non-blocking): session-assembly duplication between
exec and tui (~60 lines) — a facade `assemble_session` helper is a future core
proposal (hard-stop). Next: slice 3 (cancel).
