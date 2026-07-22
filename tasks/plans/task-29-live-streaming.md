# Task 29 — Live token streaming (ADR-0021): implementation plan

**Status:** planning (2026-07-22). Grounded in [ADR-0021](../../docs/decisions/ADR-0021-live-token-streaming.md)
(Accepted) and a fresh source pass over our own crates + the four studied
harnesses. This plan covers **all three slices**; each slice's wire SSE work
still gets the process's Phase-1 harness revisit + a per-wire test plan before it
lands (this doc is the map, not a substitute).

## The one-paragraph shape

Add an **opt-in `Provider::stream`** (default-implemented in terms of `complete`)
that returns the **same final `Completion`** while emitting `CompletionDelta`
side-channel parts. The engine's sample step forwards each delta **inline through
a callback** into the existing `EventSink` as a new `Event::MessageDelta`; it
still appends the whole `Message` at turn end and still dispatches tools from the
finalized completion. The TUI accumulates deltas into a **live cell** rendered in
the pinned region, finalized to a normal `AssistantText` block on turn end.
Pacing rides the loop we already have (16 ms paint + 32-msg drain). Three slices:
**(1)** the seam end-to-end on Anthropic + mock + a minimal plain-text cell;
**(2)** OpenAI-Responses SSE; **(3)** incremental markdown in the cell (study
Phase 4).

## What already exists (this is mostly wiring, not greenfield)

The v0 wires were built "streaming-ready"; the seam is half-built already:

| Asset | Location | Use |
|---|---|---|
| `ToolCallAssembler` (`begin`/`push_json`/`finish`, keyed by block index; parses **once** at finish) | `locode-provider/src/assemble.rs` | fold `input_json_delta` → final `ToolUse` blocks, byte-identical to non-streaming |
| Anthropic SSE event types (`MessageStreamEvent`, `StreamDelta` = `TextDelta`/`InputJsonDelta`/`ThinkingDelta`/`SignatureDelta`) — **present but unused** | `locode-provider/src/anthropic/wire.rs:416-541` | deserialize the SSE frames directly; no new types needed for Slice 1 |
| Shared HTTP + retry (`build_http_client` 10-min budget, `run_with_retry` generic over `T`, `RetryPolicy`, `backoff`) | `locode-provider/src/http.rs` | reuse transport + `ProviderError` taxonomy + bounded retry |
| `Event` is `#[non_exhaustive]`, serde `tag="type"` snake_case | `locode-protocol/src/lib.rs:371-425` | add `MessageDelta` additively |
| Engine sample step already a biased `select!` that **drops the future on cancel** | `locode-engine/src/run.rs:301-308` | mid-stream abort = future-drop, no new mechanism (Q2) |
| TUI drain/paint already coalesces bursts (16 ms `MIN_DRAW_INTERVAL` + 32 `ENGINE_DRAIN_MAX` + `deferred_draw`) | `locode-tui/src/event_loop.rs:17-24,85-137` | no channel bound, no engine coalescing (Q3) |
| Spinner already shows `"thinking"` when no tool is pending | `locode-tui/src/ui.rs:155-176` | Q4: thinking stays spinner-only |

The OpenAI-Responses SSE types do **not** exist yet (seam is prose only in
`tasks/plans/task-18-openai-responses-wire.md:645-656`) — that's Slice 2's build.

## Resolved design (from the ADR-0021 review; recap)

- **Q1** trace stays whole-message; deltas are TUI-only.
- **Q2** mid-stream cancel aborts immediately, partial discarded, `Report=Cancelled`.
- **Q3** unbounded channel, no engine coalescing, newline-gate the display.
- **Q4** reasoning on its own delta channel; UI shows no inline thinking (spinner
  is the indicator); **preserve the thinking block + signature in the finalized
  `Message`** (Anthropic multi-turn correctness).
- **Q5** a callback `&mut (dyn FnMut(CompletionDelta) + Send)`, not a channel.
- **Q6** three slices (this doc).

---

# Slice 1 — the seam end-to-end (Anthropic + mock + minimal cell)

**Goal:** one real streaming wire (Anthropic) drives a live plain-text cell in the
TUI, cancel-mid-stream aborts + discards, and the assembled `Completion` /
history / headless trace are **byte-identical** to today. Proves the whole
`wire → engine → protocol → TUI` vertical before breadth.

