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

## Streaming granularity in the reference harnesses

Grounding the delta shape below (§Decision.1) in what the four studied harnesses
actually stream. **All four stream assistant text and reasoning as incremental
chunk deltas on *separate* channels; all four dispatch a tool call only from an
assembled *whole* with parsed arguments — none ever runs a tool on partial
JSON.** The only divergence is whether the partial-argument stream is *also*
surfaced (for a live "typing" UI) or kept internal.

| Content | claude-code (Anthropic Messages) | codex (Responses SSE) | grok-build (unified event) | opencode (AI-SDK-shaped) |
|---|---|---|---|---|
| Assistant text | chunk `text_delta` | chunk `OutputTextDelta` | chunk `ChannelToken{text}` | chunk `text-delta` |
| Reasoning | chunk `thinking_delta` (+`signature_delta`), own block | chunk `ReasoningSummaryDelta` / `ReasoningContentDelta` | chunk `ChannelToken{reasoning}` | chunk `reasoning-delta` |
| Tool **name** | early — `content_block_start` | early — `output_item.added` | early — first arg delta | early — `tool-input-start` |
| Tool **args** | accumulate raw string, **not parsed mid-stream** | **not surfaced** (custom tools excepted) | chunk `ToolCallDelta` — **UI only** | chunk `tool-input-delta` — **UI only** |
| Tool **finalized** | whole, `content_block_stop` | whole, `output_item.done` | assembled at stream end | whole, `content_block_stop` |
| Usage / stop | `message_delta` (stop_reason + usage) | `response.completed` | `Completed{metrics}` | `finish{reason,usage}` |

Granularities are **why**-driven, not incidental:

- **Text & reasoning are always chunked, on distinct channels.** Thinking is
  never interleaved into the assistant-text stream — every harness carries a
  separate reasoning delta type (and a terminal signature/summary sub-channel,
  e.g. Anthropic `signature_delta`, codex `reasoning_summary_text.done`) because
  the reasoning trace is needed for **replay**, not just display, so it must stay
  a distinct, reconstructable stream.
