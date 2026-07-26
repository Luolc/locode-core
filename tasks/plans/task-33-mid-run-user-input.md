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