### 1a. `locode-provider` — the `stream` seam + `CompletionDelta`

- **New type** `CompletionDelta` (new `completion_delta.rs`, re-export from
  `lib.rs`): a display-oriented part enum mirroring what the wires stream —
  ```rust
  pub enum CompletionDelta {
      Text(String),
      Thinking(String),
      ToolUseStart { id: String, name: String },
      ToolArgs(String),        // raw partial JSON; display-only, never parsed here
  }
  ```
- **Trait method** on `Provider` (`provider.rs:20-35`), **default-implemented** so
  `mock` and any un-migrated wire keep working:
  ```rust
  async fn stream(
      &self,
      request: &ConversationRequest,
      on_delta: &mut (dyn FnMut(CompletionDelta) + Send),   // Q5: callback, +Send
  ) -> Result<Completion, ProviderError> {
      let completion = self.complete(request).await?;
      let text = completion.text();
      if !text.is_empty() { on_delta(CompletionDelta::Text(text)); }  // one synthetic delta
      Ok(completion)
  }
  ```
  *Decision:* the default fallback emits a **single `Text` delta** of the joined
  text (deltas are display-only; tool blocks are assembled from the returned
  `Completion`, so no synthetic tool deltas are needed). Returns the same
  `Completion` unchanged.
- **`+ Send` on the callback** is required because it's held across the SSE
  `.await` inside the wire, and the engine's future must stay `Send` (tokio
  multi-thread).

### 1b. `locode-provider/anthropic` — real SSE `stream`

- Flip `stream: Some(true)` when streaming (currently hard-`false` at
  `anthropic/build.rs:93`); keep a non-streaming build path for `complete`.
- Add a streaming send alongside `client::send_once` (`anthropic/client.rs:47-105`):
  branch to `response.bytes_stream()` instead of `response.json()`, wrapped in the
  same `run_with_retry` + header/auth path so transport errors and the
  `ProviderError` taxonomy are unchanged.
- **Parse loop** over SSE frames, deserializing the already-present
  `wire::MessageStreamEvent` (`anthropic/wire.rs:420-464`). Drive both the
  `on_delta` callback and a `ToolCallAssembler`, exactly mirroring the harnesses
  (Agent D: opencode `anthropic-messages.ts` step 814; claude-code `claude.ts`
  1979) — the assembly maps 1:1 onto our types:

  | SSE event | `on_delta` | assembly / final `Completion` |
  |---|---|---|
  | `content_block_start` (text) | — | start a text accumulator at `index` |
  | `content_block_start` (tool_use) | `ToolUseStart { id, name }` | `assembler.begin(index, id, name)` |
  | `content_block_start` (thinking) | — | start a thinking accumulator |
  | `content_block_delta` `TextDelta` | `Text(t)` | append `t` to the block's text |
  | `content_block_delta` `InputJsonDelta` | `ToolArgs(pj)` | `assembler.push_json(index, pj)` (raw, no parse) |
  | `content_block_delta` `ThinkingDelta` | `Thinking(t)` | append to thinking |
  | `content_block_delta` `SignatureDelta` | — | attach signature to the thinking block |
  | `content_block_stop` | — | close the block; tool block finalizes at `assembler.finish()` |
  | `message_delta` | — | capture `stop_reason` + usage |
  | `message_stop` | — | assemble the final `Completion` |

- **Byte-identical invariant:** the assembled `Completion` (content order, tool
  `id`/`name`/`input`, thinking + **signature**, usage, stop) must equal what the
  non-streaming `parse.rs:22-73` produces. This is the key golden test.
- **Q4 correctness:** the thinking block **and its `signature_delta`** land in the
  final `Completion.content` (as `ContentBlock::Reasoning { .. , signature }`) so
  the appended `Message` carries the signed block — required for Anthropic
  multi-turn-with-tools (Agent D; claude-code `query.ts:158,714-715`). Display
  still ignores thinking; this is history correctness only.

### 1c. `locode-provider` mock — real streaming for tests

- Implement `stream` on `MockProvider` (`mock.rs:20-62`) to **chunk** the scripted
  `Completion`'s text into several `Text` deltas (e.g. per word or per line), then
  return the same `Completion`. Gives deterministic multi-delta streams for
  engine/TUI tests without a network. (The default fallback's single delta is also
  fine, but a chunking mock exercises the coalescing/live-cell path realistically.)

