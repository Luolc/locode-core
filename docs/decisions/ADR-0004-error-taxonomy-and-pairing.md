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

`repair_pairing` therefore **rebuilds** the pairing instead of patching it: for each
assistant turn that called tools, take the results out of the following user turn,
keep the ones its calls asked for (in **call order**), synthesize an `is_error` block
for the rest, and drop every other result block in the transcript. Two outcomes are
counted: **synthesized** (no result existed, so the model is told the tool did not
report) and **deduped** (duplicates beyond the last, results that were not where the
API requires them, and **orphans** whose `tool_use` is nowhere — the API rejects those
as loudly as a dangling call, and this pass previously left them alone).

What it deliberately does **not** do is hunt down a misplaced result and move it back.
A first version of this amendment did, and that was wrong twice over: it makes a
scrambled conversation *sendable* rather than *right*, and it guards a cause that has
since been removed at the source — the only thing that ever reordered a transcript was
two processes appending to one rollout, which the trace format now survives by
recording lineage (ADR-0024 amendment 2026-08-01). Missing results are repaired
because they have a mundane cause (a process killed between a call and its results);
misplaced ones are not, because they no longer have one, and a repair that hides the
next cause of scrambling is worse than the 400 it prevents.

**What produced it** *(answered 2026-08-01, after this amendment was first written)*:
two locode processes appending to one rollout — not the proxy, which was the first
guess. The transcript was the interleave of two conversations, and the unpaired call
was one process's, answered two messages later after the other process had spoken. The
fix for *that* is ADR-0024's lineage amendment; the positional rule here stands on its
own, because a transcript that violates the API's rule must not be sent whatever wrote
it.
