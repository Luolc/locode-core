# ADR-0028: Mid-run user input — one queue, iteration-granular drain, visible state

## Status

**Draft — not approved.** Scheduled ahead of ADR-0027; the background-task
notification work (P0.5) is designed to reuse the queue this introduces.

## Date

2026-07-26

## Scope

What happens when the user submits a prompt while a run is in flight. Covers the
queue, where it drains into the loop, what the UI shows, and what cancel does.

Explicitly **not** a cancel-and-resubmit: the running turn is never aborted. It
is also not compaction, subagents, or background bash — but §Decision 5 records
the seam those reuse.

## Context

Today a prompt submitted mid-run waits for the entire loop to finish. All three
studied harnesses do better, and — contrary to the assumption this study started
from — **all three support it**, including Grok Build. They differ in *when* the
queue drains, and that single difference explains the UX gap the user observed.

**Claude Code — drains per loop iteration, into the tool-result batch.**
`query.ts:1580-1590`, after tool results are collected and before re-sampling:

```ts
for await (const attachment of getAttachmentMessages(..., queuedCommandsSnapshot, ...)) {
    yield attachment
    toolResults.push(attachment)   // rides the existing User message
}
```

The queue is a process-global singleton carrying two payload kinds —
`mode:'prompt'` (a typed message) and `mode:'task-notification'` — drained at the
same point with different addressing: prompts reach the main thread only; a
subagent drains only notifications stamped with its own `agentId` and never sees
the prompt stream (`query.ts:1565-1578`). Slash commands are deliberately
excluded and handled after the turn ends. Priority is two-level
(`getCommandsByMaxPriority('next' | 'later')`).

**Codex — drains per turn.** `tasks/regular.rs:88-91`:

```rust
run_turn(...).await?;
if !sess.input_queue.has_pending_input(&sess.active_turn).await {
    return Ok(last_agent_message);
}
next_input = Vec::new();   // queued input exists → run another turn
```

Queued items are folded into the next turn's input at turn start
(`tasks/mod.rs:353-362`). Coarser: the user waits for the current turn's whole
tool sequence.

**Grok Build — drains mid-turn, without cancelling.** `SessionCommand::Interject`
(`commands.rs:669-672`) — *"Inject a user message into the active turn without
canceling it. The text is queued in `pending_interjections` and drained at the
next safe point in `process_conversation_turn`."* If no turn is running it
degrades to an ordinary prompt (`run_loop.rs:739-742`), and cancel clears the
buffer (`tasks_cancel.rs:215`).

**The UX difference follows from the granularity, not from UI effort.** Claude
Code's injection lands within one tool round, so there is little to show and its
UI is correspondingly quiet. Codex's user can wait out an entire turn, so it
ships a dedicated `pending_input_preview` pane — a *"Queued follow-up inputs"*
section plus an *"edit last queued message"* affordance bound to Alt+Up /
Shift+Left (`bottom_pane/pending_input_preview.rs:137,161`,
`keymap.rs:952`). Codex's UI is clearer **because its wait is longer**.

## Decision

**1. Claude Code's granularity, Codex's honesty.** Drain at the **loop-iteration**
boundary — after dispatch, before re-sample — appending to that iteration's
tool-result batch. And show the queued state explicitly anyway.

Taking the granularity from Claude Code and the visibility from Codex is not
splitting the difference: the two are independent axes, and the pairing each
harness shipped is a local optimum, not a principle. Short waits do not make a
queued message *less* worth showing — a user who typed into a running agent
should be told what will happen to their words, even if the answer is "in a few
seconds".

**2. Ride the existing `User` message; never insert a bare one.** The drained
items are appended to the tool-result batch the loop already builds. This keeps
ADR-0004's pairing invariant intact (no message without a purpose), and does not
introduce a new prompt-cache prefix boundary.

**3. When there is no batch to ride, it becomes the next prompt.** An iteration
that emits no tool calls ends the loop, so a queued item arriving then has no
carrier. It degrades to an ordinary next-turn prompt — Grok's
`queue_interjection_fallback_prompt` path. Stating this is load-bearing: it is
the case that otherwise manifests as "my message was silently swallowed".

**4. Cancel clears the queue.** Following Grok (`tasks_cancel.rs:215`). A user
who cancels a run has changed their mind about the context the queued message
was written for; delivering it into the next turn would be surprising. The UI
must say so rather than silently dropping it.

**5. One queue, addressed payloads.** The queue carries a payload enum, not just
strings — `Prompt` today, `TaskNotification` when P0.5 lands, addressed so a
future subagent drains only its own notifications and never the user's prompt
stream. This is the whole reason this ADR is sequenced first.

**6. Slash commands do not queue-jump.** They are frontend actions, not model
input; a `/model` typed mid-run applies to the session, and injecting its text
into the transcript would be nonsense. Matches Claude Code's explicit exclusion.

### Engine / core changes

- `locode-protocol`: a `QueuedInput` payload enum. New `Event` variants for
  `InputQueued` / `InputDelivered` so a frontend can render state transitions
  without inventing its own bookkeeping (ADR-0014's enum is `#[non_exhaustive]`).
- `locode-engine`: a queue on `Session` with a `push` seam the frontend calls;
  one new step in `run.rs` between dispatch and re-sample that drains into the
  result batch; the no-tool-calls fallback; the cancel clear.
- The queue is **engine-owned, frontend-fed** — the same shape as the approval
  seam (ADR-0017): the engine never reads a terminal, the frontend never edits
  history.

### UI

- A pending-input area modelled on Codex's `pending_input_preview`: the queued
  text, and *what it is waiting for*.
- Two visibly distinct states — **queued** ("will be sent when the current tool
  call finishes") and **delivered** (it appears in the transcript as a user
  message). The user's complaint about Claude Code is precisely that these are
  indistinguishable.
- Editing the last queued message before it lands (Codex's
  `edit_queued_message`) is desirable and **deferred** — the value is in the
  visible state first.

## Alternatives Considered

### Codex's turn granularity
Rejected. Simpler to implement (one check at turn end, no interaction with the
tool-result batch), but it makes the user wait out an arbitrarily long tool
sequence for no reason. Its clearer UI is a *consequence* of that weakness, not
a reason to adopt it.

### Cancel-and-resubmit
Rejected. Trivially implementable today and semantically wrong: it throws away
completed tool work the user did not ask to discard, and it is not what any
harness does.

### Grok's "next safe point" without a defined carrier
Rejected as under-specified for us. Grok's safe points are internal to its own
turn machinery; our loop has one obvious carrier (the tool-result batch), and
naming it makes the invariant checkable.

## Consequences

- The loop grows one step; the serial path is otherwise untouched.
- P0.5's task notifications become a payload variant rather than a second
  mechanism — the stated reason for doing this first.
- ADR-0004's pairing invariant needs a test that a drained input never produces
  an unpaired message.
- A queued message changes what the model sees mid-run, so a run's transcript is
  no longer a pure function of the initial prompt. For eval reproducibility this
  is fine (evals do not type mid-run) but it should be stated.

## Open Questions

- **Ordering against a same-iteration notification.** When a prompt and a task
  notification drain together, which comes first? Claude Code's two-level
  priority suggests notifications first (context before instruction), but this
  is unverified.
- **Multiple queued prompts.** Deliver all in one batch, or one per iteration?
  Codex delivers all; batching risks the model conflating two instructions.
- **Should the model be told the message arrived mid-run?** A bare user message
  reads as if it were there all along. A `<mid-run>` marker would be honest but
  costs a divergence from the plain-prompt shape.