### 1d. `locode-protocol` — `Event::MessageDelta`

- Add variant (`lib.rs`, ~after `Message` at :400), wire tag `message_delta`:
  ```rust
  MessageDelta { delta: MessageDeltaBody },   // { text?: String, thinking?: String }  (tool parts omitted from the trace)
  ```
  Keep the body minimal — **text** (and optionally **thinking**, though the UI
  ignores it) — since deltas are display-only and never reconstructed.
- Add the ignore arm to `reconstruct_conversation` (`lib.rs:436-440`):
  `Event::MessageDelta { .. } => {}` — deltas are **not** history (the whole
  `Message` is still appended), so reconstruction must skip them (no double-count).

### 1e. `locode-engine` — forward deltas, loop otherwise unchanged

- **Decision (flag, not blanket-swap):** add `EngineConfig.stream_deltas: bool`
  (default **false**). The sample step (`run.rs:301-308`) becomes:
  ```rust
  let cancel = self.cancel.clone();
  let provider = Arc::clone(&self.provider);   // clone Arc so on_delta can borrow &mut self.sink
  let result = tokio::select! {
      biased;
      () = cancel.cancelled() => return Err(SampleError::Cancelled),
      result = async {
          if self.stream_deltas {
              let mut on_delta = |d: CompletionDelta| { /* map → Event::MessageDelta */ self.sink.emit(..) };
              provider.stream(&request, &mut on_delta).await
          } else {
              provider.complete(&request).await
          }
      } => result,
  };
  ```
  *Why the flag:* it keeps the **headless `-p` path byte-for-byte unchanged**
  (it stays on `complete`, no SSE, satisfying Q1 trivially) and lets the TUI opt
  in. This is a small, ADR-consistent refinement of ADR-0021 §3's "call `stream`
  instead of `complete`" — worth a one-line ADR note when it lands. *(Alternative
  considered: always call `stream` + have the headless `stream-json` writer drop
  `MessageDelta`. Rejected: it puts SSE on the `-p` transport path and changes its
  failure modes for the A/B researchers, for no gain.)*
- **Cancellation (Q2):** unchanged — a cancel drops the `stream` future mid-SSE,
  which aborts the HTTP read (`run.rs:291-296` comment) and returns
  `SampleError::Cancelled` → `Status::Cancelled`. The whole `Message` is only
  appended **after** `stream` returns `Ok`, so a mid-stream cancel **discards** the
  partial automatically. The already-emitted deltas are display-only; the TUI
  clears its live cell on `RunFinished` (see 1f).
- **Whole-message append is untouched** (`run.rs:102-111`): history + `Event::Message`
  + tool dispatch from the finalized `completion.content` all stay exactly as today.
- **Borrow note:** `Arc::clone` the provider before the `select!` so the `on_delta`
  closure can mutably borrow `self.sink` without conflicting with `&self.provider`.

### 1f. `locode-tui` — the live cell

- **App state** (`app.rs:103-148`): add `pub streaming: Option<String>` (the live
  buffer for the in-progress assistant turn).
- **`on_event`** (`app.rs:300-342`): new arm
  `Event::MessageDelta { delta } => { self.streaming.get_or_insert_default().push_str(&delta.text) }`
  (ignore `thinking` per Q4). Set `dirty`.
- **Finalize:** the existing `Event::Message` / `Role::Assistant` / `Text` arm
  (`app.rs:307-309`) clears `self.streaming = None` (the full `Text` block arrives
  and becomes the `AssistantText` outbox block — visually seamless). Also clear
  **defensively in `on_run_finished`** (`app.rs:368-399`) so a cancelled/errored
  turn drops the partial (Q2).
- **Render** (`ui.rs` `draw` + `live_rows`): insert one
  `Constraint::Length(live_rows)` segment **between `tail_area` and `status_area`**
  (`ui.rs:59-76`), rendering the buffer via the **same `AssistantText` bullet +
  path** as `blocks.rs:97-119`, but **plain-text, newline-gated** for Slice 1: show
  `buffer[..=last '\n']` via `wrap_plain` (`blocks.rs:209-245`), keep the trailing
  partial line hidden (claude-code `REPL.tsx:1473`). Account for the height in
  `live_rows` `non_tail` (`ui.rs:108-112`) so `paint`'s overflow math reserves the
  rows (never committed to scrollback while live). Cap the cell's rendered height
  (scroll to the tail) so a long turn doesn't push transcript into scrollback.
