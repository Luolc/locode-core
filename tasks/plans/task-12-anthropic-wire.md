# Task 12 — `locode-provider` Anthropic Messages wire (the one live `Provider`)

> **Resolved since writing:** the Task-5 provider surface is now shipped, closing several
> §8 open questions — `ConversationRequest` has **no `system` field** (the wire hoists
> leading System messages); `Completion` carries `Vec<ContentBlock>` (thinking preserved);
> `StopReason` is `#[non_exhaustive]` + `Unknown(String)`; `repair_pairing` lives in
> `locode-provider` (the wire calls it before send); the wire-identity field is
> `api_schema`. See `tasks/plans/README.md`. Sections below predate these.
>
> **Addendum §9 (2026-07-18):** the pre-implementation review resolved the remaining §8
> questions and **supersedes two defaults below** — `betas` is **no longer empty** (v0
> ships `interleaved-thinking-2025-05-14` by default, §9.3) and `ApiBackend` gains a
> first-class **`OpenRouter`** variant (§9.2). Read §9 before implementing.

> Implementation plan, written **before** code. Correctness of caching / retry /
> id-pairing / thinking-replay is the point of this task; everything else is
> plumbing. Cites harness source under
> `~/dev/coding-cli-survey/submodules/{grok-build,claude-code,codex}` as `file:line`.
> Repo ADRs: ADR-0007 (Provider trait), ADR-0013 (4-role protocol), ADR-0004
> (pairing), ADR-0009 (report). Grok Build's Anthropic-Messages backend is the
> closest working model and is cited throughout.

---

## 1. Purpose & scope

Implement the single **live** `Provider` for locode-core: Anthropic Messages
(`POST /v1/messages`), **non-streaming**, driving Claude (the primary target
model, SPEC.md:13, ADR-0007). It converts our provider-neutral
`ConversationRequest` (message stream + tools + sampling + cache hint) into a
Messages request, sends it over `reqwest`, and parses the JSON response back into
a `Completion` — preserving tool-call ids verbatim, thinking blocks *with*
signature, and usage. It owns the **transport-tier retry** (backoff + jitter +
`Retry-After`, 429 surfaced, 401 refresh-once, terminal classification of
context-overflow / quota) and runs a **defensive pre-send transcript repair/dedup**.

**In scope (v0):**
- `ConversationRequest → MessagesRequest` build (System hoist to top-level
  `system`; Developer → portable `<system-reminder>` user block by default;
  User/Assistant/tool_result placement; `cache_control` breakpoints; omit
  temperature when thinking on; `reasoning_effort → thinking` mapping).
- `MessagesResponse → Completion` parse (verbatim tool_use ids; Thinking+signature;
  usage; stop-reason mapping).
- Transport-tier retry wrapper + error classification → `ProviderError`.
- Per-model config record + `LOCODE_BASE_URL` / `LOCODE_API_KEY` env override;
  native `x-api-key` vs proxy `Bearer` auth.
- Fixture/recorded-response tests (no live key in CI).

**Deferred (reserved seams, not v0):**
- **Streaming** — SSE consumption + partial-JSON assembly. The wire *types* for it
  already exist (`MessageStreamEvent`, `StreamDelta`, cf. grok `messages.rs:243-319`)
  and the crate already ships a `ToolCallAssembler` (Task 5) for when streaming
  lands. Non-streaming is sufficient for v0 (SPEC.md:16, ADR-0005). Claude Code's
  non-streaming path is itself a *fallback* from streaming
  (`survey/01-claude-code/provider-api.md:58`) — we make it the primary.
- **Second wire** (OpenAI Chat Completions / Responses) — ADR-0007; the trait
  makes it additive.
- **Multi-cloud** Anthropic (Bedrock/Vertex/Foundry) — Claude Code carries four
  SDKs (`provider-api.md:72`); we ship first-party + base-URL proxy only.
- **OAuth subscription** flow + **model fallback on 529** (Claude Code's
  `FallbackTriggeredError`, `provider-api.md:67`) — v0 is static-key, single-model.
- **Loop-level (tier-2) resample / compaction** — lives in the engine (Task 6),
  not here (see §4.6). Grok splits the same way: sampler owns transport retry,
  the shell owns `CompactAndResubmit` / `RefreshAuthAndResubmit`
  (`survey/03-grok-build/provider-api.md:52-56`).

---

## 2. Module layout

```
crates/locode-provider/src/
├── lib.rs                 # (Task 5) Provider trait, ConversationRequest, Completion,
│                          #          ProviderError, SamplingArgs, CacheHint, ToolCallAssembler
├── mock.rs                # (Task 5) MockProvider
└── anthropic/
    ├── mod.rs             # AnthropicProvider (the struct) + `impl Provider`; wires build→send→parse→retry
    ├── config.rs          # ModelConfig / AuthScheme / ApiBackend; env resolution (LOCODE_BASE_URL/_API_KEY)
    ├── wire.rs            # serde structs for the Messages request/response (ported from grok messages.rs)
    ├── build.rs           # build_request(&ConversationRequest, &ModelConfig) -> wire::MessagesRequest
    ├── parse.rs           # response_to_completion(wire::MessagesResponse) -> Completion; map_stop_reason
    ├── retry.rs           # transport-tier retry loop + backoff/jitter + Retry-After; run_with_retry
    ├── error.rs           # HTTP status + wire error body -> ProviderError classification
    └── client.rs          # reqwest client construction, header assembly, one send()
crates/locode-provider/tests/
├── anthropic_request_shape.rs   # cache-marker count, temp-omit, system hoist, id round-trip
├── anthropic_parse.rs           # thinking+signature, usage, tool_use id preservation, stop mapping
├── anthropic_retry.rs           # 5xx vs 429 vs terminal classification; Retry-After honored
└── fixtures/                    # recorded JSON request/response bodies
```

Rationale for splitting `wire.rs` from `build.rs`/`parse.rs`: the wire structs are
pure serde DTOs (mechanical, testable in isolation, mirror grok's
`xai-grok-sampling-types/src/messages.rs`); the *conversion* is where the
Anthropic-specific judgement lives (hoisting, cache placement, temp rule) and
deserves its own tested surface — exactly grok's separation
(`messages.rs` types vs `conversation.rs:2973` `build_messages_request`).

---

## 3. Key types & signatures (concrete sketches)

### 3.1 Config / per-model record (`config.rs`)

Grok's per-model `{ base_url, api_backend, extra_headers }` shape (ADR-0007
decision; `survey/05-comparative/sampling-comparison.md:59`), plus the auth split
that base-URL override forces (native `x-api-key` → proxy `Bearer`;
ADR-0007 consequence line 30; Claude Code `ANTHROPIC_API_KEY` vs
`ANTHROPIC_AUTH_TOKEN`, `provider-api.md:57`).

