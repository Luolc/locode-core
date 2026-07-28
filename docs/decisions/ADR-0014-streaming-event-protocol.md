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
  future `Event` variants. *(Realized 2026-07-22 by [ADR-0021](ADR-0021-live-token-streaming.md):
  `Event::MessageDelta` added for live streaming. This does not supersede ADR-0014 — the
  whole-message `stream-json` trace still holds, and `MessageDelta` is deliberately excluded from
  it so the trace stays self-sufficient.)*

## Amendment (2026-07-21): one stream, multiple runs (session continuity)

With ADR-0016, a `Session` outlives a single run, and so does its event stream:
`Init` is emitted **once per session** (on the first run), then `Message` events
continue across runs with **one `Result` per run** — the stream shape is
`Init M+ Result (M+ Result)*`. `Result` is a *run* terminator, not a stream
terminator. `reconstruct_conversation` is unaffected (it folds `Message` events
in order and ignores `Result`/`Error`); the contract is pinned by a two-run
golden test in `locode-engine`
(`two_run_stream_reconstructs_the_full_conversation`).


## Amendment (2026-07-27): `MessageDeltaReset` — deltas can be annulled

The stream is display-only, but it was **append-only**, and that made one engine
behavior unrepresentable: `sample_with_retry` resamples the *same* request after a
retryable provider error, so a stream that failed part-way is followed by a second
stream of the same reply from the start. A consumer buffering deltas had no way to
know the first run was void — it rendered the reply twice. Making lossy streams
retryable (ADR-0007 amendment, same day) turned that from rare into routine.

`Event::MessageDeltaReset { reason }` is the missing signal: **every
`MessageDelta` since the last `Message` is void; drop the buffer.** The enum is
`#[non_exhaustive]` and this is additive, so no consumer breaks; the report
envelope's `schema_version` is untouched (events are not the envelope), and the
rollout is message-based, so traces on disk keep their shape.

Three properties worth stating, because each was a way to get it wrong:

- **It is emitted only when the failed attempt actually streamed.** A failure
  before the first delta has nothing to annul, and a spurious reset would clear a
  buffer the consumer had legitimately filled.
- **It precedes the retry's deltas**, or a consumer would clear the buffer after
  refilling it (pinned by an ordering assertion, not just a count).
- **It is display-only, like the deltas it annuls.** `reconstruct_conversation`
  and the whole-message `stream-json` trace both skip it: a consumer that ignores
  deltas has nothing to undo, and the resampled turn still arrives as one
  `Message`. The retry itself stays visible in the trace as the `Error` note.

**What it cannot fix:** text a UI already committed to the terminal's scrollback
cannot be withdrawn, so a long partial reply may still sit above the re-streamed
one. The TUI drops the *uncommitted* buffer and resets its committed-any flag so
the re-stream renders as a fresh block; the notice the engine emits alongside is
what explains the duplicate to the user. Fully solving it needs the transcript to
be rewritable (rejected in ADR-0019: printed history is not rewritten).