- **No changes** to the engine channel/`FnSink`, `route_engine`, the batched drain,
  `flush_outbox`, or `paint`'s scrollback logic (Agent C).
- **Turn on streaming:** `locode-tui/src/engine.rs` sets `stream_deltas: true` in
  the `EngineConfig` it builds (`engine.rs:157-168`).

### Slice 1 test matrix

| Layer | Test | Asserts |
|---|---|---|
| provider | `stream` default fallback | one `Text` delta = `completion.text()`; returned `Completion` unchanged |
| provider | mock chunked `stream` | N `Text` deltas concatenate to the scripted text; same `Completion` |
| wire (golden) | Anthropic SSE fixture → `stream` | assembled `Completion` **byte-identical** to `complete` on the same logical response (content order, tool id/name/input, thinking+signature, usage, stop) |
| wire | SSE delta sequence | `on_delta` receives the expected `Text`/`ToolUseStart`/`ToolArgs`/`Thinking` sequence; tool args never parsed mid-stream |
| wire | mid-stream transport error | reuses `ProviderError` taxonomy + bounded retry; terminal vs retryable classified as non-streaming |
| protocol | `MessageDelta` round-trip | JSONL serialize/deserialize; `reconstruct_conversation` ignores it (history == non-streaming) |
| engine | `stream_deltas=true` run | deltas emitted as `Event::MessageDelta` **and** the whole `Event::Message` still appended; report identical |
| engine | `stream_deltas=false` (headless) | **no** `MessageDelta` emitted; path identical to today |
| engine | cancel mid-stream | future dropped → `Status::Cancelled`; no assistant `Message` appended (partial discarded) |
| TUI (reducer) | feed `MessageDelta`s then `Message` | live buffer accumulates, then clears; final `AssistantText` block present; matches existing `Event::Message` test template (`app.rs:905`) |
| TUI (reducer) | cancel/`RunFinished` mid-stream | `streaming` buffer cleared; no stray block |
| TUI (render) | live cell newline-gating | trailing partial line hidden; completed lines shown; height accounted in `non_tail` |

### Slice 1 risks / watch-items

- **Byte-identical assembly** is the crux — a golden fixture reused for both
  `complete` and `stream` is the guard.
- **Borrow/`Send` around the callback** (Arc-clone the provider; `+ Send` on the
  closure) — get it right or the engine future won't be `Send`.
- **Thinking signature preservation** — easy to drop `signature_delta`; the
  multi-turn-with-tools golden test catches it.
- **Live-cell height vs scrollback** — the pinned cell shrinks `max_tail`; cap its
  height so it doesn't prematurely commit transcript to scrollback.

---

# Slice 2 — OpenAI-Responses SSE

**Goal:** the second wire streams, reusing the Slice 1 seam unchanged. No SSE
types exist yet — this slice **builds** them (Agent A; seam is prose-only in
`task-18-openai-responses-wire.md:645-656`).

### Scope

- **New wire SSE types** in `locode-provider/src/openai/responses/` mirroring
  codex's `ResponsesStreamEvent` (Agent D; codex `sse/responses.rs:160-175,327-473`).
- Flip `stream: false` → `true` when streaming (`responses/build.rs:191`); add a
  `bytes_stream()` send alongside `send_once` (`responses/mod.rs:77-129`), same
  `run_with_retry` path.
- **Event mapping** (`match event.kind`):

  | Responses SSE event | `on_delta` | assembly |
  |---|---|---|
  | `response.output_item.added` (function_call) | `ToolUseStart { id, name }` | record the call's `id`/`name` (present in the item) |
  | `response.output_text.delta` | `Text(delta)` | append |
  | `response.reasoning_summary_text.delta` / `reasoning_text.delta` | `Thinking(delta)` | append (encrypted-reasoning replay preserved) |
  | `response.function_call_arguments.delta` | *(not surfaced — codex drops it)* | — |
  | `response.output_item.done` (function_call) | — | **finalize the tool call from the item's complete `arguments` string** (no per-delta assembly needed) |
  | `response.completed` | — | usage + stop; end the stream |
  | `response.failed` / `.incomplete` / `error` | — | map → `ProviderError` (reuse `openai::classify`) |

