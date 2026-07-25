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
