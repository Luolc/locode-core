# SPEC-TUI — `locode-tui` + `locode-app`, the minimal interactive frontend

Status: **Implemented** (slices 1–9 + polish shipped through 0.1.8). This is the
original design spec; several v1 decisions were **superseded or extended** as the
work landed — the **Rendering model** below was replaced by a vendored terminal +
relative-frame render (ADR-0022), and two v1 non-goals (streaming, code
highlighting) shipped via ADR-0021 / ADR-0020. Those points are annotated inline;
live task status is in [`tasks/tracker.md`](tasks/tracker.md).
Grounding: [`docs/research/tui-harness-study.md`](docs/research/tui-harness-study.md)
(source study of the grok-build, codex, claude-code, and opencode TUIs).
Scope authority: ADR-0001 amendment 2026-07-21 (TUI crates live in this repo;
core crates stay headless).

## Objective

A **robust, minimal, clean** terminal UI for driving one locode session
interactively: type a prompt, watch the turn unfold, approve or deny tool
calls, cancel with a key, continue the conversation. Built as a thin client
of the `locode-core` facade over the four seams shipped in 0.1.4 — session
continuity (ADR-0016), the approval seam (ADR-0017), the cancel handle
(ADR-0018), and the event sink (ADR-0014). Every deliberate simplification
has a named extension path; nothing in v1 paints us into a corner the study
saw someone else stuck in.

**Dev style: rapid.** Thin vertical slices, each shippable and tested; the
robustness floor (terminal restore, cancel correctness, bounded buffers) is
non-negotiable from slice 1; the fancy surface (streaming deltas, themes,
mouse, multi-session) is explicitly deferred.

## Non-goals for v1

> **Two of these shipped after v1:** live streaming token deltas (ADR-0021, Task 29)
> and markdown code-block syntax highlighting (ADR-0020). The rest below still hold.

Mouse support; themes/config files; session persistence/resume; multiple
sessions/tabs; steer/interject mid-turn; Windows; ~~alt-screen or app-owned
scrollback~~ (ADR-0022 now runs a small vendored terminal for a relative-frame
render, still over native scrollback); vim mode; `@`-file mentions; image/paste chips.

## Crates & dependencies

Two new workspace crates (decided 2026-07-21, mirroring the ADR-0015
`locode-exec` lib+thin-binary pattern and the grok `pager`/`pager-bin` split):

- **`crates/locode-tui`** — a **library**: all TUI components and the
  runnable app behind one entry point
  (`locode_tui::main_with(ProviderRegistry) -> ExitCode`, the exec pattern).
  Depends only on `locode-core` (facade) among our crates.