- **Tool calls are always dispatched from a finalized whole.** The *name* is
  available early (at the block/item "start"/"added" event) so the UI can render
  “Running `bash`…” and pre-resolve the tool, but arguments only become
  dispatchable at the **finalize boundary** — Anthropic `content_block_stop`,
  Responses `output_item.done`, Chat-Completions `finish_reason`. A tool cannot
  run on incomplete JSON, so:
  - **claude-code / codex** accumulate-then-parse and keep the partial args
    internal — claude-code deliberately bypasses the SDK's `BetaMessageStream`
    to avoid `partialParse` on every delta ("which we don't need since we handle
    tool input accumulation ourselves"); codex doesn't even surface
    `function_call_arguments.delta` (only *custom* tools stream input, purely to
    render a diff).
  - **grok-build / opencode** *also* emit per-chunk arg deltas (`ToolCallDelta` /
    `tool-input-delta`) — but strictly for the "command being typed" UI; the
    delta type itself is documented as "NOT necessarily valid JSON in isolation",
    and dispatch still gates on the assembled call.
- **Assemble under a stable per-call key** (Anthropic block index, OpenAI
  `tool_calls[].index`, Responses `item_id`) because providers attach `id`/`name`
  inconsistently across deltas; parse **once** at the finalize boundary.

This validates §Decision.1's `CompletionDelta`: `Text` / `Thinking` as chunk
deltas (reasoning on its own variant), `ToolUseStart { id, name }` emitted early,
`ToolArgs(String)` a display-only channel, and — critically — **tool dispatch
gated on the finalized whole `Completion`** (§Decision.3), which is exactly what
all four harnesses do. (Citations: claude-code `vendor/…/claude.ts:2087-2211`;
codex `sse/responses.rs:331-380`, `session/turn.rs:2062,2159-2176`; grok-build
`chat_completions.rs:212-255`, `events.ts:46-64`; opencode `tool-stream.ts:52-78`,
`protocol/anthropic-messages.ts:661-768`.)

## Streaming pacing / coalescing in the reference harnesses

Granularity (above) is *what* streams; pacing is *how fast it reaches the
screen*. A second source pass (2026-07-22) on the delta→render cadence:

| Harness | Display / flush boundary | Redraw throttle | Channel |
|---|---|---|---|
| codex | newline-gate → completed lines to a FIFO, partial line kept as a mutable "tail" cell; queued lines revealed by a **120 Hz commit-tick** (1 line/tick smooth, adaptive **batch-drain** on backlog ≥8 lines / 120 ms) | 8.33 ms tick + 120 fps redraw coalescing | **unbounded, no backpressure** |
| grok-build | append chunk; **batch-drain ≤32** queued ACP msgs, abort on a pending keystroke | **16 ms** min-draw + deferred coalesced repaint | **unbounded** mpsc; smoothing at the drain/throttle layer |
| claude-code | display = substring up to last `\n` (line-by-line) | Ink's **~16 ms** render batch | no queue; React state batching |
| opencode | per-delta, no gating | none (Solid fine-grained reactivity) | unbounded; only a >100-message eviction cap |

Two patterns are near-universal and drive §Decision.3–4:

1. **No channel is bounded and none applies backpressure toward the model.**
   Bounding risks stalling the SSE read; coalescing lives entirely on the
   *consumer* side.
2. **Coalescing = drain all pending deltas, then one repaint per ~16 ms frame**
   (grok: batch-drain ≤32 + 16 ms min-draw + deferred draw; claude via Ink's
   ~16 ms; codex via frame coalescing). Newline-gating the *display* (codex +
   claude) is a render-layer choice, not a channel concern.

**Key consequence for us: `locode-tui`'s event loop already implements grok's
pattern** — `ENGINE_DRAIN_MAX = 32` batch-drain, `MIN_DRAW_INTERVAL = 16 ms`, and
`deferred_draw` coalescing (`crates/locode-tui/src/event_loop.rs`). A token flood
already collapses to ≤1 repaint per 16 ms frame *for free*, so the first slice
needs **no channel bound and no engine-side coalescing** — deltas flow through the
loop we already built, and the only new pacing work is **newline-gating in the
render layer**. Codex's 120 Hz line-reveal "typing animation" is optional polish,
deferred.

(Citations: codex `streaming/mod.rs`, `streaming/controller.rs`,
`markdown_stream.rs:87` (newline gate), `streaming/chunking.rs:85-116` (adaptive
policy), `app.rs:392-396` (commit tick); grok-build `app/event_loop.rs:1704-1737`
(batch-drain + deferred draw), `display_refresh.rs:12` (16 ms default); claude-code
`src/utils/messages.ts:3048-3054`, `src/screens/REPL.tsx:1458-1473` (Ink throttle +
newline gate); opencode `session/processor.ts:499-509`, `tui/src/context/sync.tsx:392-408`.)

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
Consume `Event::MessageDelta` into a live "streaming cell" in the live region;
finalize to scrollback (ADR-0022) on turn end. Adopt **codex's markdown streaming
model** (from the study): buffer deltas, re-parse the whole buffer gated at
newline boundaries, stable-prefix / mutable-tail. Simplest design with a provable
"streamed frame == final frame" guarantee. **Pacing rides the loop we already
have** (see the pacing section): deltas coalesce through the existing
`ENGINE_DRAIN_MAX = 32` batch-drain + `MIN_DRAW_INTERVAL = 16 ms` + `deferred_draw`
paint (grok's pattern) — no channel bound, no engine-side coalescing. The only new
pacing work is newline-gating the display; codex's 120 Hz typing animation is
deferred polish.

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

*Review 2026-07-22 (user): Q1–Q3 resolved below; Q4–Q6 still open.*

1. **`stream-json` output**: keep it whole-message (trace stability) and treat
   deltas as a TUI-only concern, or add an opt-in `--include-deltas`?
   **→ RESOLVED: keep whole-message; deltas are live-only.** This is exactly
   Claude Code's behavior — `--output-format stream-json` emits whole messages by
   default, and token-level chunks are opt-in behind `--include-partial-messages`
   (claude-code `src/main.tsx:976`, env `CLAUDE_CODE_INCLUDE_PARTIAL_MESSAGES`).
   A future `--include-deltas` mirroring that flag is the additive extension
   (behind the `#[non_exhaustive]` `Event` enum); not built in the first slice.
2. **Cancellation mid-stream**: the cancel token must abort the in-flight SSE
   read, not just between turns.
   **→ RESOLVED: abort immediately, discard the partial.** A mid-stream cancel
   drops the HTTP response stream the moment the ADR-0018 token trips (race the
   SSE read against the token in the wire; dropping the `reqwest` response future
   closes the connection — no new cancel surface). The partial assistant text is
   **discarded** (not appended to history); the final `Report` still reads
   `Status::Cancelled`. "Cancel = this turn didn't happen."
3. **Event volume / backpressure**: deltas are many small events on the
   currently-unbounded engine→UI channel. Bound it, or coalesce at the engine?
   **→ RESOLVED: neither — lean on the existing paint loop.** Per the pacing
   section, no reference harness bounds its channel or backpressures the model;
   coalescing is consumer-side, and `locode-tui`'s loop already does grok's
   version of it (`ENGINE_DRAIN_MAX = 32` + `MIN_DRAW_INTERVAL = 16 ms` +
   `deferred_draw`). So: keep the channel unbounded, add **no** engine-side
   coalescing, and newline-gate at the render layer. Codex's 120 Hz typing
   animation is deferred polish.
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
