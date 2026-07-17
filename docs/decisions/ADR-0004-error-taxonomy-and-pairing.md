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

## Consequences
- The loop stays productive: schema-decode failures and unknown tool names return prose telling the model to fix its call, rather than crashing.
- Transcript validity is centrally enforced, independent of how a turn ended (normal, max-turns, abort, mid-batch cancel).
- `Fatal` is rare by design; most "errors" are just data the model reads.