- **`crates/locode-app`** — the product binary: **flag-free composition
  only**, a few lines calling `main_with(ProviderRegistry::builtin())`.
  Exists from slice 1 so `cargo run -p locode-app` works throughout. This is
  the assembly point where future non-TUI capability (MCP wiring, config,
  richer UX) lands; the moment feature logic appears here, push it down.
  Binary name: **`locode`** (resolved early — user 2026-07-21, "when I can
  `locode --yolo`"; bin names aren't namespaced on crates.io).

Both `publish = false` until v1 stabilizes (flipping later is non-breaking).

**`locode-tui` stays ONE crate**, with a strict module map (`term/` lifecycle,
`engine/` session driver, `app/` reducer + state, `ui/` blocks + composer +
overlay) and namespaced enums from day one. Evidence: codex holds ~221k LOC
in a single TUI crate — its regrets are module hygiene, never crate
boundaries — while grok's sibling-crate splits are third-party forks
(textarea, inline terminal) and a second render mode, neither of which we
have (stock crates, one screen mode). Split **triggers** (a rule, not a
vibe): forking/vendoring a widget crate → sibling crate; a second frontend
consuming the block renderers → extract them; a wire-protocol extraction or
material compile-time pain → extract. Until a trigger fires, modules.
- **New dependencies (ask-first list, approve with this spec):**
  - `ratatui` (stock, `crossterm` backend) — the Rust-ecosystem standard.
    *(v1 used the stock `Viewport::Inline` + `Terminal::insert_before`; ADR-0022
    later replaced this with a minimal **vendored terminal** driving a
    relative-frame render — see the Rendering model note below.)*
  - `crossterm` (bracketed paste; kitty `DISAMBIGUATE_ESCAPE_CODES` added for
    Shift/Alt+Enter — the "no kitty protocol in v1" plan was revised, TUI slice 7).
  - `tui-textarea` — multiline composer (grok forked one; codex hand-rolled;
    for minimal-robust the crate suffices, and the composer is behind our own
    `Composer` wrapper so replacing it later is local).
  - `pulldown-cmark` — assistant markdown → styled `Line`s (headings bold,
    lists bulleted, fenced code dim/indented). *(Code-block syntax highlighting
    later added `syntect` + `two-face` — ADR-0020, TUI slice 9.)*
  - Existing workspace deps reused: `tokio`, `tokio-util`, `serde_json`.

## Architecture

Three tasks/threads plus the terminal, mirroring the convergent shape of the
study (grok §2, codex §2):

```
input thread (crossterm poll+read)──mpsc──▶│                │
engine task (owns Session)────────mpsc─────▶  select! loop  │──draw──▶ terminal
  ▲ UiCommand mpsc  ▲ oneshot decisions     │  (App state)  │
  └─────────────────┴───────────────────────│◀── Cmd spawn ─┘
```

1. **Input thread** — dedicated OS thread, `crossterm::event::poll(100ms)` +
   `read()`, forwarding into an mpsc. Never poll `EventStream` inside
   `select!` (grok's crossterm-#936 waker lesson).
2. **Engine task** — a tokio task that **owns the `Session`** and loops on a
   `UiCommand` channel: `Submit(String)` → `session.run_text(...)` →
   `EngineMsg::RunFinished(Report)`. Before each run it publishes the fresh
   `cancel_handle()` (per-run token, ADR-0018). The session's `EventSink` is
   an `FnSink` pushing `Event`s into a **bounded** mpsc toward the UI
   (bounded so overload is backpressure, not memory — codex's rule). The
   `Approver` is our `TuiApprover` (below). This channel-typed seam is the
   minimal form of "protocol seam even in-process"; extracting a wire
   protocol later is a refactor of one module.
3. **UI event loop** — single `tokio::select!` (biased), arm order: quit >
   engine events (**gated on the input queue being empty, batch-drained with
   a small bound**) > input > resize debounce (~16 ms) > deferred draw >
   animation tick (scheduled only while the spinner is visible). Draw on
   state change, capped at ~16 ms; idle = zero wakeups.

**State & update.** One `App` struct; `Msg` enum (input/engine/tick) →
`update(&mut App, Msg) -> Vec<Cmd>` — sans-IO and unit-testable (grok's
dispatch discipline); `Cmd` (send UiCommand, resolve approval, quit) executed
by the loop. Keep the enums namespaced per subsystem from day one (the
1,100-line-flat-enum warning from both Rust harnesses).

## Rendering model

> **Superseded by [ADR-0022](docs/decisions/ADR-0022-vendored-terminal-relative-frame.md)
> (2026-07-22).** The inline `insert_before` / print-once model below was the v1
> design; it was replaced by a minimal **vendored terminal** driving a
> **relative-frame full re-render** of a bottom-anchored frame (the transcript
> tail + a dynamic composer), with rows overflowing the top committed to native
> scrollback. This gave the grow/shrink composer and removed the idle gap that the
> fixed inline viewport couldn't. The block model and the print-once *goal* (native
> scrollback owns finalized history) carry over; the mechanism does not. Kept below
> as the historical v1 design.

**Inline viewport, print-once transcript, one screen mode.** The terminal's
native scrollback owns finalized history; the ratatui inline viewport renders
only the **live region** (max ~a dozen rows): status row, composer, and the
approval overlay when active. Finalized blocks are converted to pre-wrapped
`Line`s and emitted once via `Terminal::insert_before`. This is grok's
Minimal mode and codex's signature, and the direct answer to Claude Code's
documented 59 GB-RSS repaint disaster. Consequences accepted for v1:
finalized blocks can't be edited (no cancel-rewind polish) and resize does
not rewrap already-printed history (codex-legacy behavior; full
reflow-from-source is the documented extension, enabled by blocks owning
their source text).

**Blocks.** `Block` enum, each owning its source and rendering to `Vec<Line>`
at a given width:

- `UserPrompt(text)` — a **full-width shaded band** (`Color::DarkGray`,
  theme-relative), `❯ `-prefixed text at col 4 with a blank shaded row of
  vertical padding above and below (grok's `RenderBlock::UserPrompt` fill +
  codex's `user_message_bg`; ADR-0019 amendment 2026-07-22). No timestamp in v1.
- `AssistantText(markdown)` — pulldown-cmark styling.
- `ToolCall { name, one_line_args, outcome }` — **one shape in v1**: a
  compact line `• run_terminal_cmd cargo test — ok (1.2s)`, with the
  result body below it truncated to N wrapped rows, head/tail kept, middle
  ellipsis (codex's row-aware truncation). Denied calls render the
  `denial_reason` from the record — structurally, never by string-matching
  model-facing text (the double anti-pattern from claude-code and opencode).
- `TurnEnd { status, turns, usage, elapsed }` — one dim separator line per
  run (`─ completed · 3 turns · 12.3k tok · 41s ─`); `cancelled` renders
  calmly, not as an error.
- `Notice(text)` / `Error(text)` — engine `Event::Error` retry notes, etc.

In v1, blocks arrived whole from `Event::Message` — no mutable tail cell. **This
shipped since:** live streaming (ADR-0021, Task 29) added a mutable live cell that
re-renders the growing buffer (with incremental markdown) each paint and commits
completed blocks to scrollback, finalized by a seamless swap on `Event::Message`.
The engine's per-message events still bound each turn.

**Status row** (only while a run is active): spinner + elapsed +
`esc to interrupt` hint; approval-waiting swaps the spinner for a distinct
"waiting on you" glyph.

## Interaction contract

| Key | Context | Behavior |
|---|---|---|
| Enter | composer, idle | submit |
| Enter | composer, running | queue (visible `QUEUED` preview above composer) |
| Alt+Enter | composer | newline (works without kitty protocol) |
| Esc | running | cancel: fire the pre-run cancel handle; status shows "cancelling…"; settle **only** when the run's `Report` arrives (`Status::Cancelled`) — never fake completion (codex), idempotent on repeat (grok) |
| Esc | approval overlay | deny the front request (reason "denied by user") |
| Esc | idle, non-empty composer | clear draft (double-press, 800 ms) |
| Up/Down | composer, empty/at-edge | in-session prompt history |
| Ctrl+C | any | first press cancels run (if any) and arms quit hint; second within window quits |
| Ctrl+D | empty composer, idle | quit |

Queued prompts drain one per turn end; Esc at idle with a queue pops the last
queued item back into the composer before clearing drafts.

Slash commands, v1 set only: `/quit`, `/new` (fresh `Session`, same config).
A registry from day one so adding commands is additive.

## The approver

`TuiApprover` implements `locode_core::Approver`:

- `decide()` packages `{tool_use_id, tool_name, kind, input}` + a `oneshot`
  sender into an `ApprovalAsk` pushed to the UI channel, then awaits the
  oneshot **`select!`-ed against the run's cancel token** — a cancelled run
  resolves the wait immediately (see "Core gap" below).
- UI holds a FIFO `VecDeque<ApprovalAsk>`; **only the front renders**, as an
  overlay replacing the composer (draft stashed on the empty→non-empty
  transition, restored when the queue empties — grok's exact flow).
- Options (typed, minimal): **Allow** · **Allow for session** (client-side
  sticky: remembered per tool name for this TUI process — ADR-0017 puts
  stickiness in the approver implementation) · **Deny** (opens a one-line
  feedback field; feedback rides `Decision::Deny{reason}` so the model sees
  it).
- `--yolo` flag: the approver auto-answers **Allow** (single-use, never
  sticky — grok's rule) without surfacing UI.
- On turn end or cancel, the queue drains: every pending oneshot resolves
  Deny with reason `"run cancelled"`, and the stashed draft is restored.

**Core gap found by this study (candidate ADR-0017 amendment, not required
for v1):** the engine's `dispatch_batch` awaits `decide()` without observing
the cancel token, so cancellation during an approval wait depends on the
approver resolving. v1 handles it approver-side (the select above). The
cleaner fix — the engine `select!`s the decide await against the token and
pairs the rest of the batch as cancellation synthetics — should land as a
small core change + ADR note when the TUI work confirms the shape.

## Robustness floor (slice-1 requirements, from the study's unanimous list)

- **Teardown byte-order defined once** (raw mode off, bracketed paste off,
  cursor shown, viewport parked with a trailing newline) and invoked from:
  normal exit, error exit, a panic hook, and the SIGINT/SIGTERM handler
  (first signal = graceful quit, second = force). SIGPIPE left ignored.
- Resize: debounce ~16 ms; relayout the live region; printed history keeps
  its old wrap (accepted).
- Bracketed paste on; paste text normalized `\r`→`\n`; pasted content goes in
  verbatim (chips deferred).
- Every channel bounded; transcript block list capped (blocks are also in
  native scrollback; the in-memory list exists for `/new`-style redraws and
  future reflow).
- Stdout is the TUI's; nothing else writes to it (workspace lint discipline
  already enforces this).

## Testing strategy

- **Reducer tests**: `update()` is pure over `App` — table-driven tests for
  the interaction contract (submit/queue/cancel/approve/deny/quit arming).
- **Block rendering**: `ratatui::backend::TestBackend` buffer assertions per
  block type and truncation rule (codex's snapshot discipline, sized down).
- **Engine-task integration**: drive the real `Session` with `MockProvider`
  scripts (text turns, tool turns via in-test registry, errors) through the
  real channels; assert the `Msg` stream and final App state — covering
  submit→blocks, cancel mid-run (slow tool), approval allow/deny/yolo, queue
  drain, `/new`.
- **Terminal lifecycle**: unit-test the teardown sequence builder; manual
  smoke checklist for real-terminal behavior (panic, Ctrl+C, resize) per
  slice.
- CI stays keyless and headless (TestBackend; no PTY needed).

## Build order (thin slices, each PR-sized and green)

> Shipped as **nine** slices, not six: slices 1–6 below, then 7 (markdown fixes),
> 8 (composer + status bar), 9 (code highlighting, ADR-0020), plus the dynamic
> composer (ADR-0022) and two-row footer polish. See [`tasks/tracker.md`](tasks/tracker.md).

1. **Shell**: both crate scaffolds (`locode-tui` lib + thin `locode-app`
   bin), terminal init/teardown/panic/signal plumbing,
   event loop, composer, quit keys. Runs and exits clean; reducer + teardown
   tests.
2. **Drive a run**: engine task + mock provider; submit → transcript blocks
   via `insert_before`; status row; `TurnEnd` separator.
3. **Cancel**: Esc → handle; cancelling state; cancelled `TurnEnd`; Ctrl+C
   two-step quit.
4. **Approvals**: `TuiApprover`, overlay, allow/allow-session/deny+feedback,
   `--yolo`, queue drain on cancel.
5. **Conversation polish**: queued prompts, prompt history, `/quit` `/new`,
   markdown styling pass.
6. **Hardening + release**: real-wire smoke (anthropic), resize/paste edge
   pass, README + installer mention, decide `publish` flip. New ADR
   (TUI architecture) accompanies slice 1; SPEC.md gets a pointer.

## Success criteria

- `locode-app --api-schema mock` runs a full scripted conversation —
  submit, tool calls rendered, follow-up turn, approval prompt honored,
  Esc-cancel produces a calm `cancelled` separator — and exits with the
  terminal perfectly restored, including after a forced panic.
- Against the real Anthropic wire, a multi-turn grok-pack session is usable
  end-to-end with approvals on and with `--yolo`.
- An idle TUI generates zero wakeups; a busy one never drops keystrokes.
- The core crates remained untouched for v1 (proving the 0.1.4 seams were
  sufficient) — the ADR-0017 amendment aside. *(Core was later extended
  deliberately for streaming: `Provider::stream` + `Event::MessageDelta`, ADR-0021.)*