```rust
pub struct ModelConfig {
    pub model: String,                       // "claude-sonnet-4-6" etc.
    pub base_url: String,                    // default "https://api.anthropic.com"
    pub api_backend: ApiBackend,             // Native | Proxy — selects the auth header
    pub auth: AuthScheme,                    // resolved key/token
    pub anthropic_version: String,           // "2023-06-01" (claude-code betas.ts callsites)
    pub betas: Vec<String>,                  // anthropic-beta header values, latched per session
    pub extra_headers: Vec<(String, String)>,// ANTHROPIC_CUSTOM_HEADERS analogue
    pub max_tokens: u32,                     // request cap (Claude Code CAPPED_DEFAULT_MAX_TOKENS=8000)
    pub developer_rendering: DeveloperRendering, // SystemReminder (default) | MidConversationSystemBeta
    pub reasoning_encoding: ReasoningEncoding,   // Budget (default) | EffortAdaptive
}

pub enum ApiBackend { Native, Proxy }        // Native => x-api-key; Proxy => Authorization: Bearer
pub enum AuthScheme { ApiKey(String), Bearer(String) }
pub enum DeveloperRendering { SystemReminder, MidConversationSystemBeta }
pub enum ReasoningEncoding { Budget, EffortAdaptive }

impl ModelConfig {
    /// Env override for the common case: LOCODE_BASE_URL + LOCODE_API_KEY.
    /// A bare LOCODE_BASE_URL that isn't api.anthropic.com flips api_backend=Proxy
    /// (Bearer) unless the caller pins it — matching the "base-url override changes
    /// the auth header" rule (ADR-0007:30).
    pub fn from_env_for(model: &str) -> Result<Self, ProviderError>;
    fn auth_header(&self) -> (HeaderName, HeaderValue); // (x-api-key | authorization)
}
```

### 3.2 Wire structs (`wire.rs`) — ported from grok `messages.rs`

Reuse grok's `messages.rs` types almost verbatim (they are already correct and
carry the `StopReason::Unknown(String)` catch-all, `messages.rs:209-226`, and the
`cache_control` fields). **Two required deltas from grok's version:**

1. **`ToolResult` must carry `is_error`.** Grok's `ContentBlock::ToolResult`
   (`messages.rs:114-119`) has only `{tool_use_id, content, cache_control}` — it
   drops `is_error`. Our protocol `ContentBlock::ToolResult` *has* `is_error`
   (ADR-0013), and Anthropic honours `"is_error": true` on a `tool_result`. Add it:
   ```rust
   ToolResult {
       tool_use_id: String,
       content: ToolResultContent,
       #[serde(skip_serializing_if = "std::ops::Not::not")] is_error: bool,
       #[serde(skip_serializing_if = "Option::is_none")] cache_control: Option<CacheControl>,
   },
   ```
2. **`cache_control` reach on the last-message marker.** Grok only ever marks the
   last *system* block (`conversation.rs:3191-3196`). We additionally mark the last
   *message*, so the block type that ends the last message needs a `cache_control`
   slot. `Text` and `ToolResult` already have one; `ToolUse`/`Image` do not (and
   won't be the final block right before a sample — the last message before
   sampling is a User turn ending in text or tool_result). Place the marker on the
   last cache-capable block; if none, skip (documented invariant, §4.3).

Everything else (`MessagesRequest`, `SystemParam`, `ThinkingConfig`,
`OutputConfig`, `MessagesResponse`, `MessagesUsage`, `StopReason`) is copied. Keep
the streaming types (`MessageStreamEvent`, `StreamDelta`) present-but-unused for
the deferred streaming path.

### 3.3 The provider struct (`mod.rs`)

```rust
pub struct AnthropicProvider {
    http: reqwest::Client,
    config: ModelConfig,
    retry: RetryPolicy,
    auth_refresh: Option<Arc<dyn AuthRefresh>>, // 401 -> refresh once -> retry (§4.5)
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn api_schema(&self) -> &str { "anthropic-messages" }

    async fn complete(&self, req: &ConversationRequest) -> Result<Completion, ProviderError> {
        // 1. defensive transcript repair/dedup on a clone (§4.7)
        // 2. build_request(req, &self.config)  (§4.1–4.4)
        // 3. run_with_retry(|| self.client.send(&wire_req))  (§4.6)
        //    -- inside: 401 -> refresh_once -> retry (§4.5)
        // 4. response_to_completion(resp)  (§4.4)
    }
}

pub trait AuthRefresh: Send + Sync {
    /// Re-resolve the credential after a 401. Returns Some(new AuthScheme) iff a
    /// *different* credential was obtained (mirrors grok
    /// refresh_after_unauthorized, survey/03-grok-build/provider-api.md:60).
    fn refresh(&self) -> Option<AuthScheme>;
}
```

### 3.4 Request build (`build.rs`)

```rust
pub fn build_request(req: &ConversationRequest, cfg: &ModelConfig) -> wire::MessagesRequest;
```

### 3.5 Response parse (`parse.rs`)

```rust
pub fn response_to_completion(resp: wire::MessagesResponse) -> Completion;
fn map_stop_reason(sr: Option<wire::StopReason>) -> protocol::StopReason;
```

### 3.6 Retry (`retry.rs`)

```rust
pub struct RetryPolicy {
    pub max_attempts: u32,     // ~8 (Claude Code default 10; codex stream_max_retries 5)
    pub base_delay: Duration,  // 500ms (Claude Code 500ms·2^(n-1))
    pub max_delay: Duration,   // 32s cap (Claude Code)
    pub rate_limit_attempts: u32, // 2 (grok caps 429 at 2, survey 03:46)
    pub retry_after_cap: Duration, // 120s (grok client.rs:211)
}

pub async fn run_with_retry<F, Fut>(policy: &RetryPolicy, mut op: F) -> Result<Completion, ProviderError>
where F: FnMut(u32) -> Fut, Fut: Future<Output = Result<Completion, ProviderError>>;

fn backoff(policy: &RetryPolicy, attempt: u32, retry_after: Option<Duration>) -> Duration;
// Retry-After takes precedence and bypasses the exp cap (Claude Code provider-api.md:64),
// itself capped at retry_after_cap. Jitter ±(0.9..1.1) (codex retry.rs:45).
```

### 3.7 Error classification (`error.rs`)

```rust
/// Map an HTTP status + parsed error body (or transport failure) to ProviderError.
pub fn classify(status: Option<StatusCode>, x_should_retry: Option<bool>,
                retry_after: Option<Duration>, body: &ErrorBody) -> ProviderError;
```
See §4.6 for the classification table.

---

## 4. Behavior / algorithms

### 4.1 Request construction — message-stream → Messages

Our `ConversationRequest.messages: Vec<protocol::Message>` is *already*
block-structured per message (ADR-0013), so we do **not** need grok's
`pending_assistant`/`pending_tool_results` accumulator dance
(`conversation.rs:3049-3069`) — that exists because grok's `ConversationItem` is
one-thing-per-item. We map each `Message` directly by role:

- **`Role::System`** → append its text blocks to `system_blocks` (top-level
  `system`), regardless of position. Leading System is the norm (ADR-0013
  "front-loaded"); a stray mid-stream System is still hoisted, matching grok
  which routes every `ConversationItem::System` into `system_blocks`
  (`conversation.rs:3074-3082`). Multiple System blocks concatenate in order.
