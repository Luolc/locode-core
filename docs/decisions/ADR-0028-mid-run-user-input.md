# ADR-0028: Mid-run user input — one queue, iteration-granular drain, visible state

## Status

**Accepted** (user, 2026-07-26). Scheduled ahead of ADR-0027; the background-task
notification work (P0.5) is designed to reuse the queue this introduces.

**Implementation note.** The queue is a **handle taken before the run**, not a
`UiCommand`. `Session::run` takes `&mut self` and the engine task awaits it, so
its command loop is not polled mid-run — the same constraint that made
`cancel_handle()` a pre-run clone (ADR-0018). `Session::input_queue()` follows
that established shape exactly.

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

## Cross-wire validity (verified 2026-07-26)

"Riding the tool-result batch" is a **neutral-protocol** statement, not an
Anthropic one. Our `Message { role: User, content: Vec<ContentBlock> }` may hold
`ToolResult` and `Text` blocks together; each wire lowers that shape its own way,
and both of ours already do:

- **Anthropic Messages** — one `user` message carrying both block kinds. Native.
- **OpenAI Responses** — a flat item array, so the two simply become adjacent
  items. `openai/responses/build.rs:90-97` already handles the mix: a
  `ToolResult` block *flushes any pending text/image run first*, emitting the
  text as its own item before the `function_call_output`.

**This makes block order load-bearing, and the invariant is: append the drained
text AFTER the tool-result blocks.** On Responses, text placed *before* them
would flush into an item that precedes the tool outputs — the user appearing to
speak before the tools reported. Claude Code's `toolResults.push(attachment)`
appends, so the natural implementation is already correct; it is written down
here because the Responses lowering makes it silently wrong the other way round,
and its own comment already warns that "order is load-bearing for the prefix
cache".

**OpenAI Chat is the shape that would not carry it** — a `role:"tool"` message
takes only the tool output, so a mid-run prompt must lower to the `tool` messages
plus a separate `role:"user"` message. We ship no Chat wire (`anthropic`,
`openai-responses`, `mock`), so this costs nothing today; it is recorded because
it proves the carrier must stay a protocol concept that wires lower, never a
literal "one message" rule baked into the engine.

## Resolved (user decisions + source, 2026-07-26)

**Ordering of a prompt against a same-iteration notification — FIFO by arrival.**
Claude Code has no notification-vs-prompt ranking. `getCommandsByMaxPriority`
(`messageQueueManager.ts:525-532`) filters `PRIORITY_ORDER[cmd.priority ??
'next'] <= threshold` over the queue array, preserving insertion order; both
kinds default to `'next'`. The `'later'` tier is an *idle* axis — the drain asks
for it only when a Sleep tool ran (`query.ts:1570`), i.e. "deliver when nothing
is happening". We copy this: one FIFO queue, arrival order, with `'later'`
reserved unused.

**Multiple queued prompts — concatenate into one.** The common case is a user
typing several lines and hitting Enter, not issuing competing instructions.
Joining them into a single text block avoids presenting the model with what looks
like two separate turns.

**The model IS told the message arrived mid-run.** Grok already does exactly
this, and its constant is the phrasing:

```rust
const INTERJECTION_WIRE_PREFIX: &str = "The user sent a message while you were working";
```

Its PTY tests assert the distinction precisely: a genuine mid-turn interjection
carries the preamble, while a queued message that lands as an ordinary next-turn
prompt is a standard `<user_query>` with **no** preamble. Claude Code does the
same to its own agent — this session received, verbatim: *"The user sent a new
message while you were working … This is how Claude Code surfaces messages the
user sends mid-turn — within the running turn, often alongside the next tool
result."* First-hand confirmation of both the marker and the carrier.

The rationale is semantic, not cosmetic: a mid-run message is *not* the same
speech act as a fresh prompt. It may amend an earlier instruction, be a
by-the-way aside, or be an instruction for after the current work ("when you
finish this, also…"). The model can only act correctly — including changing
course *within* the running loop — if it knows which it is. An unmarked message
reads as though it had been there from the start.

The marker applies **only** to the mid-run path. A queued message that degrades
to the next turn's prompt (§Decision 3) is an ordinary prompt and carries none.

## Remaining edge cases (from the state machine, to settle in implementation)

- **Delivered cannot be recalled.** §Decision 4's "cancel clears the queue"
  covers only *undelivered* items. Once drained, the text is inside a `Message`
  already pushed to `history`; cancel breaks the loop but rolls nothing back, so
  the model sees it next turn. The asymmetry is fine but must be stated, or an
  implementer will assume cancel undoes both.
- **A cancelled prompt stays in history — today, undecided.** `drive()` pushes
  the user message *before* the loop (`run.rs:178`), and a mid-sample cancel
  appends no assistant message ("the history is unchanged since the last
  append", `run.rs:202-204`). So a cancelled turn leaves a trailing user
  message, and the next submit appends another: `[…, user: p1, user: p2]`, both
  visible to the model. The API permits consecutive same-role messages, so
  nothing errors — but "cancel the execution, keep the intent" is currently an
  accident rather than a decision. Options: keep it, pop the message on cancel,
  or keep it with a cancelled marker.

## Open Questions

None outstanding — the three original questions are resolved above.

## Amendment (2026-07-26): transcript position must equal wire position

Implementing §Decision 1's "visible" half took three passes, all failing the
same way, so the rule is promoted from an implementation detail to an invariant:

> **A message renders at the point it occupies on the wire — not where the user
> typed it, and not where the UI first learned about it.**

Queued input is the case that makes this non-obvious. A prompt typed during tool
round 1 may not be drained until after round 2; those are different positions in
the conversation, and only the second is where the model saw it. Echoing at
queue time renders the question *before* the assistant turn that preceded its
delivery, so the reply appears to arrive a turn late — which is precisely how
the bug was spotted, by reasoning about the ordering rather than reading code
("if it had been inserted there, the next message would have answered it").

Two consequences worth stating, because both were violated in turn:

- **Queued and delivered are different states and must not share a rendering.**
  Queued belongs in the pending list; delivered belongs in the transcript.
  Collapsing them — pushing to the transcript on queue — is what produced the
  early-by-one-round ordering.
- **Every delivery path needs its own echo, at its own position.** There are
  three: taken by the engine mid-run, never taken and submitted by the fallback,
  and sent while idle. An echo attached to one path silently disappears when
  another path is what runs.

**Derive position from the carrier message, never from polling.** The engine
appends the text to the tool-result message, so that message *is* the ordering
information — a frontend should render the echo while handling it, after the
batch's tool cells. Polling a queue for "has it been taken yet" races the very
message that answers the question, and renders the echo before the tool cells it
should follow. Correct by construction beats correct by timing.

This binds the P0.5 task notifications that reuse the queue: a notification
renders where it was drained, not where it arrived.
