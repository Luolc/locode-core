# ADR-0021: Live token streaming (provider SSE → engine deltas → TUI incremental render)

## Status
**Proposed** — draft for review. This ADR defines the seam and weighs the
options; it does not lock the implementation. It touches the **core public
surface** (`Provider` trait, `Event` enum, the engine loop), which is an
AGENTS.md "ask first" boundary — so this document *is* the ask.

## Date
2026-07-22

## Context

The engine is **non-streaming by design** (ADR-0005): `Session::run` awaits a
whole `Completion` from `Provider::complete`, then dispatches tools, then
appends — one buffered turn at a time. That is correct and simple, but in an
interactive TUI it means the screen sits on a spinner for the entire model turn
and the reply appears all at once. Every reference harness streams; a live
session without it feels dead. It is also the **one blocker** for Phase 4 of the
markdown study (streaming render) — Phases 1–3 (highlighting, tables, links) are
all unblocked and proceeding.

**This is distinct from ADR-0014.** ADR-0014's `stream-json` is an *output trace*
format: one `Event::Message` per completed turn, for replayable headless traces.
It is whole-message, not token-level. ADR-0021 is about **live token deltas**
during a single turn. The two compose: streaming deltas feed the live TUI;
`stream-json` keeps emitting whole messages for trace stability (see Open
Questions).

Relevant seams as they stand:
- `Provider::complete(&self, &ConversationRequest) -> Result<Completion, ProviderError>`
  (`crates/locode-provider/src/provider.rs:34`) — one buffered call. The Anthropic
  and OpenAI-Responses wires each reserved an SSE seam in their plans but send
  non-streaming today.
- `Event` (`locode-protocol`, ADR-0014) — `#[non_exhaustive]`, JSONL, currently
  `init` / `message` / `result` / `error`. Room to add a delta variant.
- The TUI already renders finalized `Block`s via `insert_before` and keeps a
  bounded live region; a "streaming cell" would live in that region until the
  turn finalizes (ADR-0019 named this an extension point).

## Decision (proposed)

Add token streaming as an **opt-in capability layered over the existing loop**,
not a second loop (respecting the ADR-0005 "no second loop" boundary). Four
layers:

### 1. Provider layer — a `stream` method with a default fallback
Add to the `Provider` trait:

```rust
async fn stream(
    &self,
    request: &ConversationRequest,
    on_delta: &mut dyn FnMut(CompletionDelta),
) -> Result<Completion, ProviderError> {
    // Default: non-streaming wires emit one delta = the whole completion.
    let completion = self.complete(request).await?;
    on_delta(CompletionDelta::from_completion(&completion));
    Ok(completion)
}
```

- Returns the **same final `Completion`** it does today (so tool assembly,
  pairing, and history are unchanged); deltas are an *additional* side channel.
- **Default-implemented** in terms of `complete`, so `mock` and any wire that has
  not implemented SSE keep working with a single synthetic delta. No breaking
  change to existing providers at the source level (though adding a trait method
  is still a public-surface change — hence "ask first").
- `CompletionDelta` (new, in `locode-provider`): normalized, mirroring the parts
  the wires actually stream — `Text(String)`, `Thinking(String)`, and
  `ToolUseStart { id, name }` / `ToolArgs(String)` for tool-call assembly.

### 2. Wire layer — SSE parsing (the bulk of the work)
Anthropic (`stream: true`, `content_block_delta` → `text_delta` /
`thinking_delta` / `input_json_delta`) and OpenAI-Responses (`response.*.delta`
events). Reuse the existing `ToolCallAssembler` to fold `ToolArgs` deltas into
the final `Completion`, so the assembled result is byte-identical to the
non-streaming path. Mid-stream transport errors reuse the `ProviderError`
taxonomy + the wire's bounded retry.

### 3. Engine layer — forward deltas as events, loop unchanged
`Session::run`'s **sample** step calls `stream` instead of `complete`, with a
sink that emits a new `Event::MessageDelta { text | thinking | … }` to the
existing `EventSink`. **Structure is unchanged**: the engine still appends the
whole `Message` at turn end (so `stream-json` and history are identical), still
dispatches tools only after the full completion (tools need complete args).
Streaming is **display-only** for assistant text/thinking; tool execution is
unaffected.

### 4. TUI layer — a streaming cell + incremental markdown (study Phase 4)
Consume `Event::MessageDelta` into a live "streaming cell" in the bounded live
region; finalize to `insert_before` on turn end. Adopt **codex's markdown
streaming model** (from the study): buffer deltas, re-parse the whole buffer
gated at newline boundaries, stable-prefix / mutable-tail. Simplest design with
a provable "streamed frame == final frame" guarantee.

## Alternatives Considered

- **Return a `Stream<Item = Delta>`** (futures-style) instead of a delta
  callback — more idiomatic, but adds a `futures` dependency, complicates
  object-safety with `async_trait`, and forces every caller into a stream loop.
  The callback keeps the trait shape close to today's and the default fallback
  trivial. *Leaning against.*
- **A separate streaming loop / a JSON-RPC seam to the engine** (codex's
  in-process protocol) — rejected: violates the ADR-0005 "no second loop"
  boundary and ADR-0019 chose typed channel messages over a wire protocol at v1
  scale.
- **Stream tool-call arguments to the UI too** — deferred: assembling tool args
  silently and only streaming assistant text/thinking is simpler and matches what
  users actually watch. Revisit if "typing" tool args proves valuable.
- **Do nothing / keep buffered** — rejected: it is the main "feels dead" gap and
  blocks markdown Phase 4.

## Consequences
- **Public-surface change** to `Provider` (new method, defaulted), `Event` (new
  `#[non_exhaustive]` variant — additive), and the engine's sample step. The
  facade re-exports may need touching. This is the reason for the ask.
- Per-wire SSE parsing is real, testable work with new failure modes
  (mid-stream error, cancellation mid-stream, partial JSON). Each wire gets its
  own plan + golden tests, as the non-streaming wires did.
- Non-streaming consumers (headless `-p json`) are unaffected — they ignore
  deltas and read the final `Report`.

## Open Questions (for review)
1. **`stream-json` output**: keep it whole-message (trace stability) and treat
   deltas as a TUI-only concern, or add an opt-in `--include-deltas`? *Lean:
   keep whole-message; deltas are live-only.*
2. **Cancellation mid-stream**: the cancel token must abort the in-flight SSE
   read, not just between turns. Confirm the wire's HTTP client drops the
   response stream on cancel (ADR-0018 interaction).
3. **Event volume / backpressure**: deltas are many small events on the
   currently-unbounded engine→UI channel. Bound it, or coalesce deltas
   (e.g. flush per newline / per ~16 ms) at the engine boundary?
4. **Thinking deltas**: stream reasoning text live (like the reply) or keep
   thinking collapsed until done? *Lean: stream, dimmed, collapsible later.*
5. **Ordering vs. the delta callback's `&mut dyn FnMut`**: is a callback the
   right shape, or should the engine pass an `mpsc::Sender<CompletionDelta>` so
   the wire is decoupled from engine internals? *Lean: sender, symmetrical with
   the existing `EventSink`.*
6. **Scope of the first slice**: land the seam + `mock`/Anthropic streaming +
   the TUI streaming cell first; OpenAI-Responses and the incremental-markdown
   polish as follow-ons?

## SPEC reconciliation
`SPEC.md` currently lists streaming under deferred/reserved seams. On acceptance,
move it to a scheduled task and point the tool contract's streaming note at this
ADR. Not changed yet (this is a Proposed draft).
