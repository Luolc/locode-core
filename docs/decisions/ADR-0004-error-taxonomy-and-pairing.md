# ADR-0004: Soft/fatal error taxonomy and strict tool-call/result pairing

## Status
Accepted

## Date
2026-07-17

## Context
Two invariants recur across all four studied systems. (1) Almost every tool failure should be **recoverable**: the model sees the error and tries again. Only unrecoverable transcript state should abort. Codex encodes this as `FunctionCallError::{RespondToModel, Fatal}`. (2) Providers **reject the entire request** if a `tool_use` has no `tool_result`, or a `tool_result` is duplicated. All four spend real code guarding this; Grok exposes reusable `repair_dangling_tool_calls` + `dedup_duplicate_tool_results`.

## Decision
Model tool errors as `enum ToolError { Respond(String), Fatal(String) }`. **Default everything to `Respond`** (soft) — bad args, unknown tool, not-found, command failure, timeout, permission-declined all become a `tool_result{is_error: true}` the model can recover from. Reserve `Fatal` for "the transcript is unrecoverable," which aborts the turn with a non-zero exit. Enforce, as a single pre-send pass on the transcript: **every `tool_use` id has exactly one `tool_result`.** On an interrupted/aborted turn, synthesize `is_error` results for calls that didn't run.

## Alternatives Considered
### One error type; any error aborts
- Rejected: throws away the model's ability to self-correct; makes the agent brittle.

### Guard pairing only inside the loop
- Rejected: the invariant is a **wire-format** requirement. Making it a single function the provider layer calls unconditionally (before every send) is more robust than scattering checks.

## Amendment (2026-07-25): a soft error must name the cause it actually knows

`Respond` keeps the loop productive only when the prose is *true*. A `tool_use`
truncated by the output-token limit arrives with an empty `input` (the
Anthropic wire returns `{}`, not partial JSON), so the typed decode reported
`missing field \`file_path\`` — an accusation that the model malformed its
call. The model, having no way to know it was cut off, re-sent the same
oversized call and was cut off identically; the "recoverable" error looped
forever.

The loop therefore recognizes this case ahead of dispatch and synthesizes the
result itself, naming the output-token limit and telling the model not to
repeat the call unchanged (ADR-0005 amendment, same date). The general rule
this sharpens: when the engine holds context the tool layer does not, it owns
the message — a soft error that misattributes blame is worse than a fatal one,
because it is retried.

## Consequences
- The loop stays productive: schema-decode failures and unknown tool names return prose telling the model to fix its call, rather than crashing.
- Transcript validity is centrally enforced, independent of how a turn ended (normal, max-turns, abort, mid-batch cancel).
- `Fatal` is rare by design; most "errors" are just data the model reads.

## Amendment (2026-08-01): pairing is **positional**, and the repair rebuilds it

A session died with `messages.434: 'tool_use' … found without 'tool_result' blocks
immediately after`, and every resample failed identically (0 turns, 0 tokens) — the
request never left the client. The pre-send repair this ADR mandates had run and
found nothing to do.

**The mismatch:** the repair asked *"does a result for this id exist anywhere in the
transcript?"* while the API asks *"is the result in the message immediately after
the call?"*. A result that exists but sits one message too late satisfies the first
and fails the second. Since the whole history replays every turn, that transcript
was permanently unsendable — the same permanence the ADR-0007 amendment describes
for blank text blocks, from a different direction.

**The rule, stated so it cannot be read existentially:** every `tool_use` in a
message must be answered by a `tool_result` in the **message immediately after**,
and results are written in **call order**. An id appearing somewhere else in the
conversation does not satisfy the invariant, and neither does a result whose call is
absent.

`repair_pairing` therefore **rebuilds** the pairing instead of patching it: remember
each result's content by id (last occurrence wins, keeping the old dedup rule),
strip every result block out (dropping messages left empty), then write exactly the
results each assistant turn's calls need, in order, at the front of the following
user turn. Three outcomes, counted separately in `RepairStats` because they mean
different things:

- **relocated** — the content existed but in the wrong place, and was moved. This is
  the case the old check could not see, and the one that poisoned the session.
- **synthesized** — no result existed anywhere, so an `is_error` block says the tool
  did not report (unchanged behavior).
- **deduped** — duplicates beyond the last, and **orphans** whose `tool_use` is
  nowhere. Orphans are now dropped; the API rejects them as loudly as a dangling
  call, and this pass previously left them alone.

**Not answered here:** what produced the misplaced result. The transcript came from
a session running through a proxy already known to drop and retry frames, so the
cause may be upstream, a crash between appending the call and its results, or
something in our own drain — the rollout is on the reporter's machine and has not
been read. This amendment makes the *consequence* impossible either way: a
transcript that can be repaired is repaired before it is ever sent, and one bad turn
costs a turn instead of the session.
