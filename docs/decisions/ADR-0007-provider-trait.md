# ADR-0007: `Provider` trait over an API-agnostic request; Anthropic Messages wire first

## Status
Accepted

## Date
2026-07-17

## Context
The sampling layer is as much a part of the harness as the tools. Three of four studied systems hand-roll their provider client (a few hundred lines) for full control over caching, retries, and usage; OpenCode delegates to the Vercel AI SDK and is migrating back to native. Grok proves the right shape: build **one API-agnostic `ConversationRequest`**, then convert it to whichever wire the model speaks. Our primary target model is Claude.

## Decision
Hand-roll the client behind a **`Provider` trait** over an API-agnostic `ConversationRequest` (messages, pack-provided tools, sampling params, cache hint). *(As built: no separate `system` field — the wire hoists leading `System` messages, ADR-0013; tools are faithful per-pack, not "dialect-skinned" — ADR-0012; the trait method identifying the wire is `api_schema()`, and the per-model config record is the modest `{ api_schema, base_url, api_key, model }` built at Task 12/14.)* Implement **one wire first: Anthropic Messages** (Claude is primary) — so `cache_control` breakpoint caching, thinking blocks, and the "omit temperature when thinking is on" quirk are handled from day one. OpenAI Chat Completions is the planned second wire; OpenAI Responses the third. Non-streaming is sufficient for v0. Keep a per-model `{ base_url, api_backend, extra_headers, env_key }` record (Grok's shape) plus one env override (`LOCODE_BASE_URL` + `LOCODE_API_KEY`) for the common case.

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
