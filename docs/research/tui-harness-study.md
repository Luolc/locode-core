# TUI harness study — how the four CLIs build their interactive terminal UIs

Source study of the TUI layers of **Grok Build** (`xai-grok-pager`), **Codex**
(`codex-rs/tui`), **Claude Code** (React/Ink), and **opencode**
(`packages/tui`, SolidJS/OpenTUI), conducted 2026-07-21 against the
`coding-cli-survey` submodules. Citations are `harness: path:line`, relative
to each submodule root. This document feeds [`SPEC-TUI.md`](../../SPEC-TUI.md).

Method: one deep source read per harness covering stack, architecture, turn
lifecycle, approvals, interrupt, transcript rendering, input editor, chrome,
robustness, and complexity; then cross-comparison below.

---

## 1. Per-harness profiles

### Grok Build — `xai-grok-pager` (Rust, ratatui + crossterm)

The largest and most disciplined of the four: **414 files / ~384k LOC** in the
pager crate alone (plus sibling crates: render 35k, minimal-mode 5.6k, a
textarea fork 10.4k, an inline-terminal fork 3k).

- **Agent in-process on a dedicated thread, speaking ACP over in-memory
  channels** — protocol-shaped without process isolation; subprocess/remote
  modes reserved (`grok: src/acp/spawn.rs:3-4,100-121`).
- **Three screen modes** (`grok: src/app/mod.rs:226-240`): Fullscreen
  (alt-screen, app-owned 49k-LOC scrollback pane), Inline, and **Minimal** —
  finalized blocks printed once into native scrollback via `insert_before`
  with a small pinned live region. Supporting both retained and print-once
  transcripts taxes unrelated features (cancel-rewind, recap re-print special
  cases; `grok: src/app/dispatch/turn.rs:208-226`).
- **Event-driven redraw, no fixed FPS**: paint on state change, throttled to
  16 ms during streaming; animation ticks scheduled *only while something
  animates* — an idle app parks with zero wakeups (`grok:
  src/app/event_loop.rs:1723-1737,1174-1176`).
