# ADR-0014: Streaming event protocol (`stream-json`) — a self-sufficient trace source

## Status
Accepted

## Date
2026-07-17

## Context
For headless runs the caller often cares about the **full conversation trace**, not just
the final answer + metadata. The maintainer's `swe-lab` already reconstructs Claude Code
history by running `claude -p … --output-format stream-json --verbose` and concatenating
every event whose `type ∈ {user, assistant}` (pulling out the wrapped `message`). That works,
but the notes flag its weakness: **Claude's stream omits `system` and `tools`** ("Claude
Code's message view, without the raw system prompt"), which forced a *second* capture path (a
reverse proxy) to recover them.

locode should do better: make the stream a **self-sufficient, replayable source** — emit the
base prompt and tool specs up front so the entire run reconstructs with nothing else. This
ADR also reprioritizes: `stream-json` is a **first-class v0 output mode**, not a deferred
seam (it supersedes the "single-JSON first, events later" lean in
[docs/design/report-envelope.md](../design/report-envelope.md)).

## Decision
Define an `Event` enum in `locode-protocol`, serialized as **JSONL** (one object per line,
`#[serde(tag = "type")]`), `#[non_exhaustive]`:

- **`init`** (once, first): `session_id`, `harness`, `provider`, `model`, `cwd`, `max_turns`,
  `preamble: Vec<Message>` (the `System` + `Developer` messages), `tools: Vec<Value>` (the
  tool specs). *This is the fix for Claude's gap* — the stream carries its own context.
- **`message`**: one full [`Message`] per turn appended to history (role + content blocks;
  `tool_use`/`tool_result` live inside the blocks). This is the trace.
- **`result`** (terminal): the full `Report` — identical to `--output-format json`.
- **`error`**: a non-terminal note (e.g. a retry); terminal errors ride in `result`.

Plus `reconstruct_conversation(&[Event]) -> Conversation` = `init.preamble` ++ every `message`
event. A round-trip test proves the stream reconstructs the whole history, `System`/`Developer`
included.

**One summary, two modes:** the terminal `result` event carries the *same* `Report` that
`--output-format json` emits alone — so `json` and `stream-json` share one summary type.

### Related decisions settled here
- **Error taxonomy:** Claude uses a *flat* model — `is_error: bool` + a `subtype` string
  (`success` or one of `error_during_execution`/`error_max_turns`/`error_max_budget_usd`/
  `error_max_structured_output_retries`). We keep a **single flat `Status` enum** (clearest)
  and grow its values as the loop introduces terminal states — no two-level nesting.
- **Cost:** stay **tokens-only** for now (like Codex); `total_cost_usd` is a **TODO** (needs a
  pricing table), not blocking.
- **Transcript-in-`json`-mode:** **deferred** — the single `json` envelope stays a *summary*
  (final_message + tool_calls + usage); the full trace is `stream-json`'s job. Revisit later.

## Alternatives Considered
- **Single-JSON envelope only:** rejected — loses the per-turn trace the caller wants.
- **Mirror Claude's stream (no `init`):** rejected — not self-sufficient; reconstruction would
  need a side proxy for `system`/`tools`, exactly the pain `swe-lab` documents.
- **Per-token delta events:** deferred — the loop is non-streaming (ADR-0005), so whole-message
  events suffice; deltas are a `#[non_exhaustive]` addition if needed (cf. Claude's
  `--include-partial-messages`).

## Consequences
- `stream-json` is a v0 output mode: the loop (Task 6) emits `Event`s; `locode-exec` (Task 14)
  offers `--output-format {json, text, stream-json}` (`json` = the `result` alone; `stream-json`
  = the full event stream; `text` = final message).
- The event types + reconstruction land now in `locode-protocol` (ahead of the loop), with the
  JSONL round-trip + reconstruction test as the contract.
- Reserve turn markers (`turn.started`/`turn.completed{usage}`, cf. Codex) and message deltas as
  future `Event` variants.
