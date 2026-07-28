# ADR-0007: `Provider` trait over an API-agnostic request; Anthropic Messages wire first

## Status
Accepted

## Date
2026-07-17

## Context
The sampling layer is as much a part of the harness as the tools. Three of four studied systems hand-roll their provider client (a few hundred lines) for full control over caching, retries, and usage; OpenCode delegates to the Vercel AI SDK and is migrating back to native. Grok proves the right shape: build **one API-agnostic `ConversationRequest`**, then convert it to whichever wire the model speaks. Our primary target model is Claude.

## Decision
Hand-roll the client behind a **`Provider` trait** over an API-agnostic `ConversationRequest` (messages, pack-provided tools, sampling params, cache hint). *(As built: no separate `system` field — the wire hoists leading `System` messages, ADR-0013; tools are faithful per-pack, not "dialect-skinned" — ADR-0012; the trait method identifying the wire is `api_schema()`, and the per-model config record is the modest `{ api_schema, base_url, api_key, model }` built at Task 12/14.)* Implement **one wire first: Anthropic Messages** (Claude is primary) — so `cache_control` breakpoint caching, thinking blocks, and the "omit temperature when thinking is on" quirk are handled from day one. OpenAI Chat Completions is the planned second wire; OpenAI Responses the third. *(Amended 2026-07-19: this order **inverted** — OpenAI **Responses** shipped as the second wire (`openai-responses`, Task 18; stateless, freeform tools, encrypted-reasoning replay); a Chat Completions wire was deferred, not built. And streaming, called sufficient-non-streaming below, later shipped additively — ADR-0021.)* Non-streaming was sufficient for v0. Keep a per-model `{ base_url, api_backend, extra_headers, env_key }` record (Grok's shape) plus one env override (`LOCODE_BASE_URL` + `LOCODE_API_KEY`) for the common case.

Baked into the trait's contract (or a shared wrapper): two-tier retry (transport backoff+jitter honoring `Retry-After`; loop-level rebuild-and-resample, bounded); **surface 429s** rather than silently hammer; treat context-overflow and quota as terminal; parse usage from the terminal event; **client-side token estimate** (~bytes/4) with the authoritative input count taken from response `usage`; preserve provider tool-call ids verbatim; run the transcript repair/dedup pass before every send.

## Alternatives Considered
### Delegate to an SDK (Vercel AI SDK / provider SDKs)
- Rejected: per-model special-case sprawl and loss of control over caching/usage/retries; OpenCode is actively walking this back.

### OpenAI Chat Completions first (widest reach)
- Considered and inverted: with Claude as the primary target, Anthropic-native caching and thinking are worth more in v0. The trait makes wire order reversible; Chat Completions is the natural second for reach (OpenRouter/Together/Groq/vLLM/Ollama via base-URL override).

### Hard-wire a single Anthropic client, no trait
- Rejected: the trait is the deliverable; extra wires must be additive, not a fork.

## Consequences
- The loop builds one request and knows nothing about any wire; adding a provider is one file implementing the trait.
- Prompt caching is Anthropic-`cache_control` in v0; the stable `prompt_cache_key` becomes the OpenAI-family concern when that wire lands.
- Base-URL override often changes the auth header (native `x-api-key` → proxy `Authorization: Bearer`), so auth lives in the per-model record, not a global constant.

## Amendment (2026-07-18): OpenRouter backend + default betas

Decided in the Task-12 pre-implementation review (full detail and citations:
`tasks/plans/task-12-anthropic-wire.md` §9). The user's primary backend is
OpenRouter's Anthropic-compatible Messages endpoint, which needs more than the
generic proxy path:

- **`ApiBackend` gains a first-class `OpenRouter` variant** (`Native | OpenRouter |
  Proxy`), auto-detected from a `base_url` host of `openrouter.ai` (pinnable). It
  selects `Bearer` auth, mirrors the beta list onto OpenRouter's
  **`x-anthropic-beta`** header (in addition to `anthropic-beta`), and injects a
  default `provider` preferences body field
  (`{ignore:["amazon-bedrock"], allow_fallbacks:false, require_parameters:true}`,
  config-overridable) — `require_parameters:true` prevents OpenRouter routing to a
  backend that silently drops `cache_control`/`thinking`. Reference implementation:
  the user's `cc-reverse-proxy` repo. A vendor variant is preferred over generic
  knobs so the daily path stays two env vars; `Proxy` + `extra_headers` remains the
  generic escape hatch.
- **Default betas are no longer empty:** v0 ships
  `["interleaved-thinking-2025-05-14"]` by default (user requirement; Claude Code's
  own default; proxy-safe per OpenRouter's docs). Rule going forward: **the default
  beta set must be proxy-safe**; first-party-only betas stay opt-in. With this beta,
  the thinking `budget_tokens` clamp to `max_tokens-1` is waived (the API allows
  budgets exceeding `max_tokens` when thinking is interleaved).
- **Config record env grows `LOCODE_MODEL`** alongside `LOCODE_BASE_URL` /
  `LOCODE_API_KEY`; the v0 default model is `claude-sonnet-5`. The wire-identity
  string is plain `"anthropic"`.

## Amendment (2026-07-19): reasoning-effort ladder + `Config` error

`ReasoningEffort` extends to `None | Minimal | Low | Medium | High | XHigh |
Other(String)` (`#[non_exhaustive]`): effort tiers fragment per vendor/model
generation, so the ladder covers the observed union and `Other` passes
vendor-specific strings through verbatim. Wires that take effort strings send
them as-is and let the API's own error surface (never silently clamp — that
would corrupt eval comparisons); wires with fixed mappings (the Anthropic
Budget encoding) reject `Other` pre-send with the new terminal
`ProviderError::Config` variant. `SamplingArgs.reasoning_effort: None` (outer
Option) still means "omit the parameter".

## Amendment (2026-07-25): the output-token budget, and dropping the silent cap

`SamplingArgs.max_tokens` defaults to **64k** (`DEFAULT_MAX_TOKENS`), up from
4096, and `ModelConfig.max_tokens_cap` becomes `Option<u32>` defaulting to
**`None`** on both wires.

**The budget.** For a file-writing agent this is not a reply-length knob: a
turn's output is dominated by one `tool_use` argument blob, so it is really the
ceiling on the largest single tool call the model can emit. At 4096 the wire
truncated ordinary `Write` calls, and truncation is silent by construction —
the API returns the `tool_use` with an empty `input` (see the ADR-0004/0005
amendments of the same date). 64k is Claude Code's `ESCALATED_MAX_TOKENS`
(`utils/context.ts:25`), the value it retries at after exactly this failure,
and the largest value correct on every model this crate targets. Not 128k: the
frontier models allow it but `upperLimit` is per model and Haiku 4.5 stops at
64k, and a tool call whose arguments exceed 64k tokens is not one worth
completing. It is a ceiling, not a reservation — nothing is spent unless the
model generates it.

**Why the field cannot be `Option`.** The Anthropic Messages API requires
`max_tokens` on every request; "let the API decide" is not available on this
wire. opencode encodes the asymmetry exactly — `max_tokens: Schema.Number`
(required) for Anthropic against `Schema.optional` for both OpenAI protocols —
and always sends a value, falling back to the model's declared output limit
(`protocols/anthropic-messages.ts:510,546`). Codex sends no sampling cap
because the Responses API does not require one; Grok Build's `Option` collapses
to `unwrap_or(0)` when it lowers onto Messages
(`xai-grok-sampling-types/src/conversation.rs:3265`), which is not a model to
copy. Our OpenRouter gateway happens to tolerate an omitted `max_tokens` by
filling its own default — verified live — but `ApiBackend::Native` would 400,
so the leniency of one gateway is not a contract to build on.

**Why the cap goes away.** A ceiling applied as a `min` is silent by
construction: a caller who deliberately asks for more gets less, with no error
and no way to notice. This ADR already rejects that shape for
`reasoning_effort` — "never silently clamp — that would corrupt eval
comparisons" — and the output budget is no different. The wire now forwards the
caller's value and lets the API's own error surface. `Some(n)` remains, for
pinning a model whose real ceiling is lower than the default (e.g.
`claude-3-haiku` at 4096), where the clamp turns a 400 into a working request.

The prior value was 8000, credited to Claude Code's
`CAPPED_DEFAULT_MAX_TOKENS` — a miscitation twice over: that constant is a
*default*, not a ceiling, and it is gated on the `tengu_otk_slot_v1`
slot-reservation experiment, off by default outside first-party
(`services/api/claude.ts:3394-3397`).

**Breaking:** `ModelConfig.max_tokens_cap` changes type on both wires and
`anthropic::config::DEFAULT_MAX_TOKENS_CAP` is removed. Downstream code that
set the field passes `Some(n)`; code that relied on the implicit 8k/32k
ceilings now sends the caller's budget instead. Acceptable pre-1.0, and the old
ceilings were the bug.

**Deferred:** the per-model table (Claude Code's and opencode's shape). One
safe default plus an opt-in pin is the honest v0.

## Amendment (2026-07-25): the Anthropic wire always sends adaptive thinking

`thinking: {type: "adaptive", display: "summarized"}` is now **unconditional**
on this wire (user decision). `ReasoningEncoding` is removed along with the
`budget_tokens` encoding it selected; `reasoning_effort` now chooses only the
depth, rendered as `output_config.effort`.

**Why unconditional.** `SamplingArgs.reasoning_effort` defaults to `None` and
nothing in the TUI, exec, or engine ever set it, so `map_reasoning` returned
early and the request carried no `thinking` field at all. That was read as
"thinking off". It is not: omitting the field means *the serving model decides*
— Fable 5 thinks unconditionally and rejects `{type:"disabled"}`, Opus 5 runs
adaptive when the field is absent. Traces showed reasoning we never asked for,
and the same request behaved differently per model. Sending the field states
what was already happening, and makes it the same everywhere.

**Why the Budget encoding is gone, not just off by default.** It emitted
`{type: "enabled", budget_tokens: N}` from a fixed ladder (Low 4096 … XHigh
32768). That shape is **removed on every model this wire targets** — Fable 5,
Opus 5, Opus 4.8/4.7 and Sonnet 5 all 400 on it; only Opus 4.6 / Sonnet 4.6
still accept it, deprecated. It was `ReasoningEncoding::default()`, so the
default configuration was broken for every current model; it survived only
because the branch was unreachable. Keeping a variant that cannot work on any
supported model is keeping a footgun. Live check through a lenient gateway
confirmed the failure is worse than a 400 there: the parameter is silently
dropped, and Sonnet 5 returned no thinking at all while appearing to succeed.

**`display: "summarized"`, not the default.** The API default is `"omitted"`,
which still streams thinking blocks and still bills the same — it just empties
the text while keeping a multi-KB signature. A real trace from this repo showed
318 characters of reasoning against a 2044-character signature; under the
default it would have been 0 against 2044. Display is a visibility knob, and an
agent whose traces are the debugging surface wants it on.

**Consequences.** `temperature` is never sent on this wire (the API requires
temp=1 whenever thinking is on, and the current models reject the field
outright) — the neutral `SamplingArgs.temperature` stays meaningful for other
wires. `EFFORT_BETA` is removed: `output_config.effort` is GA and needs no beta
header, and the const was never attached to `betas` anyway. The pre-send
`ProviderError::Config` rejection of `ReasoningEffort::Other` is gone — every
tier now rides through verbatim and an unsupported one surfaces the API's own
error, which is what this ADR asks for.

**Breaking:** `anthropic::ReasoningEncoding` and `anthropic::config::EFFORT_BETA`
are removed, as is `ModelConfig.reasoning_encoding`.

**Known gap:** adaptive thinking is unsupported on pre-4.6 models (Sonnet 4.5,
Haiku 4.5 and older), which want `budget_tokens`. Those are out of scope by the
same decision — if a cheap older model is ever wanted for subagents, this wire
needs the per-model table that is already deferred above.

## Amendment (2026-07-25): locode owns the effort ladder

Effort is now a first-class, **locode-named** setting: a `--effort` flag on both
CLIs, an `effort` settings key, and an `/effort` command — all speaking
`locode_provider::Effort` (`low` · `medium` · `high` · `xhigh` · `max`), which
each wire maps onto its own vocabulary. `ReasoningEffort` gains a `Max` tier so
the deepest rung is first-class rather than riding through as `Other("max")`.

**Why a ladder of our own.** Effort naming is not portable — vendors ship
different tiers under different names, and a tier valid on one model is a 400 on
the next. Exposing the provider's vocabulary directly would make `/effort` mean
different things on different days and break the moment the model changes. The
five rungs mirror Anthropic's because Anthropic is what we run today and a 1:1
mapping is the honest starting point; the indirection exists so a wire with
fewer tiers collapses rungs in *its* mapping rather than forcing a different
menu on the user. `Effort::maps_to` surfaces that mapping in the menu's second
column, so a collapse is visible rather than silent.

**The rungs were verified against the live API, not read off the vendored
source.** The Claude Code snapshot predates Fable 5 and lists only
`low|medium|high|max` (`utils/effort.ts:14-19`). Probing `claude-fable-5`,
`claude-opus-5` and `claude-sonnet-5` directly: `low`, `medium`, `high`,
`xhigh`, `max` are all accepted; `ultra`, `ultrathink`, `extreme` are 400s.
**`max` is the top rung**, with `xhigh` between `high` and it.

**"ultracode" is deliberately not a rung.** Claude Code's own `/effort` UI
settles this (user screenshot, 2026-07-26): the slider runs
`low · medium · high · xhigh · max`, and `ultracode` sits **past a divider**,
annotated *"xhigh + workflows"*. It is a composite mode — an effort rung plus
behavior — not a sixth level, which is why the API accepts `xhigh` and `max`
but 400s `ultracode`. The related "ultrathink" is likewise a keyword matched in
the *prompt* (`utils/thinking.ts:45`) that bumps effort for that turn
(`utils/effort.ts:321`). Both are layers over the setting; modelling either as
a rung would conflate two mechanisms and put a value on the wire that the API
rejects.

**`auto` clears rather than sets.** `/effort auto` (and an absent flag/setting)
sends no `output_config`, leaving the API its own default — mirroring Claude
Code's `/effort [low|medium|high|max|auto]`. Precedence is flag > setting >
auto; an unparseable settings value warns and degrades to auto rather than
failing a run (ADR-0024 §1.5 tolerance).

## Amendment (2026-07-27): a wire owes two guarantees — valid output, and loud loss

A retrying reverse proxy dropped the SSE frames of one short message. The turn
still ended in `message_stop`, so the wire reported success, and an **empty text
block** — invented by the stream assembler to keep block indices addressable —
was recorded into the session. Anthropic *emits* empty text blocks but **rejects
them on input**, and the whole history is replayed every turn, so from that
record on, every request failed identically with
`invalid_request_error: messages: text content blocks must be non-empty`. The
session could never be continued again; retries never left the client.

The upstream defect is the proxy's. What this amendment fixes is ours: **a
transient upstream glitch became permanent local state, and a lossy stream was
reported as a finished turn.** A `Provider` implementation therefore owes two
guarantees, independent of how well-behaved the service is:

**1. Never construct a request the target API will reject.** History arrives from
the engine as protocol types; the wire is the layer that knows its API's shape
rules, so normalizing to them is its job — not the engine's, and not the
transcript's. Concretely for Anthropic: blank (empty or whitespace-only) text
blocks are dropped in `build.rs`, which is also what *heals* an already-poisoned
session — the block simply stops being sent, with no file surgery. The
message-level `if !blocks.is_empty()` guard already in `map_messages` completes
it: a message emptied by the filter is skipped whole, so this can never produce
the `content: []` the API also rejects. Dropping blank text can never orphan a
`tool_use` — a message carrying one keeps that block and stays non-empty
(ADR-0004), pinned by `dropping_blank_text_cannot_orphan_a_tool_use`.

Blank blocks are also dropped at parse time so they never reach the transcript or
the rollout. A response left with no content at all is not special-cased here: the
engine already resamples empty completions, which is the right answer.

**2. A lossy stream is a failure, not a short answer.** "Stream truncated" was
already detected (no `message_stop` → retryable transport error). "Stream complete
but lossy" was not — it was reported as success. Three signals now raise the same
retryable transport failure, so the engine's existing resample handles them:

- a frame whose `type` we recognize but whose payload will not deserialize. This
  used to take the *forward-compatibility* path, which is the actual defect: an
  unknown event type and a corrupted known one are opposite situations and must
  not share a branch (`KNOWN_EVENTS` splits them);
- `stop_reason: tool_use` with no `tool_use` block assembled — impossible on an
  intact stream, and previously accepted as a bare text turn, letting the run
  continue past a tool call that never happened;
- a delta addressing a block index that never started, or a block of the wrong
  kind — previously a silent no-op.

The principle behind both: **a provider hiccup should cost one turn, not the
session.** Recoverability is the property being designed for, not perfection —
we make no attempt to reconstruct what was lost, only to refuse to build on it.

**Known consequence, not yet addressed.** Making more streams retryable makes an
existing display bug more visible: `sample_with_retry` resamples the same request
after deltas have already been emitted, so a UI that appended the partial text
shows it twice, and rows already committed to scrollback cannot be withdrawn.
Fixing it properly needs a "discard the in-flight partial" signal in the event
protocol (ADR-0014) — a core public-surface change, so it is **flagged for the
user rather than taken here**.