- **`Role::Developer`** → **default (`DeveloperRendering::SystemReminder`)**: one
  `role:"user"` message whose text is wrapped
  `<system-reminder>…</system-reminder>` (ADR-0013 fallback row; the portable
  default). **Beta path (`MidConversationSystemBeta`)**: a `role:"system"`
  mid-conversation message — this requires (a) extending wire `MessageRole` with a
  `System` variant (grok's enum is `User|Assistant` only, `messages.rs:63-66`) and
  (b) adding the `mid-conversation-system` beta header. Deferred behind the flag;
  v0 ships the portable fallback so no beta gating is needed.
- **`Role::User`** → `role:"user"`; map each block: `Text→text`,
  `Image→image` (Base64/Url per `ImageSource`), `ToolResult→tool_result`
  (preserving `tool_use_id` and `is_error`). A User message may hold `tool_result`
  blocks — the Anthropic convention (ADR-0013; grok routes `ToolResult` into a
  user message, `conversation.rs:3061-3069,3150-3154`).
- **`Role::Assistant`** → `role:"assistant"`; map `Text→text`,
  `Thinking{text,signature}→thinking{thinking,signature}` (§4.2),
  `ToolUse{id,name,input}→tool_use` **preserving `id` verbatim** (§4.4).

`tools` → `ToolParam { name, description, input_schema }` straight from
`req.tools` (grok `conversation.rs:3209-3222`). `max_tokens` from
`sampling_args.max_tokens` (clamped to `cfg.max_tokens`). `top_p` passed through.
`temperature` — see §4.3.

**Consecutive-role merging** (Claude Code merges consecutive user turns because
*Bedrock* requires it, `provider-api.md:27`) is **not** needed for first-party
Anthropic — skip in v0, note as a Bedrock-only concern.

### 4.2 Thinking replay (send side) — signatures are load-bearing

When history contains an Assistant `Thinking{text, signature}` block, it **must**
be re-sent as a wire `thinking` block carrying the **same `signature`**. Anthropic
rejects (400) an assistant turn that used extended thinking + tool_use if the
thinking block (and its signature) is not echoed back verbatim on the next
request. Grok does exactly this replay: it emits an Anthropic `thinking` block
whose `signature` comes from the reasoning item's `encrypted_content`
(`conversation.rs:3169-3183`) — "`tco_*` encrypted blobs only set `signature`;
real model reasoning sets `thinking`" (`conversation.rs:3166-3168`). Our protocol
already keeps them contiguous (a Thinking block sits in the same Assistant message,
before its tool_use), satisfying the "reasoning must stay contiguous with the
following tool_use" rule (`sampling-comparison.md:71`).

Edge: `signature: None` (shouldn't occur for genuine Anthropic thinking — the
stream always attaches one, §4.4). If text present but signature missing, drop the
block on send rather than send an unsigned thinking block (safer than a 400); log
at debug. Emit-only-if-non-empty, matching grok's `if !thinking.is_empty() ||
!signature.is_empty()` guard (`conversation.rs:3177`).

### 4.3 `cache_control` breakpoints (the ≤4 / +1 rule) + temp-omit

Anthropic's hard limit is **4** `cache_control` breakpoints per request; a 5th is
a 400. Two placement families (`survey/05-comparative/sampling-comparison.md:66`,
`01-claude-code/provider-api.md:34-37`):

- **System blocks:** Claude Code marks up to 3–4 system text blocks, each with its
  own `cache_control`; source comment: *"Do not add any more blocks for caching or
  you will get a 400."* Grok marks exactly **one** — the *last* system block
  (`conversation.rs:3191-3196`).
- **Message level:** Claude Code places **exactly one** marker, on the **last
  message** (`addCacheBreakpoints`, `provider-api.md:37`); "single marker is
  deliberate — tuned to the server's KV-page eviction."

**v0 policy (`CacheHint::Standard`):** mark the **last system block** (1) **and**
the **last cache-capable block of the last message** (1) → **2 markers total**,
comfortably under 4. This is precisely Claude Code's minimal-agent takeaway ("two
`cache_control` markers: last system block + last message",
`provider-api.md:80`). `CacheHint::Off` → zero markers.

**Hard invariant (asserted + tested):** after build, count every `cache_control`
across `system` + `messages`; **`assert count <= 4`** (and, for `Standard`,
`count == 2` given a non-empty system + non-empty tail message). This is the
request-shape test in the acceptance criteria (Task 12). Marking >1 system block
is a future refinement (splitSysPromptPrefix-style, `provider-api.md:36`) — the
counter guard is what keeps that from ever crossing 4.

`SystemParam` serialization: reuse grok's rule — single block *without*
`cache_control` collapses to `SystemParam::Text`, otherwise `SystemParam::Blocks`
(`conversation.rs:3199-3206`). Since `Standard` always marks the last system
block, we take the `Blocks` form whenever caching is on.

**Omit temperature when thinking is on.** When the built request has a `thinking`
config (Enabled/Adaptive), **do not send `temperature`** — the API requires
`temperature = 1` with thinking and rejects any other value (`provider-api.md:31`,
Claude Code claude.ts, "omitted entirely when thinking is enabled",
`01-claude-code/provider-api.md:30-31,43`). This is an explicit **divergence from
grok**, which unconditionally forwards `req.temperature`
(`conversation.rs:3269`) and merely relies on callers leaving it `None`. We set
`temperature = None` deterministically in the thinking branch regardless of
`sampling_args.temperature`.

### 4.4 `reasoning_effort → thinking` mapping

`SamplingArgs.reasoning_effort: Option<ReasoningEffort{Minimal,Low,Medium,High}>`.
Two encodings exist in the wild:

- **Effort-based (grok Messages backend):** `output_config.effort` string +
  `thinking = Adaptive{display: Summarized}` (`conversation.rs:3232-3260`). The
  effort strings are `Low→"low" | Medium→"medium" | High→"high"` and grok maps
  `None|Minimal→None` (thinking off) (`types.rs:812-820`). This needs the
  `effort-2025-11-24` beta (claude-code `betas.ts:15`).
- **Budget-based (older Claude Code):** `thinking = Enabled{budget_tokens}`,
  clamped to `max_tokens - 1` (`01-claude-code/provider-api.md:43`); adaptive
  models instead get `thinking:{type:"adaptive"}`.

**v0 default = `ReasoningEncoding::Budget`** (no beta gating, widest model
support). Mapping:

| `ReasoningEffort` | `ThinkingConfig` (Budget) | note |
|---|---|---|
| `None` (absent) | none (thinking off) | temperature passes through |
| `Minimal` | `Disabled` / omit | grok treats Minimal as no-thinking (`types.rs:814`) |
| `Low` | `Enabled{ budget_tokens: min(4096, max_tokens-1) }` | |
| `Medium` | `Enabled{ budget_tokens: min(8192, max_tokens-1) }` | |
| `High` | `Enabled{ budget_tokens: min(16384, max_tokens-1) }` | clamp is mandatory |

`ReasoningEncoding::EffortAdaptive` (opt-in, newer models) instead emits
`output_config.effort` + `thinking=Adaptive{display:Summarized}` exactly like grok
(`conversation.rs:3250-3257`) and appends the `effort` beta. Whenever any thinking
config is emitted, §4.3's temperature-omit fires.

### 4.5 Response parse → `Completion`

Iterate `MessagesResponse.content` (grok `conversation.rs:3294-3315` is the shape,
but we **keep thinking** — grok drops it there only because its `From` returns a
single item, `conversation.rs:3282-3286,3311-3312`, and recovers it on the
*streaming* path):

- `Text{text}` → `ContentBlock::Text(text)`.
- `ToolUse{id,name,input}` → `ContentBlock::ToolUse{ id, name, input }` —
  **id preserved verbatim** (ADR-0007: "preserve provider tool-call ids
  verbatim"). We deliberately do **not** run grok's `sanitize_tool_call_id`
  (`conversation.rs:2986-2996`): grok sanitizes because ids can arrive from its
  Chat/Responses backends in non-Anthropic shapes; our ids come only from the
  Messages wire (`toolu_…`, already `[A-Za-z0-9_-]`), so sanitizing is a no-op that
  would only risk breaking the `tool_use.id ↔ tool_result.tool_use_id` pairing
  invariant (ADR-0004). (If a proxy ever returns exotic ids, sanitize **both**
  sides identically on send — pairing survives — but that's a documented future
  toggle, not v0.)
- `Thinking{thinking,signature}` → `ContentBlock::Thinking{ text: thinking,
  signature: Some(signature) }`. The signature is captured for replay (§4.2). On
  the streaming path this is where grok accumulates `signature` via
  `SignatureDelta` into `encrypted_content` (`stream/messages.rs:268-269,
  318-344`); non-streaming hands us the whole block at once.
- `Image` / `ToolResult` in an assistant response are unexpected → ignore (grok
  `conversation.rs:3313`).

**Usage:** `MessagesUsage → protocol::Usage` field-for-field:
`input_tokens→input_tokens`, `output_tokens→output_tokens`,
`cache_read_input_tokens→cache_read_tokens`,
`cache_creation_input_tokens→cache_creation_tokens` (`messages.rs:228-236`; our
`Usage` fields per the protocol). This is the **authoritative** input count; any
client-side `bytes/4` estimate (ADR-0007; grok `BYTES_PER_TOKEN=4`,
`03-grok-build/provider-api.md:64`) is the engine's concern, overwritten by this.

**Stop reason:** `map_stop_reason` translates wire `StopReason` → our
`protocol::StopReason` (`#[non_exhaustive]`, `Unknown(String)`). Known → known;
`ModelContextWindowExceeded` is surfaced as the stop *and* (if `content` is empty)
folded into a terminal `ProviderError::ContextOverflow` (§4.6); anything unknown →
`Unknown(raw)` (never fail the parse — same discipline as grok
`messages.rs:219-226`).

### 4.6 Retry — transport tier (this crate) vs loop tier (engine)

**Two tiers, everywhere** (`survey/05-comparative/sampling-comparison.md:38-49`).
This crate owns **tier-1 (transport)** only; **tier-2 (rebuild-and-resample /
compact)** is the engine's (Task 6, ADR-0007 "loop-level rebuild-and-resample,
bounded"). Grok draws the identical line — sampler = transport retry, shell =
`CompactAndResubmit`/`RefreshAuthAndResubmit` (`03-grok-build/provider-api.md:38-56`);
codex too (`responses_retry.rs` reconnect vs history rebuild).

**Classification table** (`classify`, `error.rs`):

| Condition | `ProviderError` | `retryable()` | tier-1 behavior |
|---|---|---|---|
| reqwest connect/timeout/read | `Transport` | yes | exp backoff + jitter |
| HTTP 5xx | `Api{status,message}` | yes | exp backoff + jitter |
| HTTP 529 **or** body `type:"overloaded_error"` | `Api{status:529}` | yes | backoff; Claude Code matches status **or** body since the SDK sometimes drops the 529 mid-stream (`provider-api.md:66`) |
| HTTP 429 | `RateLimited{retry_after}` | yes (**capped**) | honor `Retry-After`, **cap at `rate_limit_attempts=2`**, then **surface** — never hammer (`sampling-comparison.md:49`, grok 03:46) |
| HTTP 408/409 | `Api{status}` | yes | backoff (Claude Code retryable set, `provider-api.md:65`) |
| HTTP 401/403 | `Auth` | no (special) | **refresh once → retry** (§4.5 below), else terminal |
| HTTP 400 context/`prompt is too long` / 413 | `ContextOverflow` | **no (terminal)** | surfaced to engine → compact (Task 6) |
| HTTP 400/402 quota / "credit balance too low" / insufficient_quota | `Quota` | **no (terminal)** | surfaced |
| other 400/404/422 | `Api{status}` | no | terminal |
| 200 body fails to deserialize | `Decode` | no | terminal |

- **`x-should-retry` header:** `false` **forces terminal** (overrides the table);
  `true` is **ignored** (never *forces* a retry). This is grok's exact rule
  (`client.rs:214-227`, and "`x-should-retry: false` forces `Fatal`; `true` is
  ignored", `03-grok-build/provider-api.md:50`) and Claude Code honours the same
  header (`provider-api.md:65`).
- **Backoff:** `base_delay=500ms`, `delay = base·2^(attempt-1)`, cap `32s`,
  jitter `×rand(0.9..1.1)` — Claude Code's shape (`provider-api.md:64`) with
  codex's jitter form (`codex-client/src/retry.rs:38-47`). **`Retry-After`
  takes precedence and bypasses the exp cap** (`provider-api.md:64`), itself
  capped at `120s` (grok `client.rs:206-212`; note grok only parses integer
  delta-seconds, not HTTP-date — same simplification here, HTTP-date → `None` →
  fall back to exp backoff, `client.rs:76-80`).
- **`max_attempts ≈ 8`** (Claude Code default 10, codex 5; ~a few minutes of
  transient-error tolerance). 429 has its own tighter cap of 2.
- **429 is surfaced, not silently retried past the cap** — the deliberate choice
  of codex and grok so rate-limit info reaches the caller
  (`sampling-comparison.md:49`). Claude Code retries 429 only for non-subscribers;
  v0 is API-key, so we treat 429 as surface-after-2.

**401 refresh-once** (`Auth`): on the first 401/403 inside the retry loop, call
`auth_refresh.refresh()`; if it returns a *different* credential (grok's
`refresh_after_unauthorized` only fires on a changed token,
`03-grok-build/provider-api.md:60`), rebuild the client headers and retry **once**;
a second 401 → terminal `ProviderError::Auth`. Claude Code treats 403 "OAuth token
revoked" like 401 (`provider-api.md:72`). In v0 (static key) `refresh()` re-reads
`LOCODE_API_KEY`/config; with no OAuth wired it will usually return `None` → 401
is terminal immediately. The seam (`trait AuthRefresh`) is what a future OAuth flow
plugs into (grok's `xai-grok-auth` DI seam, `03-grok-build/provider-api.md:58-60`).

`ProviderError::retryable()` encodes the "yes" rows above; `run_with_retry`
consults it plus the 429/`Retry-After`/`x-should-retry` special cases.

### 4.7 Pre-send transcript repair / dedup

ADR-0004 makes pairing a **wire-format** invariant and prescribes "a single
function the provider layer calls unconditionally (before every send)". Grok runs
`repair_dangling_tool_calls` (`conversation.rs:2784`) + `dedup_duplicate_tool_results`
(`conversation.rs:2911`) both at every write boundary **and** before
`build_conversation_request` (`03-grok-build/provider-api.md:65-69`) — belt and
suspenders.

**Ownership decision:** the canonical repair lives in the **engine** (Task 6,
which synthesizes `is_error` results on abort/max-turns) and the pure functions
themselves belong in **`locode-protocol`** (they operate on `Vec<Message>` and are
provider-neutral). The Anthropic wire calls the *same* shared functions **as its
first step** on a clone of `req.messages`, defensively — a request must never leave
this crate with a dangling `tool_use` or duplicate `tool_result`, regardless of
who called it. Semantics to match grok:
- `repair_dangling_tool_calls`: for every `tool_use` id with no following
  `tool_result`, splice a synthetic `tool_result{is_error:true}` with
  model-actionable wording (grok's `DanglingToolCallReason`,
  `conversation.rs:2737-2751,2885`).
- `dedup_duplicate_tool_results`: keep the **last** result per `tool_use_id`
  (`03-grok-build/provider-api.md:67`).

**Open coordination point** with Task 6 (§8): confirm the functions land in
`locode-protocol` and both engine + wire call them, so we don't fork the logic.

### 4.8 Headers & send (`client.rs`)

Every request carries: `content-type: application/json`, `accept:
application/json`, `anthropic-version: <cfg.anthropic_version>` (Claude Code sends
`2023-06-01`, e.g. `betas.ts` callsites / `teleport/api.ts:280`), the auth header
(`x-api-key: <key>` for `Native`, `Authorization: Bearer <token>` for `Proxy`),
`anthropic-beta: <comma-joined cfg.betas>` when non-empty (latched per session so
toggling a feature mid-run doesn't bust the server cache, `provider-api.md:39`),
and `cfg.extra_headers`. POST `{base_url}/v1/messages`, `stream:false` (v0). One
send, JSON in / JSON out; retry is the wrapper's job.

---

## 5. Design decisions (each: harness `file:line` · why · why-not-alt · differences)

1. **Non-streaming first.** — *Source:* Claude Code falls back to non-streaming on
   stream failure (`01-claude-code/provider-api.md:58`); "Non-streaming is
   acceptable" (`05-comparative/sampling-comparison.md:87`). *Why:* v0 buffers each
   assistant turn fully before dispatch (SPEC.md:16, ADR-0005); zero SSE state
   machine to get wrong. *Why-not:* streaming needs indexed partial-JSON tool-arg
   assembly (`sampling-comparison.md:25-36`) — real complexity for no v0 benefit.
   *Difference:* for the others streaming is primary and non-streaming a fallback;
   we invert it. The `ToolCallAssembler` + `MessageStreamEvent` types are already
   in place for when streaming lands.

2. **Reuse grok's `messages.rs` wire structs, + `is_error` on `ToolResult`.** —
   *Source:* grok `messages.rs:114-119` (ToolResult **without** `is_error`),
   `messages.rs:209-226` (`StopReason::Unknown` catch-all). *Why:* they're already
   correct, forward-compatible, and battle-tested. *Why-not (hand-new structs):*
   pointless divergence. *Difference:* we **add** `is_error` because our protocol
   carries it (ADR-0013) and Anthropic honours it; grok omitted it.

3. **Verbatim tool-call ids; no sanitization.** — *Source:* grok sanitizes on both
   send sides (`conversation.rs:2986-2996,3112,3151`). *Why:* ADR-0007 demands
   verbatim ids; Anthropic ids are already `[A-Za-z0-9_-]`; sanitizing is a no-op
   that only risks pairing (ADR-0004). *Why-not:* grok needs it because ids also
   flow from its Chat/Responses backends — a harness difference, not a rule for a
   single-wire client.

4. **System hoist to top-level `system`.** — *Source:* grok
   `conversation.rs:3074-3082,3191-3206`; ADR-0013 mapping table + "An Anthropic
   `role:"system"` message is our `Developer`, not our `System`." *Why:* Anthropic
   `system` is a single top-level param; leading (and any) System blocks concatenate
   into it. *Why-not (inline system message):* Anthropic has no
   `role:"system"` in the message array except via the mid-conversation beta —
   which is our **Developer**, not System.

5. **Developer → portable `<system-reminder>` user block (default), beta path
   flagged.** — *Source:* ADR-0013 Developer row ("wire flag; default = portable
   fallback"); the `claude-code-system-surfaces` note (three system surfaces).
   *Why:* the beta path needs a wire `MessageRole::System` (grok's enum lacks it,
   `messages.rs:63-66`) **and** the `mid-conversation-system` beta header — avoid
   both in v0. *Why-not (always beta):* gates the whole wire on a beta.
   *Difference:* Claude Code uses the real beta message; we default to the portable
   form and keep the beta behind `DeveloperRendering`.

6. **`cache_control`: last system block + last message, ≤4 guard.** — *Source:*
   Claude Code (`01-claude-code/provider-api.md:34-37,80`, *"do not add … or you
   will get a 400"*); grok marks only last system (`conversation.rs:3191-3196`).
   *Why:* two markers is the cheapest correct Anthropic caching win; the ≤4 assert
   is the hard server cap. *Why-not (grok's system-only):* leaves the turn-tail
   uncached; Claude Code's single last-message marker is "tuned to KV-page
   eviction." *Difference:* we combine both schools (grok's system marker + Claude
   Code's message marker) and add an explicit counter assertion.

7. **Omit temperature when thinking on.** — *Source:*
   `01-claude-code/provider-api.md:30-31,43`; contrast grok
   `conversation.rs:3269` (unconditional forward). *Why:* API requires `temp=1`
   with thinking and 400s otherwise. *Why-not (forward temp like grok):* grok only
   gets away with it by leaving temp `None`; we make the omission deterministic.

8. **`reasoning_effort → budget_tokens` (default), effort/adaptive flagged.** —
   *Source:* grok effort-based (`conversation.rs:3232-3260`, `types.rs:812-820`) vs
   Claude Code budget-based (`provider-api.md:43`). *Why:* budget works without the
   `effort-2025-11-24` beta (`betas.ts:15`) and on 4.0–4.5 models; clamp to
   `max_tokens-1`. *Why-not (effort-only):* beta-gated + adaptive-model-only.
   *Difference:* we support both via `ReasoningEncoding`; grok is effort-only on
   Messages, old Claude Code is budget-only.

9. **Thinking + signature replay.** — *Source:* grok replays `signature` from
   `encrypted_content` (`conversation.rs:3169-3183`) and captures it on the stream
   (`stream/messages.rs:268-269,318-344`). *Why:* Anthropic 400s a thinking+tool_use
   turn if the signed thinking block isn't echoed verbatim; reasoning must stay
   contiguous with the following tool_use (`sampling-comparison.md:71`). *Why-not
   (drop thinking like grok's `From`, `conversation.rs:3311-3312`):* that path drops
   it only because it returns one item and recovers it on the stream; our
   `Completion.content: Vec<ContentBlock>` holds it directly.

10. **Two-tier retry; tier-1 here, tier-2 in engine.** — *Source:*
    `05-comparative/sampling-comparison.md:38-49`; grok
    `03-grok-build/provider-api.md:38-56`; codex `responses_retry.rs`,
    `codex-client/src/retry.rs`. *Why:* transport retries are wire-local; rebuild/
    compact needs loop state the provider doesn't own (ADR-0007). *Why-not (all
    retry in the wire):* couples the provider to compaction/history. *Difference:*
    single-provider, so no WebSocket→HTTP or tri-protocol fallback (codex/grok);
    no model-fallback (Claude Code's 529×3, `provider-api.md:67`).

11. **429 surfaced (cap 2), context/quota terminal, `x-should-retry` honored.** —
    *Source:* `sampling-comparison.md:49`; grok `client.rs:214-227`,
    `03-grok-build/provider-api.md:46,50`. *Why:* rate-limit + quota info must reach
    the caller; hammering is antisocial. *Why-not (retry 429 freely):* Claude Code
    only does so for non-subscribers.

12. **`Retry-After` integer-seconds, cap 120s, bypasses exp cap.** — *Source:*
    grok `client.rs:76-80,206-212`; Claude Code `provider-api.md:64`. *Why:* server
    knows better than our backoff; the 120s cap guards a misbehaving upstream.
    *Why-not (parse HTTP-date):* inference backends emit integer seconds only;
    HTTP-date → `None` → exp backoff.

13. **Per-model record + `LOCODE_*` env; auth in the record.** — *Source:* ADR-0007
    decision + consequence (line 13, 30); `05-comparative/sampling-comparison.md:59,62`;
    Claude Code key vs `AUTH_TOKEN` (`provider-api.md:57`). *Why:* base-URL override
    changes the auth header (native `x-api-key` → proxy `Bearer`), so auth can't be
    a global constant. *Why-not (global key):* breaks the moment you point at a
    proxy.

14. **Defensive repair/dedup before every send.** — *Source:* ADR-0004; grok
    `conversation.rs:2784,2911`, `03-grok-build/provider-api.md:65-69`. *Why:* a
    dangling/duplicate pair 400s the whole request; make it unconditional at the
    wire boundary. *Why-not (trust the loop):* ADR-0004 rejects scattering the
    check — the wire is the last line of defense.

---

## 6. Tests (fixtures only — no live key in CI)

**Request-shape (`anthropic_request_shape.rs`), asserting on the built
`wire::MessagesRequest` / its serialized JSON:**
- **Cache-marker count:** `CacheHint::Standard` over a request with system + a
  tail user turn → exactly **2** `cache_control` markers (last system block + last
  message); a synthetic 5-system-block case must still assert **≤4** and fail loudly
  above it. `CacheHint::Off` → **0** markers.
- **System hoist:** a stream `[System, User, Assistant, User]` → `system` populated
  from the System message, `messages` has no system-role entry.
- **Developer default:** a `Developer` message → a `role:"user"` message wrapped in
  `<system-reminder>…</system-reminder>` (no beta header); beta-flag variant → a
  `role:"system"` message + beta present.
- **Temp-omit:** `reasoning_effort=Some(Medium)` → request has `thinking` and
  **no** `temperature`; `reasoning_effort=None` with `temperature=Some(0.2)` → temp
  present, no thinking.
- **Effort mapping:** `Low/Medium/High` → expected `budget_tokens`
  (clamped to `max_tokens-1`); `Minimal` → no thinking.
- **tool_use id round-trip (send):** an Assistant `ToolUse{id:"toolu_x"}` and the
  paired User `ToolResult{tool_use_id:"toolu_x"}` serialize with **identical,
  verbatim** ids.
- **`is_error` on tool_result** serializes only when true.

**Parse (`anthropic_parse.rs`), from recorded response fixtures:**
- **id preservation:** a `tool_use` block with `id:"toolu_abc"` → `Completion`
  `ToolUse.id == "toolu_abc"` (no sanitization).
- **Thinking signature round-trip:** a `thinking` block `{thinking, signature}` →
  `ContentBlock::Thinking{ text, signature: Some(sig) }`; then feed that Completion
  back through `build_request` and assert the wire `thinking` block carries the
  **same** signature (proves replay).
- **usage mapping:** all four token fields map correctly (incl. `cache_read` /
  `cache_creation`).
- **stop-reason mapping:** each known value maps; an unknown string →
  `Unknown(raw)` (never panics); `model_context_window_exceeded` + empty content →
  `ContextOverflow`.

**Retry / classification (`anthropic_retry.rs`), no network — feed synthetic
status+body into `classify` and drive `run_with_retry` with a scripted op:**
- **5xx** and **transport** → `retryable()==true`, backed off and retried.
- **429** → `RateLimited{retry_after}`, `Retry-After` honored (bypasses exp cap),
  capped at 2 attempts then **surfaced**.
- **529 / `overloaded_error` body** → retryable even when status is masked.
- **400 context / 413** → `ContextOverflow` **terminal** (no retry).
- **quota** → `Quota` terminal; **401** → refresh-once (scripted `AuthRefresh`
  returning a new key) → one retry → success; a second 401 → terminal `Auth`.
- **`x-should-retry:false`** on an otherwise-retryable 5xx → terminal.
- **backoff/jitter:** deterministic bounds check (delay within
  `[base·2^(n-1)·0.9, …·1.1]`, capped at 32s), Retry-After overrides.

**Transcript hygiene:** a stream with a dangling `tool_use` → after the wire's
defensive pass, a synthetic `tool_result{is_error}` is present; duplicate
`tool_result`s collapse to the **last**.

Fixtures: hand-written minimal JSON bodies under `tests/fixtures/` (a real
tool-use response, a thinking+tool_use response, a 429 head, a 400 context-overflow
body, a 529/overloaded body). No recorded live traffic needed; shapes are stable
and documented.

---

## 7. Dependencies to add (`crates/locode-provider/Cargo.toml`)

All are ADR-0007/SPEC-sanctioned (SPEC Tech Stack: `reqwest` with `rustls`, `tokio`,
`thiserror`) — but adding a dep is an **"Ask first"** item (AGENTS.md Boundaries),
so this list is for approval, not a fait accompli.

| Crate | Features | Why | Precedent |
|---|---|---|---|
| `reqwest` | `rustls-tls`, `json`, `http2`; **`default-features=false`** | the one HTTP client; `rustls` avoids the system OpenSSL dep (SPEC:38, ADR-0007) | grok `xai-grok-sampler` owns a `reqwest::Client` (`client.rs`); codex `codex-http-client` wraps reqwest |
| `tokio` | `rt`, `macros`, `time` (`time` for `sleep` in backoff) | async runtime (SPEC:18); `time` for retry sleeps | codex `codex-client/src/retry.rs:6` uses `tokio::time::sleep`; grok uses tokio |
| `serde` | `derive` | wire struct (de)serialization | universal |
| `serde_json` | — | `input: Value`, body encode/decode | grok `messages.rs`, everywhere |
| `thiserror` | — | `ProviderError` variants (already the repo's error crate, SPEC:34, Task 4 added `thiserror` 2) | already in-tree |
| `async-trait` | — | `#[async_trait] impl Provider` (already added in Task 4) | already in-tree |
| `rand` | `default` (`rand::rng().random_range`) | backoff jitter | codex `codex-client/src/retry.rs:3,45` uses `rand::Rng` |
| `http` / `reqwest::header` | (via reqwest) | `HeaderName`/`HeaderValue`/`StatusCode`/`RETRY_AFTER` | grok `client.rs:19,206-212` |

Already present in the crate: `locode-protocol` (path dep). `MockProvider` (Task 5)
needs none of the above. **No `base64`/`url` needed** for v0 (images arrive
pre-encoded as `ImageSource::Base64{data}` / `Url`, ADR-0013). Reqwest's `json`
feature pulls `serde_json` transitively but we depend on it directly anyway.

---

## 8. Open questions

1. **`ConversationRequest` shape vs the todo.** The as-designed request I'm
   building against (this task's brief) is `ConversationRequest{ messages,
   tools, sampling_args, cache_hint }` — **no separate `system` field**; System
   lives in the message stream and the wire hoists it (matches ADR-0013 "no
   separate `system` field"). `tasks/todo.md:106` (Task 5) still lists
   `ConversationRequest{ system, messages, tools, sampling, cache_hint }`.
   **Confirm** Task 5 lands the field-less-`system` shape before Task 12 depends
   on it; if Task 5 keeps a `system` field, §4.1's hoist logic simplifies (read it
   straight) but ADR-0013's uniform stream is contradicted. *(Flagging an
   identifier/shape guess, per AGENTS.md.)*

2. **Where do `repair_dangling_tool_calls` / `dedup_duplicate_tool_results` live?**
   I propose the pure functions in `locode-protocol` (provider-neutral), called by
   **both** the engine (Task 6, canonical) and this wire (defensive). Needs
   sign-off so the logic isn't forked. (§4.7)

3. **`StopReason` variant set on `Completion.stop`.** The brief says
   `#[non_exhaustive]` with `Unknown(String)`, but the exact known variants are
   Task 5's. `map_stop_reason` (§4.5) assumes names mirroring the wire
   (`EndTurn/MaxTokens/ToolUse/StopSequence/Refusal/PauseTurn/…`). Confirm the Task
   5 enum so the mapping compiles.

4. **`Refusal` handling.** Anthropic can terminate with `stop_reason:"refusal"`
   (+ `stop_details`, grok `messages.rs:271-291`). Is a refusal a `Completion` with
   `stop=Refusal` and empty content (engine decides), or a terminal
   `ProviderError`? Default proposal: a normal `Completion{stop:Refusal}` — the
   engine (Task 6) maps it to a terminal report, not the wire. Confirm.

5. **Default model id + `max_tokens`.** What's the v0 default `model` string and
   cap? Claude Code caps at 8000 (`provider-api.md:29`). Proposal: config-required
   `model`, default `max_tokens` per SamplingArgs with an 8000-ish ceiling in
   `ModelConfig`. Needs the concrete model id the project targets.

6. **Beta headers actually needed in v0.** With `DeveloperRendering::SystemReminder`
   + `ReasoningEncoding::Budget` defaults, **no beta is required**. Confirm we ship
   v0 with an empty `betas` (prompt-caching is GA, not beta) — the `effort` /
   `mid-conversation-system` / `interleaved-thinking` betas (`betas.ts`) stay behind
   their opt-in flags.

7. **Client-side token estimate.** ADR-0007 wants a `~bytes/4` estimate with the
   authoritative count from response `usage`. Is that estimate the **engine's**
   (Task 6) or exposed as a helper here? Proposal: engine-owned; the wire only
   returns authoritative `Usage`. (grok keeps it in `xai-token-estimation`, a
   separate crate.)

---

## 9. Addendum — pre-implementation review decisions (2026-07-18)

Recorded from the review with the user before any Task-12 code. These close every
remaining §8 question and **supersede two defaults in §3.1/§4.8**: the empty-`betas`
default (§8 Q6) and the two-variant `ApiBackend`. ADR-0007 carries a dated
amendment for the ADR-level deltas (ADR-first).

### 9.1 Closed questions (confirmed with the user)

| Item | Decision |
|---|---|
| §8 Q4 refusal | A normal `Completion{stop: Refusal}`; the **engine** maps it to a terminal report. |
| §8 Q5 default model | **`claude-sonnet-5`** (native id). New env override **`LOCODE_MODEL`** joins `LOCODE_BASE_URL`/`LOCODE_API_KEY` (via OpenRouter the user sets `LOCODE_MODEL=anthropic/claude-sonnet-5`). `max_tokens` ceiling stays ~8000, config-overridable. |
| §8 Q6 betas | **SUPERSEDED** — see §9.3. v0 ships `interleaved-thinking-2025-05-14` **by default**. |
| §8 Q7 token estimate | Engine-owned; the wire returns authoritative `Usage` only. |
| `api_schema()` string | Plain **`"anthropic"`** (matches the documented `--api-schema` default; the schema names the wire format, and OpenRouter-vs-native is config, not a second schema). Supersedes §3.3's `"anthropic-messages"`. |
| Deps (§7) | Approved as listed (`reqwest` rustls/json/http2 no-default-features, `rand`, tokio `time`). |
| Live smoke test | **At end of Task 12** (not deferred to Task 14), manual, against **OpenRouter** (the user's real backend); never in CI. See §9.4. |

### 9.2 `ApiBackend::OpenRouter` — a first-class third variant

The user's primary backend is OpenRouter's Anthropic-compatible Messages endpoint
(`https://openrouter.ai/api/v1/messages`). Two OpenRouter-specific quirks make the
generic `Proxy` variant insufficient; both are implemented, with rationale comments,
in the user's `~/dev/cc-reverse-proxy` (the reference implementation):

1. **Beta-header mirroring.** OpenRouter reads Anthropic beta features from
   **`x-anthropic-beta`**, not the native `anthropic-beta`
   (`cc-reverse-proxy/reverse_proxy.go:601-607`, citing
   `openrouter.ai/docs/guides/routing/provider-selection#anthropic-beta-features`;
   verified against the live doc 2026-07-18). We emit the beta list on **both**
   header names for OpenRouter (mirroring, like the proxy) — harmless and robust.
2. **Provider-preferences injection.** For messages requests, inject a top-level
   `provider` body field unless the request already carries one
   (`reverse_proxy.go:562-570`):
   ```json
   {"ignore": ["amazon-bedrock"], "allow_fallbacks": false, "require_parameters": true}
   ```
   `require_parameters: true` is load-bearing: without it OpenRouter may route to a
   backend that **silently drops unsupported params** — exactly the failure mode
   that would eat `cache_control` or `thinking`. Config-overridable
   (`provider_prefs: Option<serde_json::Value>`; `None` → this default trio).

**Decision:** `ApiBackend { Native, OpenRouter, Proxy }`. `OpenRouter` is
**auto-detected** when the `base_url` host is `openrouter.ai` (pinnable explicitly);
any other non-Anthropic base URL resolves to `Proxy` as before. `OpenRouter`
selects: `Authorization: Bearer` auth, beta mirroring (1), prefs injection (2).
*Why a vendor variant instead of generic knobs (`beta_header_name` + `extra_body`)*:
the user's daily path should be two env vars, not hand-written body JSON; precedent
is grok's `ApiBackend` enum of known backends. The generic escape hatch remains
`Proxy` + `extra_headers`.

### 9.3 Betas: default = interleaved thinking (supersedes §8 Q6 and §3.1's empty default)

**Decision (user, 2026-07-18): interleaved thinking is required, not optional.**
`ModelConfig.betas` defaults to `["interleaved-thinking-2025-05-14"]`. Claude Code
enables this beta by default for every non-3.x model on first-party
(`betas.ts:257-262`, sole kill-switch `DISABLE_INTERLEAVED_THINKING`), and OpenRouter
documents it explicitly — it is proxy-safe.

**Wire implications (all already covered by the §4 design, now load-bearing):**
- An assistant turn may carry **multiple `thinking` blocks interleaved with
  `tool_use`** blocks. §4.2's replay rule applies to *every* thinking block, in
  order, signatures verbatim — `Completion.content: Vec<ContentBlock>` preserves
  ordering by construction. The §6 signature round-trip test gains an interleaved
  fixture (thinking → tool_use → thinking → tool_use in one turn).
- **Budget clamp relaxed:** with interleaved thinking, the API allows
  `budget_tokens > max_tokens` (the budget spans the whole turn). The §4.4 clamp
  `min(X, max_tokens-1)` applies **only when the beta is absent**; with the default
  beta set, effort budgets (4096/8192/16384) pass unclamped.
- Temp-omit (§4.3) unchanged: any thinking config still drops `temperature`.
- Via OpenRouter the beta list is mirrored to `x-anthropic-beta` (§9.2).
- The beta header is sent regardless of whether `reasoning_effort` enables a
  thinking config (harmless no-op without one — Claude Code does the same).

**Beta survey** (from Claude Code `src/constants/betas.ts` + `src/utils/betas.ts`,
read 2026-07-18). Claude Code's own rule — experimental betas are gated on
`shouldIncludeFirstPartyOnlyBetas()` because proxies may reject them
(`betas.ts:210-220`) — becomes our rule: **the default set must be proxy-safe;
everything else is opt-in config.**

| Beta | Verdict for locode v0 | Why |
|---|---|---|
| `interleaved-thinking-2025-05-14` | **default on** | user requirement; CC default (`betas.ts:257-262`); OpenRouter-documented |
| `effort-2025-11-24` | opt-in (existing `ReasoningEncoding::EffortAdaptive` flag) | already designed in §4.4; adaptive-model-only |
| `context-1m-2025-08-07` | opt-in (config `betas`) | CC gates per-model (`has1mContext`); premium pricing; useful for long runs later |
| mid-conversation-system | opt-in (existing `DeveloperRendering` flag) | already designed in §4.1 |
| `structured-outputs-2025-12-15` | deferred with `--json-schema` | model-gated (`modelSupportsStructuredOutputs`); note the OpenRouter doc still cites the older `-2025-11-13` id — pin whichever the target accepts when this lands |
| `context-management-2025-06-27` | skip | 1P-only per CC's proxy gate; server-side compaction is deferred anyway |
| `prompt-caching-scope-2026-01-05` | skip | 1P-only; no-op without a scope field |
| `fast-mode-2026-02-01` | skip (possible later seam) | `speed:"fast"` param, Opus-only |
| `claude-code-20250219`, `cli-internal`, `token-efficient-tools`, `redact-thinking`, `afk-mode`, advisor/task-budgets | skip | CC product-identity / ant-internal experiments — not our traffic |
| web-search / tool-search betas | skip | server-side tools, out of v0 scope |

Betas stay **latched per session** (§4.8) — the set is fixed at `ModelConfig`
construction, never toggled mid-run.

### 9.4 Live smoke test (end of Task 12, manual, OpenRouter)

One multi-turn run with the user's OpenRouter key proving, on the real backend:
1. **Interleaved-thinking replay:** a turn with thinking interleaved between two
   `tool_use` blocks round-trips (signatures echoed verbatim; no 400 on turn 2).
2. **Caching survives routing:** `cache_read_input_tokens` /
   `cache_creation_input_tokens` come back non-zero on the second request (proof
   `require_parameters` + `cache_control` worked end-to-end).
3. **Error classification sanity:** capture one OpenRouter error body shape (e.g.
   an invalid-model 4xx) and check `classify` degrades sensibly.
Optionally routed through `cc-reverse-proxy` (`--target https://openrouter.ai/api`)
to capture the exact wire traffic for the fixtures directory.

### 9.5 Live-smoke findings (2026-07-18, recorded after implementation)

The §9.4 smoke ran against OpenRouter with the user's key; all three invariants
passed (thinking + signature + `redacted_thinking` replayed across turns with no
400; `cache_read_input_tokens` = the full turn-1 write; a real OpenRouter error
body classified terminal). Findings folded back into the code/docs:

1. **`redacted_thinking` is real and load-bearing.** A live response carried a
   `redacted_thinking` block; without a wire variant the whole response fails to
   deserialize. Added end-to-end (`ContentBlock::RedactedThinking { data }` in
   `locode-protocol` — ADR-0013 amendment — plus wire/parse/build replay).
   grok's `messages.rs` lacks the variant; noted as our third delta.
2. **Vertex stays allowed (user decision).** The default `provider` prefs remain
   cc-reverse-proxy's trio (`ignore: ["amazon-bedrock"]`, no `only` pin) —
   Vertex is a production-relevant Anthropic provider. Probes showed Vertex
   honours the thinking config **only when the interleaved-thinking beta header
   reaches it** (silently drops thinking otherwise — `require_parameters` does
   not catch header-gated behavior), which our always-on beta mirroring covers.
3. **Cross-provider routing forfeits cache reads.** Turn 2 routed to a different
   provider re-*writes* the prompt cache instead of reading it. Where cache-hit
   determinism matters (e.g. the smoke itself), set
   `provider_prefs = {"only": ["anthropic"], …}` per-config; the default keeps
   failover.
4. **Thinking emission is model-discretionary under the interleaved beta** —
   trivial prompts may yield zero thinking blocks; the smoke uses a
   comparison task that reliably provokes one. Not a wire defect.