- **Key difference from Anthropic:** Responses delivers **complete tool
  arguments** in `output_item.done`, so the `ToolCallAssembler` is optional here —
  args come whole (Agent D; codex `turn.rs:2062,2136`). Usage arrives only at
  `response.completed`. Stream that ends without `response.completed` → error
  (codex `responses.rs:516-522`).
- **Same byte-identical invariant** vs the non-streaming Responses `parse`.

### Slice 2 test matrix (delta from Slice 1)

- Golden Responses SSE fixture → `stream` **byte-identical** to `complete`.
- Tool call finalizes from `output_item.done` args (never partial).
- Reasoning deltas captured; encrypted-reasoning replay block intact in the final
  `Completion`.
- `response.failed`/incomplete/stream-truncated → correct `ProviderError`.
- Engine + TUI: unchanged (they already consume `CompletionDelta`); a smoke test
  that `api_schema = "openai-responses"` streams end-to-end in the TUI.

### Slice 2 risks

- Building the event types faithfully (a lot of `response.*` variants) — port
  codex's set, cover the reasoning + custom-tool sub-channels, `_ => trace-and-skip`
  the rest.
- Stateless (`store:false`) + streaming interaction through OpenRouter — verify
  live as the non-streaming wire's plan did.

---

# Slice 3 — incremental markdown in the streaming cell (study Phase 4)

**Goal:** replace Slice 1's plain-text live cell with **incremental markdown**, so
streamed prose renders with the same formatting as a finalized `AssistantText`
block — unblocking markdown study **Phase 4**. No provider/engine/protocol
changes; this is TUI-only.

### Scope

- Adopt **codex's markdown-streaming model** (ADR-0021 §4; study): buffer the raw
  delta text, **re-parse the whole buffer gated at newline boundaries**, render
  the **stable prefix** (completed lines) as committed markdown and keep the
  **in-progress last line as a mutable tail**. Re-use `crate::ui::markdown::render`
  (`markdown.rs:23`, pure over `(text, width)`) — re-render the growing buffer each
  paint.
- **Provable invariant:** the streamed final frame == the finalized
  `AssistantText` render for the same text (so finalize is a no-op visual swap).
- **Deferred (not this slice):** codex's 120 Hz line-reveal "typing animation" and
  the ~16 ms partial-line flush timer (Q3) — add only if long lines feel laggy.

### Slice 3 test matrix

- "streamed frame == final frame": feed a markdown doc as deltas, assert the last
  streamed render equals `Block::AssistantText(doc).render(width)`.
- Newline-gated re-parse: an unbalanced fence / half-table mid-stream doesn't
  panic or flash a broken render (stable prefix only).
- Wide content (tables/code) inside the live cell wraps/gates correctly.

### Slice 3 risks

- Markdown re-parse flicker on incomplete blocks — the newline gate + stable-prefix
  is the mitigation (codex's exact reason).

---

# Cross-cutting

- **Quality gate** each slice: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -D warnings`, `cargo test --workspace`. Wire slices add golden SSE
  fixtures (as the non-streaming wires did).
- **Public-surface changes** (ask-first, already ADR-approved): `Provider::stream`
  + `CompletionDelta` (`locode-provider`), `Event::MessageDelta` (`locode-protocol`),
  `EngineConfig.stream_deltas` (`locode-engine`), facade re-exports for the new
  types. Each rides its own PR.
- **ADR note:** when Slice 1 lands, add a one-line ADR-0021 amendment recording the
  `stream_deltas` flag refinement (engine chooses `stream` vs `complete`) so the
  ADR stays authoritative.
- **Ordering:** Slice 1 → 2 → 3, each independently shippable. Slice 1 is the only
  one that touches the core public surface end-to-end; 2 is wire-local; 3 is TUI-local.
- **Stays deferred** (per ADR-0021): `--include-deltas` trace flag, inline/transcript
  thinking UI, the 120 Hz typing animation, the long-line flush timer.

## Open implementation decisions (flagged for the slice PRs)

1. **`stream_deltas` flag vs blanket `stream`** (1e) — recommended: the flag (keeps
   `-p` unchanged). Confirm at Slice 1.
2. **`MessageDelta` body** (1d) — text-only, or text+thinking? Recommended text-only
   (UI ignores thinking); thinking correctness is handled in the finalized `Message`,
   not the delta.
3. **Mock streaming granularity** (1c) — per-word vs per-line chunking for the test
   mock. Cosmetic; per-word exercises coalescing harder.
