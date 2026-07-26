# Task 33 — Mid-run user input (ADR-0028)

Source-grounded design record. Status lives in `tasks/tracker.md`.

## The constraint that shapes everything

`Session::run` takes `&mut self` and the engine task **awaits** it, so
`cmd_rx.recv()` is not polled during a run — no `UiCommand` can reach a running
session. This is the same constraint ADR-0018 hit, and its answer is the
precedent: `cancel_handle()` is cloned *before* `run()` and shared.

So the queue is a **pre-run handle**, not a command. `Session::input_queue()`
returns a clonable handle; the frontend pushes into it; the loop drains it.

## Slices

### S1 — the queue + the drain (engine)

- `locode-engine/src/queue.rs`: `InputQueue`, an `Arc<Mutex<Vec<String>>>` handle.
  `push` / `pending` / `take_all` / `clear`.
- `Session` holds one; `Session::input_queue()` clones the handle.
- `run.rs`, between dispatch and the tool-result `Message` construction:
  drained text is joined and pushed as a `ContentBlock::Text` **after** the
  `ToolResult` blocks (ADR-0028 cross-wire ordering invariant — the Responses
  lowering flushes a leading text run *ahead* of the tool outputs).
- Marker on the mid-run path only (grok's `INTERJECTION_WIRE_PREFIX` shape).
- Every `Terminal::Cancelled` break clears the queue.

### S2 — the frontend (TUI)

- `RunStarted` carries the handle alongside `cancel`.
- Submitting while `RunState::Running` pushes to the queue instead of sending
  `UiCommand::Submit`.
- The composer area renders pending items — the ADR's *visible* half.
- On `RunFinished` with items still queued, they are submitted as an ordinary
  prompt: the ADR's no-carrier fallback (grok's
  `queue_interjection_fallback_prompt`).

## Tests

- Drained text lands in the tool-result message, **after** the results.
- The marker is present mid-run and absent on the fallback path.
- Multiple queued prompts concatenate.
- Cancel drops undelivered items; delivered ones stay in history.
- A run with no tool calls leaves the item queued for the fallback.

## Result (2026-07-26)

Both slices shipped in one PR.

- `InputQueue` is a pre-run handle, exactly as the constraint analysis predicted:
  the engine task awaits `run()`, so no `UiCommand` reaches a running session.
- The drain lives in `Session::with_queued_input`, appending **after** the
  results (the cross-wire ordering invariant).
- Cancel clears on **both** cancelled paths — the iteration-top check and the
  mid-sample error — not just one.
- **Found while implementing:** "the engine queue is empty" cannot by itself mean
  "the engine took it" — it is equally true when nothing was ever pushed. Left
  that way, a prompt queued for a *later* turn would be silently dropped at the
  next turn end. `App::mid_run_pushed` counts what was actually handed over, and
  only that many entries are consumed. An existing turn-end test caught it.

## Follow-up fixes (#244, #245) — two UI bugs, one root cause

The engine half was right first time; the transcript rendering took two more
passes. Both bugs came from the same place: **the ADR asked for two visibly
distinct states, queued and delivered, and the implementation kept collapsing
them into one.**

**#244 — the prompt was invisible.** `send` skipped the transcript echo while a
run was active, relying on turn end to echo it when the queue popped and
submitted. S2 then made the delivered path submit *nothing* — which quietly
removed the only echo site. A delivered prompt appeared nowhere at all, while
the model plainly answered it.

*Lesson:* deleting a code path also deletes whatever else it was carrying. The
delivered path was removed for being redundant as a **submit**; nobody checked
what it was doing as an **echo**.

**#244 also — a delivered prompt kept rendering as queued.** `ui.rs` renders
`prompt_queue` as the pending list, and entries lived there until turn end even
though the engine had already put the text on the wire. The engine drains
mid-run and asynchronously, and *nothing tells the reducer* — so
`reconcile_delivered_input` polls on every update. Same `mid_run_pushed` gate as
above, for the same reason.

**#245 — echoed a full tool round too early.** #244's fix echoed at *queue*
time, i.e. the moment Enter was pressed. But a prompt typed during tool round 1
may not be drained until after round 2, and those are different positions in the
conversation. The transcript showed the question before the assistant turn that
*preceded* its delivery, so the model's answer looked like it arrived a turn
late.

*How it was caught:* not by reading code. The user reasoned from the rendering —
"if it had been inserted there, the next message would have answered it" — and
the session trace confirmed the text landed in tool batch 2, not batch 1.

*Lesson, and the invariant now in ADR-0028:* **a message's position in the
transcript must be the position it occupies on the wire.** For queued input that
means echoing at the point the engine *takes* it, never when the user types it.
The three delivery paths each echo exactly once, each at their own real entry
point: delivered → `reconcile_delivered_input`; never-taken → the fallback
submit; idle → `send`.

**Where to be careful in this area, for whoever extends it (P0.5 notifications
reuse all of this):**

- An empty engine queue never means "the engine took it" on its own.
- The reducer is not notified of a drain; it polls.
- Every echo site must correspond to a real wire position, and there are three.
- The trace is the ground truth for ordering questions — read
  `~/.locode/sessions/**/rollout-*.jsonl` before trusting a rendering.

## #247 — the same invariant, failed a third way

The echo landed *before* its own batch's tool cells, so the wire order
(results, then text) rendered reversed.

Cause: `reconcile_delivered_input` polled at the **top of `update()`**, while
tool cells are finalized from the tool-result message *inside* that same
update. When the carrier message arrived, the poll fired first and pushed the
prompt; the tool cells followed. Polling raced the very message it was polling
about.

Fix: stop polling. The engine appends the text **to** the tool-result message,
so the echo is driven by that message — finalize the batch's tool cells, then,
still handling the same message, echo the text it carries. Order is now correct
*by construction* rather than by timing, which is the only version of this that
stays fixed.

*Lesson:* when the data already carries the ordering, read it from the data.
Polling for "has it happened yet" reintroduces exactly the ordering question the
message had already answered. Three fixes in, the pattern is clear — every
failure here came from deriving position from something other than the wire
message itself.

A test now pins the ordering directly (`the_echo_lands_after_the_tool_cells_of_
its_own_batch`), rather than only asserting that the echo exists.