- **Input on a dedicated OS thread** (`poll(100ms)+read` → mpsc), explicitly
  because dropping crossterm's `EventStream::next()` future inside `select!`
  strands its waker (crossterm #936) (`grok: src/app/event_loop.rs:1084-1157`).
- **One biased `select!`** with load-bearing arm order: the ACP stream arm is
  *gated on the input queue being empty* and batch-drains at most 32 messages,
  so a token firehose can't starve keystrokes (`grok:
  src/app/event_loop.rs:1685-1738`).
- **Unidirectional reducer**: `Action → dispatch(&mut App) → Vec<Effect>`
  ("dispatch stays sans-IO"), effects run on a `JoinSet` and return as
  `TaskResult` back through dispatch (`grok: src/app/actions.rs:1-8`).
- **Permissions**: request + `oneshot` response, FIFO `VecDeque` per agent,
  front-only rendering, client-side YOLO auto-answering `AllowOnce` (never
  `AllowAlways`), turn end/cancel drains the whole queue with `Cancelled`
  replies and restores the stashed draft. Whole mechanism ≈700 lines (`grok:
  src/app/acp_handler/permissions.rs:20-89,143-150`).
- **Ctrl+C cancels; Esc does not** (Esc is overloaded: overlays, clear-draft,
  rewind picker). Cancel is idempotent and retryable, with a 2 s
  turn-end-reconciliation watchdog because a lost turn-end RPC once bricked
  the UI (`grok: src/app/dispatch/turn.rs:68-95,280-330`).
- **Transcript**: typed `RenderBlock` enum + `BlockContent` trait with
  Collapsed/Truncated/Expanded display modes; streaming state lives in a
  separate tracker object that maps ACP updates to block mutations (`grok:
  src/scrollback/block.rs:363-388`, `src/acp/tracker.rs:1-5`).
- **Teardown byte-order defined exactly once** and shared by clean exit,
  error path, panic hook, and signal task; `EndSynchronizedUpdate` first so
  multiplexers stop buffering, alt-screen leave last; frame-writer thread
  drained before teardown (`grok: src/app/mod.rs:1185-1263,1280-1291`).
  SIGPIPE deliberately left `SIG_IGN` so children don't inherit a flipped
  disposition (`grok: src/app/signal_handler.rs:16-22`).

### Codex — `codex-rs/tui` (Rust, ratatui + crossterm, forked)

**368 files / ~221k LOC**, snapshot-tested to an unusual degree (575 `.snap`
files). The signature move is the **inline viewport with scrollback-native
history**.

- **No alt-screen for the main UI**: a custom `Terminal` keeps a mutable
  viewport rect anchored at the cursor row (`codex:
  tui/src/custom_terminal.rs:146-168,236-241`); finalized history is an
  *escape-sequence operation*, not a render — a DECSTBM scroll region is set
  over the rows above the viewport and pre-wrapped lines are printed so only
  that region scrolls into native scrollback (`codex:
  tui/src/insert_history.rs:1-4,221-243`). Depends on nornagon
  ratatui/crossterm forks for the scrolling-region backend surface (`codex:
  codex-rs/Cargo.toml:551-556`).
- **The viewport frame renders only the active (in-progress) cell + bottom
  pane**; finalized cells leave the widget tree entirely (`codex:
  tui/src/chatwidget/rendering.rs:6-60`).
- **Demand-driven frames**: a coalescing `FrameScheduler` actor with a 120 fps
  clamp; animations self-schedule; zero idle wakeups (`codex:
  tui/src/tui/frame_requester.rs:35-128`).
- **Protocol seam even in-process**: the TUI speaks JSON-RPC (`turn/start`,
  `turn/steer`, `turn/interrupt`, `thread/fork`; server→client *requests* for
  approvals) to an app-server running in the same process — the TUI is a pure
  reference client of the same API other frontends use (`codex:
  app-server-client/src/lib.rs:88-94`).
- **Streaming**: markdown committed **only up to the last newline**; a stable
  region queues for scrollback commit while a mutable tail stays live; whole
  in-progress tables held back because a new row re-shapes columns (`codex:
  tui/src/markdown_stream.rs:87-104`, `streaming/controller.rs:1-37`). Final
  messages are consolidated into one source-backed cell that can re-render
  from markdown source on resize (`codex: history_cell/messages.rs:365-407`);
  the resize-reflow path re-emits the whole transcript re-wrapped, 75 ms
  debounced (`codex: tui/src/app/resize_reflow.rs:1-15`).
- **Esc interrupts** (`turn/interrupt`), and the UI settles only on
  `turn/completed{Interrupted}` — never fake completion client-side; the
  active cell finalizes as failed (`codex: tui/src/chatwidget/
  turn_runtime.rs:311-339`). **Double-Esc = edit-previous via server-side
  `thread/fork`**, not client transcript surgery (`codex:
  tui/src/app_backtrack.rs:10-22`). Ctrl+C: first press interrupts and arms a
  quit hint; second quits (`codex: tui/src/chatwidget/interaction.rs:360-414`).
- **Queued input + steer**: messages typed mid-turn queue with a preview;
  `turn/steer` is a first-class protocol op with race retry; rejected steers
  auto-resubmit (`codex: tui/src/chatwidget/input_queue.rs:21-45`).
- **Approvals**: server→client requests with ids and **typed decision enums**
  (`Accept`, `AcceptForSession`, `AcceptWithExecpolicyAmendment`, `Decline`
  ["turn continues"], `Cancel` ["turn interrupts"]); one modal at a time on a
  view stack; remote resolution dismisses local prompts (`codex:
  app-server-protocol/src/protocol/v2/item.rs:60-79`).
- **Restore paranoia, three layers**: panic hook, Drop guard, and a
  hard keyboard reset (`\x1b[<u`) for terminals that missed the kitty pop;
  termios resync after SIGCONT; the stdin `EventStream` is dropped and
  recreated around external editors because merely not polling it still
  steals stdin (`codex: tui/src/tui.rs:298-313,504-510`,
  `tui/src/tui/event_stream.rs:10-18`).
- **Composer**: custom textarea with atomic `TextElement` placeholder ranges
  (paste chips >1000 chars, mentions), a `PasteBurst` timing heuristic for
  terminals without bracketed paste, single-entry kill buffer, **no undo** —
  proof a composer can feel first-class without one (`codex:
  tui/src/bottom_pane/textarea.rs:92-135`, `paste_burst.rs:154-165`).

### Claude Code — React/Ink (TypeScript)

The cautionary tale. A vendored Ink fork is effectively a terminal compositor
(yoga layout, cell-diffing, bidi, hit-testing); `REPL.tsx` is a 5,006-line god
component.

- **The transcript lives in the repaint region**, and everything bad follows:
  the diff renderer cannot repaint rows that scrolled off-screen, so those
  diffs force a function literally named `fullResetSequence_CAUSES_FLICKER`
  (`claude-code: src/ink/log-update.ts:146,216-219`). The long-transcript
  post-mortem is written in the code: *"~250 KB RSS per message fiber tree…
  at ~2000 messages… GC death spiral (observed: 59 GB RSS)"*, fixed across
  three ticket generations ending in UUID-anchored render windows
  (`claude-code: src/components/Messages.tsx:277-309`).
- **The stateless `query()` generator is the good bone**: a `while(true)` loop
  over an explicit `State` record, fed fresh state each turn by the UI; the
  engine stays a library (`claude-code: src/query.ts:204-279`).
- **The `canUseTool` Promise seam** is shape-correct (engine awaits a promise
  the dialog resolves) but the decision rides **model-facing prose**: denial
  is `REJECT_MESSAGE` text, and the renderer *prefix-matches that prose back
  out of the transcript* to decide how to render (`claude-code:
  src/utils/messages.ts:207-215` ↔
  `src/components/messages/UserToolResultMessage.tsx:50`). Same for
  `INTERRUPT_MESSAGE` (`src/components/messages/UserTextMessage.tsx:83`).
- **Tools own React render methods** — seven of them on the `Tool` type the
  engine dispatches on (`renderToolResultMessage`, `renderToolUseMessage`, …
  `claude-code: src/Tool.ts:566-694`) — the anti-pattern our ADR-0003/0008
  already reject, here shown metastasized (plus UI-only members like
  `extractSearchText` and a permission-dialog switch on tool identity with a
  TODO planning to move dialogs *into* the tool, `claude-code:
  src/components/permissions/PermissionRequest.tsx:47-82,145`).
- **Ref-mirror epidemic**: parallel refs for messages, abortController,
  streamMode, inputValue — the cost of running an event loop inside a
  rendering framework's stale-closure model (`claude-code:
  src/screens/REPL.tsx:1182-1222`).
- Genuinely good ideas worth stealing: **generation-counted query guard**
  (kills cancel+resubmit races, `REPL.tsx:2866-2923`); **queue-while-busy
  with mid-turn drain** (queued prompts surface to the model *between tool
  batches*, `src/query.ts:1547-1590`); **preserve partial output on
  interrupt** and **auto-restore the prompt on fast interrupt**
  (`REPL.tsx:2121-2129,2996-3022`); **replace, don't append, ephemeral
  progress ticks** (a 13k-message array of `sleep_progress` taught them,
  `REPL.tsx:2608-2627`); single dialog-focus arbiter over N queues
  (`REPL.tsx:2017-2065`); terminal restore with raw `writeSync` *outside*
  React under a timeout (`src/utils/gracefulShutdown.ts`).

### opencode — `packages/tui` (TypeScript, SolidJS + OpenTUI)

**~27k LOC** — the smallest, because the server does the heavy lifting.

- **Client/server without a socket**: the TUI is written against an
  HTTP+SSE API but by default the server runs in a Bun worker and fetch is
  shimmed to worker RPC — remote-readiness without local latency (`opencode:
  packages/opencode/src/cli/cmd/tui.ts:239-251`).
- **One flat store, ID-ordered arrays, binary-search upsert** — event-sourced
  UI state with idempotent upserts keyed by sortable ids (`opencode:
  packages/tui/src/context/sync.tsx:41-52`); SSE micro-batched at 16 ms into
  single render batches (`opencode: packages/tui/src/context/sdk.tsx:54-80`).
- **Owned scrollback** (`<scrollbox>` sticky-bottom): buys rich in-transcript
  interactivity (folding, click-into-subagent) at the cost of reimplementing
  selection/copy and giving up native scrollback.
- **Permissions as server-owned data**: `permission.asked` events + a reply
  endpoint (`once | always | reject` + optional corrective message); "always"
  shows the exact patterns to be persisted before confirming; **auto/YOLO
  mode is purely client-side** — the sync layer replies `once` immediately
  (`opencode: packages/tui/src/context/sync.tsx:190-199`).
- **The typed-error erasure anti-pattern**: rejection is a typed error
  server-side, but the runner persists tool failures as
  `{type: "unknown", message}` — so the TUI must **substring-sniff the error
  string** (`error()?.includes("rejected permission")`…) to render denied
  vs failed (`opencode: packages/core/src/session/runner/
  publish-llm-event.ts:44-47` ↔
  `packages/tui/src/routes/session/index.tsx:1857-1863`). Same lesson as
  Claude Code, from the opposite direction: keep the tag.
- **Interrupt**: double-Esc armed with a 5 s auto-disarm → abort endpoint;
  server fails unsettled tools with explicit events so the transcript stays
  consistent; aborted messages render as calm "interrupted" metadata, not
  errors (`opencode: packages/tui/src/component/prompt/index.tsx:391-420`).
- **Two tool render shapes** (inline one-liner vs bordered block) + per-tool
  pending verbs + collapse-by-default covers every tool with ~15 small
  components (`opencode: packages/tui/src/routes/session/index.tsx:1702-1782`).
- DB-backed sessions give resume/fork/share for free; the client caps memory
  at 100 messages/session because the DB is the source of truth.

---

## 2. Comparison matrix

| Dimension | Grok Build | Codex | Claude Code | opencode |
|---|---|---|---|---|
| Stack | ratatui+crossterm (2 in-house forks) | ratatui+crossterm (nornagon forks) | vendored Ink fork (React/yoga) | SolidJS + OpenTUI |
| TUI size | ~384k LOC | ~221k LOC | (mixed into app; REPL 5k + ink fork) | ~27k LOC |
| Core ↔ UI seam | ACP in-process (dedicated thread) | JSON-RPC app-server in-process | direct library calls + React state | HTTP+SSE (worker-shimmed) |
| Transcript home | 3 modes; Minimal = native scrollback via `insert_before` | native scrollback via scroll-region writes | repaint region (the disaster) | app-owned scrollbox |
| Render cadence | event-driven, 16 ms cap, parks idle | demand-driven, 120 fps clamp, parks idle | 16 ms frame interval | 60 fps target + reactive |
| Input source | dedicated OS thread → mpsc | crossterm `EventStream` behind drop/recreate broker | Ink stdin handling | OpenTUI events |
| State model | `Action → dispatch → Vec<Effect>` reducer | AppEvent channel + ChatWidget owns derived state | React state + ref mirrors | Solid store, binary-search upserts |
| Cancel key | **Ctrl+C** (Esc overloaded) | **Esc** (Ctrl+C = interrupt+arm-quit) | **Esc** (Ctrl+C interrupt) | **double-Esc** (armed, 5 s) |
| Cancel settles | server ack + 2 s watchdog | only on `turn/completed{Interrupted}` | AbortController + local settle | abort endpoint + server events |
| Approvals | oneshot + FIFO queue, front-only render | server→client request, typed decision enum, modal stack | Promise seam + dialog queue (prose-coded) | asked/reply events (`once/always/reject`) |
| YOLO | client-side auto-`AllowOnce` | policy in turn params | policy modes + classifier | client-side auto-`once` |
| Queued input | server-authoritative queue + interject + send-now | queue + first-class `turn/steer` | queue + mid-turn drain as attachments | queue badge + `delivery: steer\|queue` |
| Streaming display | tracker maps chunks → live block mutation | newline-commit; stable/tail split | line-by-line streamingText | `{field, delta}` append events |
| Deny channel | RejectOnce + `meta.followup_message` | `Decline` vs `Cancel` typed | prose `REJECT_MESSAGE` (string-matched) | typed error erased → string-sniffed |
| Editor | textarea fork, chips, history 200 | custom textarea, chips, no undo | 2.3k-line PromptInput + vim | textarea + extmarks + $EDITOR |
| Panic/restore | teardown sequence defined once, shared | 3-layer restore + termios resync | writeSync outside React + timeout | Effect acquireRelease + SIGHUP |

## 3. Convergent patterns (what everyone does)

1. **Settled transcript out of the hot repaint path.** Codex and grok-minimal
   print finalized blocks once into native scrollback; opencode caps and
   virtualizes; Claude Code is the counterexample that proves the rule (59 GB
   RSS). The live region is small: active block + status + composer + modal.
2. **Approval = a data request the UI resolves later** (oneshot / Promise /
   reply-endpoint), FIFO-queued, **one rendered at a time**, with
   YOLO/always-allow implemented **client-side** as auto-answering the
   single-use option. Nobody blocks the render loop on a decision.
3. **Cancel is asynchronous and idempotent**: fire the interrupt op, keep
   showing "running/cancelling", settle only on the authoritative terminal
   event, and guard with a watchdog/retry (grok's 2 s reconcile; codex's
   settle-on-`Interrupted`). Everyone preserves partial work; nobody discards
   the conversation.
4. **Typing never blocks**: input stays enabled during a run; submissions
   queue with a visible badge/preview and drain at turn end (mid-turn
   steer/interject is the deluxe version).
5. **Event-driven rendering with a frame cap and zero idle wakeups** (grok
   removed its always-on 30 Hz metronome; codex's scheduler clamps at 120 fps;
   opencode micro-batches events at 16 ms).
6. **Typed block/cell/part enum per transcript entry**, each owning enough
   source to re-render, with collapse/truncate defaults and per-tool
   presentation (two shapes suffice: inline line vs bordered block).
7. **Terminal restore never trusts the framework**: panic hook + signal path
   + drop guard sharing one teardown sequence; raw-mode/alt-screen/kitty
   restored with direct writes.
8. **Editor table stakes**: multiline textarea, Enter submits /
   modifier-Enter newline, bracketed paste with `\r`→`\n`, large-paste
   collapsed to a chip, Up/Down history, slash commands, draft
   stash/restore around modal flows.

## 4. Divergences and what they teach

- **Native scrollback vs owned scrollback** is the deepest fork. Native
  (codex, grok-minimal) is cheap, flicker-free, and keeps the terminal's own
  selection/search — but blocks can't be un-printed (grok's cancel-rewind
  special cases) and resize-rewrap needs source-backed cells (codex reflow).
  Owned (grok-fullscreen, opencode) buys folding/selection/search overlays at
  5–50k LOC. **Grok is the only one paying for both, and it shows.**
- **Esc vs Ctrl+C for cancel**: majority Esc-cancels (codex, claude;
  opencode double-Esc); grok reserves Esc for overlays/clear/rewind and uses
  Ctrl+C. Either works; what matters is the two-step guard against
  accidental quits and the idempotent retry.
- **Protocol seam depth**: codex proves a full JSON-RPC seam in-process keeps
  the TUI a pure client; grok gets the same via ACP types over channels;
  opencode via HTTP; Claude Code has no seam and pays with React-in-the-
  engine. For locode the engine facade *is* the seam — typed channel messages
  suffice; a wire protocol is a later extraction, not a v1 need.
- **Streaming granularity**: char-deltas (claude), field-deltas (opencode),
  newline-committed markdown (codex). Codex's rule — never render a partial
  markdown line; keep a stable/tail split — is the one that composes with
  native scrollback.

## 5. Anti-pattern catalog (with receipts)

1. **Tools that render** — 7 React methods on the engine's `Tool` type
   (`claude-code: src/Tool.ts:566-694`). Presentation must live in a
   TUI-side registry keyed by tool name/kind.
2. **UI semantics encoded in model-facing strings** — `REJECT_MESSAGE` /
   `INTERRUPT_MESSAGE` prefix-matched by renderers (`claude-code:
   src/components/messages/UserToolResultMessage.tsx:50`); typed rejection
   erased to `{type:"unknown"}` then string-sniffed (`opencode:
   packages/tui/src/routes/session/index.tsx:1857-1863`). locode already has
   the structural fields (`denial_reason`, `Status::Cancelled`) — keep it
   that way.
3. **Transcript in the repaint region** (`claude-code:
   src/components/Messages.tsx:277-309`).
4. **Polling crossterm's `EventStream` inside `select!`** — waker-strand bug
   masked for months by an always-on tick (`grok:
   src/app/event_loop.rs:1084-1092`).
5. **Dual transcript modes** (print-once + retained) leaking special cases
   into unrelated features (`grok: src/app/dispatch/turn.rs:208-226`).
6. **A single flat mega-enum for app events** — codex's `AppEvent` at 1,109
   lines; grok's Action/Effect/TaskResult at 2,768 lines combined. Namespace
   early.
7. **Boolean `isLoading`** — use a generation-counted guard
   (`claude-code: src/screens/REPL.tsx:2866-2923`).
8. **Unbounded anything**: progress ticks appended instead of replaced
   (120 MB transcripts), fibers per message, unbounded channels between core
   and UI. Codex bounds core↔UI channels so overload surfaces as
   backpressure, not memory (`codex: app-server-client/src/lib.rs:13-16`).

## 6. Distilled best practices for locode-tui

The v1 spec ([`SPEC-TUI.md`](../../SPEC-TUI.md)) is built on these:

1. Inline viewport + `insert_before` print-once transcript (native
   scrollback); live region = status + composer + overlay only. One mode.
2. Dedicated input thread → mpsc; one biased `select!`; engine-event arm
   gated on empty input queue with bounded batch drain.
3. Event-driven draw with a ~16 ms cap; animation tick only while animating;
   idle = zero wakeups.
4. `Msg → update(&mut App) → Vec<Cmd>` reducer, sans-IO, unit-testable;
   effects on spawned tasks feeding results back as `Msg`.
5. Approver = oneshot + FIFO queue + front-only overlay; YOLO client-side;
   queue drained (denied-as-cancelled) on turn end/cancel with draft restore.
6. Esc cancels (idempotent; settles on the engine's `cancelled` report);
   Ctrl+C two-step quit; typed state machine, not booleans.
7. Typed `Block` enum owning its source text; two render shapes; truncate
   tool output by wrapped rows with head/tail keep.
8. Teardown sequence defined once; panic hook + signal handler + Drop guard
   all call it; bracketed paste on; resize debounced.
9. Bound every queue and cap every buffer from day one.
10. Keep typing enabled mid-run; queue with visible preview; drain one per
    turn end; Esc/edit pops the queue back into the editor.
