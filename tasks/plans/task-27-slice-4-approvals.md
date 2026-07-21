# Task 27 / Slice 4 — approvals: TuiApprover, FIFO overlay, --yolo

Per [`docs/tui-dev-process.md`](../../docs/tui-dev-process.md). Grounding:
SPEC-TUI §The approver; study §4; ADR-0017.

## Phase 0 — status analysis

- **State**: slices 1-3 merged — runs drive, cancel works. The engine always
  uses the default `AllowAll` approver (submits auto-run every tool). `--yolo`
  parses but only lifts the path jail; it does not yet bypass approvals
  (there are none to bypass).
- **Minimal next unit**: inject a `TuiApprover` so tool calls pause for a user
  decision (Allow / Allow-for-session / Deny), rendered as a FIFO overlay;
  `--yolo` auto-allows without surfacing UI. This is the slice that makes
  `locode --yolo` the user's target smoke-test surface.
- **Why now**: last core-loop capability; slice 5 (conversation polish) and 6
  (hardening) assume it.
- **Prereqs**: `Approver`/`ApprovalRequest`/`Decision`/`Session::with_approver`
  (ADR-0017, shipped 0.1.4, facade-exported — verified); the cancel handle
  (slice 3) for "deny and stop" composition and cancel-during-approval.
