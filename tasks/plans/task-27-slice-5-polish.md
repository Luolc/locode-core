# Task 27 / Slice 5 — conversation polish (queued prompts, history, slash, markdown)

Per [`docs/tui-dev-process.md`](../../docs/tui-dev-process.md). Grounding:
SPEC-TUI §Interaction contract / §Non-goals; study §3/§7.

**Subdivided (process-doc allowance):** 5a = interaction (queued prompts,
prompt history, `/quit` `/new`); 5b = markdown styling. Two PRs for
reviewability.

## Phase 0 — status analysis

- **State**: slices 1-4 merged; `locode --yolo` usable. Enter while running
  shows a hint and drops the prompt; no prompt history; no slash commands;
  assistant text is plain-wrapped.
- **Minimal next unit (5a)**: queue prompts submitted while a run is active
  (drain one per turn end), Up/Down prompt history, `/quit` + `/new`.
- **Why now**: the last interaction gaps before hardening (slice 6); all are
  quality-of-life a smoke tester reaches for immediately.
- **Prereqs**: run lifecycle + submit (slice 2), cancel (slice 3).
- **Unblocks**: slice 6 (hardening polishes a feature-complete UI).
- **Risks**: (1) `/new` must reset the engine's `Session` (owned by the engine
  task) — needs owned cli+registry in the task; (2) history nav must not
  clobber a multiline draft; (3) queue drain ordering vs continuity.

## Phase 1 — harness revisit (fresh reads 2026-07-21)

- **codex** queued input (`chatwidget/input_flow.rs:87-120`, re-read): typed
  mid-run messages queue with a preview above the composer, drained one per
  completion; editing a queued message pops it back (Alt+Up). → **Adopt**:
  UI-side `prompt_queue`, preview, drain one per turn end; Esc-at-idle pops the
  last queued back into the composer (our spec's binding).
- **grok** prompt history (`dispatch/prompt.rs:705-785`, re-read): per-session
  `Vec` capped at 200, **move-to-front dedup**, Up-arrow browse. → **Adopt**
  exactly (200 cap, move-to-front dedup).
- **codex/grok** slash: `/`-triggered; ~55 built-ins. → **Minimal v1**:
  `/quit` and `/new` only, via a small registry so adding more is additive
  (spec). Unknown slash → a notice, not a passthrough.
- **claude-code** history nav: Up/Down with cursor-at-edge gating. →
  **Adapt**: nav only when the composer is empty or already showing a recalled
  entry (a `history_nav` mode) — never clobbers an in-progress draft.

**Decisions (5a)**: UI-side `prompt_queue` (VecDeque), drain-one-per-turn-end,
Esc-pops-last; `history` (Vec, move-to-front dedup, cap 200) with a
`history_nav` cursor gated to empty/recalled; `/quit`→Quit, `/new`→NewSession
(engine rebuilds the Session; UI clears transcript state + queue, prints a
`— new session —` separator). Unknown slash → notice.

## Phase 2 — design (5a)

- `engine.rs`: owned `Cli` (add `#[derive(Clone)]`) + owned `ProviderRegistry`
  moved into the task; `UiCommand::NewSession` rebuilds via `build_session`
  and emits `EngineMsg::SessionReset` then `Ready`. `run()`/`main_with` pass
  the registry by value.
- `app.rs`: `prompt_queue: VecDeque<String>`; `history: Vec<String>`,
  `history_nav: Option<usize>`, `history_saved: Option<String>`. New
  `Cmd::NewSession`. Enter routing: slash first (`/quit`/`/new`/unknown), then
  running→queue, then submit. `record_history` on real submits. Up/Down →
  `history_prev`/`history_next` (gated). Esc-at-idle pops the last queued
  before the clear-draft path. `on_session_reset` clears transcript-adjacent
  state.
- `ui.rs`: render `prompt_queue` as dim `queued: …` lines above the composer;
  `— new session —` is a `Block::Notice`/separator via the outbox.

### Edge cases

Queue then cancel (queue preserved — next submit is the user's choice; drain
resumes on the next idle); `/new` mid-run (allowed — rebuild resets the
session; the in-flight run's late messages are dropped by the fresh session?
No — the old run finishes first; `/new` while running queues like a prompt?
Decision: `/new` is only honored at idle for v1, else a notice "finish or
cancel the run first"); history nav with a multiline draft (disabled — nav
only when single-line/empty); empty queue drain (no-op); duplicate consecutive
history (deduped).

### Test matrix / preset targets (5a)

1. [reducer] Enter while running queues (not dropped); RunFinished drains one
   (echo + Cmd::Submit); second queued waits.
2. [reducer] Esc at idle with a queue pops the last queued into the composer.
3. [reducer] history: submit records (move-to-front dedup, cap); Up recalls
   most-recent, Up again older, Down restores; nav disabled with a multiline
   draft.
4. [reducer] `/quit`→Cmd::Quit; `/new` at idle→Cmd::NewSession + state clear;
   `/new` while running→notice; unknown `/foo`→notice.
5. [integration] engine `UiCommand::NewSession` → SessionReset + Ready; a run
   after reset starts fresh (history not carried — a 1-turn mock completes).
6. [PTY smoke] queue a prompt during a run; it runs after; `/new` resets;
   `/quit` exits.
7. [gates] fmt/clippy/test/doc green.

## Phase 2 — design (5b, markdown)

- `ui/blocks.rs`: replace `AssistantText`'s `wrap_text` with a pulldown-cmark
  pass → styled `Line`s: headings bold, list items bulleted, code fences dim +
  indented, inline bold/italic/code. No syntect. Fallback to plain wrap on
  parse issues. Tests assert heading/list/code styling on sample markdown.

## Open questions for the user (non-blocking)

- `/new` restricted to idle in v1 (a notice while running). OK? (Default: yes.)
- Typed deny-feedback (deferred from slice 4) — fold into 5a? (Default: still
  deferred; not requested.)

## Result — 5a (2026-07-21)

Shipped: UI-side `prompt_queue` (queue-while-running, drain one per turn end,
Esc-at-idle pops the last back), prompt history (move-to-front dedup, cap 200,
Up/Down nav gated to single-line/empty drafts), slash commands (`/quit`,
`/new`→engine `SessionReset` + rebuild, unknown→notice); engine task now owns
cli+registry for `/new` rebuilds; `on_engine`/`on_run_finished` return Cmds so
the drain can submit.

All preset targets met: 331 workspace tests (6 new reducer tests + 1 engine
NewSession integration test). Full gates + doc green. PTY smokes on the
**release** binary: queue "second" during a slow run 1 → it runs as run 2;
`/new` → `— new session —`; `/quit` exits 0.

Deviation/lesson: a PTY smoke initially failed because it ran a STALE
`--release` binary (5a was built debug-only) — **release smokes must
`cargo build --release` first**. Recorded for future slices. `/new` restricted
to idle (notice while running) per the plan default.

Next: 5b (markdown styling of assistant text).