- **Unblocks**: `locode --yolo` end-to-end (the user's acceptance gate);
  slice 5/6.
- **Risks**: (1) the approver runs on the engine task inside `run_text` and
  must round-trip to the UI without deadlock; (2) cancel-during-approval —
  the known ADR-0017 gap (`decide()` doesn't observe the cancel token); (3)
  oneshot senders aren't `Debug` (EngineMsg derives Debug).

## Phase 1 — harness revisit (fresh reads 2026-07-21)

- **grok** `handle_permission_request` (`acp_handler/permissions.rs:20-89`,
  re-read): request + `response_tx` oneshot; **YOLO auto-answers `AllowOnce`
  client-side** and returns without queueing; unknown session → cancel
  immediately; else enqueue FIFO, front-only render; notification rate-limited
  to empty→non-empty. Cancel/turn-end drains the queue replying `Cancelled` to
  every pending `response_tx`, restores the stashed draft
  (`dispatch/permissions.rs:222-244`). → **Adopt** wholesale at v1 scale:
  oneshot + FIFO + front-only + client-side yolo + drain-on-cancel + draft
  stash.
- **codex** (approvals report): server→client request, **typed decision enum**
  (`Accept`/`AcceptForSession`/`Decline`[continue]/`Cancel`[interrupt]); one
  modal at a time. → **Adopt** the typed-outcome shape: our UI vocabulary is
  `Allow`/`AllowSession`/`Deny{reason}`, mapped by the approver to the core
  `Decision::{Allow, Deny}` (stickiness stays approver-side per ADR-0017).
- **claude-code / opencode**: deny detected by string-matching model-facing
  prose (the anti-pattern). → **Rejected**: our denial is structural
  (`Decision::Deny{reason}` → paired `is_error` result + `denial_reason` in the
  record); nothing string-matches.

**Decisions**: `TuiApprover` holds `yolo`, a per-tool `session_allow`
`Arc<Mutex<HashSet>>` (stickiness), and the UI event sender. `decide()`: yolo →
`Allow`; tool in `session_allow` → `Allow`; else send an `ApprovalAsk` (carrying
a oneshot for the UI's `ApprovalOutcome`) and await it, mapping the outcome to
`Decision` (AllowSession also inserts into `session_allow`). Front-only overlay,
FIFO queue, draft stash/restore, drain-on-cancel. **Deferred**: typed
deny-feedback sub-mode (Deny uses "denied by user"; feedback text is polish);
`Event::Approval` rendering (the outcome already shows via tool-result pairing).

## Phase 2 — design

- New `engine/approval.rs` (or in engine.rs): `ApprovalView { tool_use_id,
  tool_name, kind, args }` (display); `ApprovalOutcome { Allow, AllowSession,
  Deny{reason} }` (UI→approver vocabulary); `ApprovalAsk { view, respond:
  oneshot::Sender<ApprovalOutcome> }` with a manual `Debug` skipping the
  sender; `EngineMsg::Approval(ApprovalAsk)`.
- `TuiApprover` (`#[async_trait]`, new `async-trait` dep — reasonable, in the
  relaxed rule): built in `build_session`, injected via `Session::with_approver`.
  `session_allow` lives entirely inside it (no threading to the loop).
- `app.rs`: `App.approval_queue: VecDeque<ApprovalView>`, `stashed_draft:
  Option<String>`. `Msg::Approval(ApprovalView)`. New `Cmd::ResolveApproval {
  id, outcome }`. `on_key`: when the queue is non-empty, non-Ctrl keys drive
  the overlay (Enter/y=Allow, a=AllowSession, d/Esc=Deny); resolving pops the
  front and restores the draft when the queue empties. Ctrl+C still
  cancels/quits (cancel drains the queue). Submit disabled while an approval
  is pending.
- `event_loop.rs`: `pending_approvals: HashMap<String, oneshot::Sender<
  ApprovalOutcome>>`. `route_engine` takes the responder out of an incoming
  `Approval` into the map, forwards `Msg::Approval(view)`. `Cmd::
  ResolveApproval` pops the sender and sends the outcome. `Cmd::CancelRun` and
  `RunFinished` drain all pending with `Deny{reason:"run cancelled"}`.
- `ui`: an overlay module replacing the composer area while `approval_queue`
  is non-empty — tool name + args + the three options + hint.

### Edge cases

Cancel while an approval is pending (drain oneshots with Deny → dispatch pairs
denied → loop-top cancel → Cancelled); quit while pending (senders dropped →
`decide()` sees a closed oneshot → Deny, safe); multiple tool calls in a batch
(queued FIFO, resolved one at a time — the engine's serial dispatch means only
one is ever pending, but the queue is general); yolo (no ask ever sent);
AllowSession then the same tool again (auto-allowed, no ask); draft stashed on
first ask, restored when queue empties.

### Test matrix / preset targets

1. [reducer] Approval enqueues + stashes draft; Allow resolves front + restores
   draft when empty; Deny resolves with reason; AllowSession resolves.
2. [reducer] Submit disabled while an approval pends; Ctrl+C still cancels.
3. [approver unit] `TuiApprover::decide`: yolo → Allow (no send); session_allow
   hit → Allow (no send); else sends an ask and maps the awaited outcome
   (Allow/AllowSession-inserts/Deny) to the right `Decision`; closed oneshot →
   Deny.
4. [integration] engine task + scripted mock emitting a tool turn, no yolo:
   assert an `EngineMsg::Approval` arrives; reply Allow via the oneshot; run
   completes and the tool ran. Then a second run with Deny: the tool is
   recorded denied (`denial_reason` set), run completes.
5. [integration] `--yolo`: a tool turn runs with NO `EngineMsg::Approval`.
6. [PTY smoke] `locode --api-schema mock` (scripted tool turn, no yolo):
   overlay appears; press `y`; run completes. And `--yolo`: no overlay.
7. [gates] fmt/clippy/test/doc green.

## Open questions for the user (non-blocking)

- Typed deny-feedback (a one-line reason field on Deny) deferred to keep the
  slice tight — the model still sees a structural denial. Add in slice 5 if
  wanted. (Default: deferred.)

## Result (2026-07-21)

Shipped: `TuiApprover` (approval.rs — yolo/session-sticky auto-allow +
oneshot round-trip), injected via `Session::with_approver`; `EngineMsg::
Approval(ApprovalAsk)` with a manual Debug (oneshot isn't Debug); reducer FIFO
`approval_queue` + draft stash/restore + overlay key handling (y/Enter allow,
a allow-session, d/Esc deny); loop-owned `pending_approvals` map with
resolve-by-id and drain-on-cancel (unblocks the ADR-0017 decide()-vs-cancel
gap approver-side); a front-only overlay replacing the composer. `--yolo`
auto-allows with NO overlay.

All preset targets met: 326 workspace tests (5 new reducer tests + 3 approval
integration tests [Allow lets the tool run, Deny records denial_reason and
continues, yolo surfaces no asks] + a sticky-set unit test). Full gates + doc
green. PTY smokes: `locode --yolo` runs a real `echo` tool with no overlay and
a completed separator; non-yolo shows `Allow run_terminal_cmd?`, `y` runs the
tool, completes.

**`locode --yolo` is now usable end-to-end** — the user's acceptance surface.

Deviations from plan: none. Deferred as planned: typed deny-feedback field
(Deny sends "denied by user"); `Event::Approval` journal rendering (the
outcome shows via tool-result pairing). New dep: `async-trait` (the approver
trait; reasonable, recorded). Next: slice 5 (conversation polish — queued
prompts, prompt history, /quit /new, markdown).
