# Task 18 — `locode-provider` OpenAI Responses wire (`api_schema = "openai-responses"`)

> Implementation plan, written **before** code. The third `Provider` and the first
> OpenAI-family wire, prioritized over Chat Completions (user decision, 2026-07-18):
> it is the wire codex actually speaks (codex is Responses-only), the only wire that
> can deliver the freeform/grammar `apply_patch` tool, and grok build's own backend
> for xAI models (encrypted-reasoning replay, ZDR). **Stateless always**
> (`store:false`, never `previous_response_id`).
>
> Cites harness source under `~/dev/coding-cli-survey/submodules/{codex,grok-build}`
> as `file:line` (codex pinned at `1d94125`, grok-build at `b189869b`), the official
> OpenAI OpenAPI spec (`openai/openai-openapi`, pulled 2026-07-16; the platform
> reference pages are generated from it), OpenRouter docs, and **live probes run
> against OpenRouter on 2026-07-18/19** (§0). Repo ADRs: ADR-0007 (provider = wire
> schema, gateway = config), ADR-0013 (4-role protocol + OpenAI mapping table),
> ADR-0004 (pairing), ADR-0003 (ToolSpec — this task amends it, §8).
>
> Also in scope here (consumed by Task 17): **hoisting the shared transport
> machinery** (`RetryPolicy`/`run_with_retry`/`backoff`/`HttpFailure`/
> `parse_retry_after`/`normalize_input_schema`) out of the `anthropic` module into a
> shared `locode-provider` layer (§2, §4.8).

---

## 0. Probe log (live, via OpenRouter — key from env only, never echoed)

Established facts from the 2026-07-19 probe session (recorded in STATUS.md), plus
two probes run while writing this plan:

| # | Probe | Finding |
|---|---|---|
| P1 | `POST /api/v1/responses` with `store:true` / `previous_response_id` | **400 — OpenRouter's Responses beta is stateless-only** (also documented: "Requests that set `store: true` or a non-null `previous_response_id` are rejected with a 400"). |
| P2 | custom tool + Lark grammar, `openai/gpt-5-mini` | Works end-to-end: `{"type":"custom","name":…,"format":{"type":"grammar","syntax":"lark","definition":…}}` → a proper `custom_tool_call` output item with grammar-constrained raw-text `input`. |
| P3 | `x-ai/grok-4.5` through `/v1/responses` | Reasoning items **always** carry `encrypted_content`; JSON function tools work (`function_call` with `call_id` `"call-…"`); **`"type":"custom"` tools are 422-rejected by xAI** (their tool enum: function, web_search, x_search, …) → freeform tools are OpenAI-models-only. |
| P4 (new, 2026-07-19) | `instructions` + `store:false` on `openai/gpt-5-mini`, **no `include` param** | `instructions` honored (model obeyed it verbatim). Reasoning item came back **with `encrypted_content` populated even without `include`** (matches the 2026 spec note that encrypted content is now returned by default; we still send `include` for compat, §4.1). Reasoning item carries an extra **`format:"openai-responses-v1"`** field — one more reason replay must be **whole-item-opaque** (§4.4). |
| P5 (new, 2026-07-19) | `x-ai/grok-4.5` with `instructions` + `provider:{allow_fallbacks:false,…}` | `instructions` honored by xAI too; the **`provider` preferences object is accepted on `/v1/responses`** (request routed and completed). xAI reasoning items carry a **populated `summary`** (without requesting one), `encrypted_content`, `format:"xai-responses-v1"`. Usage: `input_tokens_details.cached_tokens`, `output_tokens_details.reasoning_tokens` present; OpenRouter appends `cost`/`cost_details`/`is_byok` extras (ignore-unknown-fields required). |

---

## 1. Purpose & scope

Implement `OpenAiResponsesProvider`: `api_schema() = "openai-responses"`,
`POST {base_url}/v1/responses`, **non-streaming**, always-Bearer auth. It converts
`ConversationRequest` into a Responses request (System→`instructions` hoist,
Developer→`developer` message, tool_result→`function_call_output` explosion,
reasoning-item replay, function **and freeform/custom** tools), sends via the
shared transport layer, and parses the terminal `Response` object back into a
`Completion`.

**In scope (v0):**
- The **`ToolSpec` freeform extension** in `locode-protocol` + `locode-tools`
  (the protocol change that unblocks codex `apply_patch`) — proposed ADR deltas
  in §8; **ask-first item**.
- Request build: input-item mapping per ADR-0013, `store:false` always,
  `include:["reasoning.encrypted_content"]`, reasoning-effort mapping, verbatim
  `call_id` round-trip, whole-item reasoning replay.
- Response parse: output-array iteration (`reasoning` / `message` /
  `function_call` / `custom_tool_call`), usage mapping (incl. `cached_tokens` +
  `cache_write_tokens`), `status`/`incomplete_details` → stop mapping,
  `response.error` classification.
- **Hoist the shared transport layer** out of `anthropic/` (retry loop, backoff,
  `HttpFailure`, `Retry-After` parsing, schema normalization); OpenAI-family
  error classification (429-`insufficient_quota` = Quota!).
- `OpenAiModelConfig` record + the same `LOCODE_BASE_URL`/`LOCODE_API_KEY`/
  `LOCODE_MODEL` env story; backend detection (Native/OpenRouter/Proxy);
  OpenRouter `provider`-prefs injection (probe P5).
- Fixture + canned-`TcpListener` tests; `--api-schema openai-responses` in
  `locode-exec`; a manual live smoke (§6.4).

**Deferred (reserved seams, not v0):**
- **Streaming** — the SSE event-name surface is reserved (§4.9); codex is
  streaming-only (`stream: true` hardcoded, `core/src/client.rs:899`) but grok
  proves the non-streaming path (`create_response` deserializes a whole
  `rs::Response`, grok `client.rs:1196-1205`) and probes P2/P4/P5 exercised it
  live. We invert codex's default exactly as Task 12 inverted Claude Code's.
- **WebSocket transport** + `previous_response_id` incremental input — codex's
  second transport (`codex-api/src/common.rs:266-293`); HTTP resends full
  history, which is also the only OpenRouter-compatible mode (P1).
- **Azure** (`store:true` is Azure-only in codex, `client.rs:898`;
  `is_azure_responses_provider`, `codex-api/src/provider.rs:88-127`) — our
  stateless rule is unconditional in v0; an Azure backend variant would revisit.
- `text` controls (verbosity / `json_schema` output) — deferred with
  `--json-schema` (SPEC Open Q3); the wire struct keeps the field.
- `service_tier`, `client_metadata`, responses-lite (`client.rs:842-863`),
  hosted tools (`web_search` etc.), `parallel_tool_calls` tuning, per-turn
  `reasoning.context` control.

---

## 2. Module layout

```
crates/locode-provider/src/
├── lib.rs                    # + pub mod http, openai; re-exports
├── http.rs                   # NEW shared transport layer (hoisted from anthropic/):
│                             #   RetryPolicy, backoff, run_with_retry (made generic),
│                             #   HttpFailure, parse_retry_after, build_http_client,
│                             #   normalize_input_schema
├── anthropic/                # unchanged behavior; retry.rs/error.rs shrink to
│                             #   re-exports + the anthropic-specific classify()
└── openai/
    ├── mod.rs                # shared OpenAI-family surface: OpenAiModelConfig,
    │                         #   ApiBackend detection, error-body DTOs, classify()
    ├── common.rs             # error body shapes (OpenAI + OpenRouter), auth headers
    └── responses/
        ├── mod.rs            # OpenAiResponsesProvider + impl Provider
        ├── wire.rs           # serde DTOs: request, input items, tools, Response,
        │                     #   output items, usage (hand-rolled; §5.2)
        ├── build.rs          # build_request(&ConversationRequest, &config) -> wire
        └── parse.rs          # Response -> Completion; stop mapping
crates/locode-provider/tests/
├── responses_request_shape.rs
├── responses_parse.rs
├── openai_classify.rs        # shared with Task 17
├── responses_provider.rs     # canned TcpListener end-to-end
├── responses_live_smoke.rs   # #[ignore]d manual (OpenRouter)
└── fixtures/…
```

Rationale: mirrors the proven `anthropic/` split (wire DTOs vs conversion vs
transport). The **`openai/` parent module** exists because Task 17 (chat) shares
the config record, backend detection, auth, and error classification — hoisting
those *now* is cheaper than a second extraction later; precedent is grok build,
where one sampler + one config serve all three backends
(`survey/03-grok-build/provider-api.md`). The **`http.rs`** hoist is this task's
explicit acceptance criterion (`tasks/todo.md` Task 18).

What moves to `http.rs` verbatim vs what stays wire-specific:

| Piece | Destination | Note |
|---|---|---|
| `RetryPolicy`, `backoff`, `run_with_retry` | `http.rs` | `run_with_retry` becomes generic over the success type: `async fn run_with_retry<T, F, Fut>(policy, op) -> Result<T, ProviderError>` — it only inspects `HttpFailure`, never the `Completion` (`anthropic/retry.rs:69-101`) |
| `HttpFailure { error, force_terminal, retry_after }` | `http.rs` | `force_terminal` stays: Anthropic sets it from `x-should-retry`; OpenAI wires always pass `false` (no such header exists there — checked OpenAI + OpenRouter header docs) |
| `parse_retry_after` (integer seconds only) | `http.rs` | OpenAI sends `retry-after` on 429s (rate-limit guide); OpenRouter sends it "when providers supply hints" (limits doc). The `x-ratelimit-reset-*` duration strings ("1s", "6m0s") are **not** parsed — nonstandard format, `retry-after` + backoff suffice |
| `build_http_client` (30s connect / 10min total) | `http.rs` | same budget: non-streaming reasoning turns run minutes |
| `normalize_input_schema` ($schema strip) | `http.rs` | its own doc comment already says "hoist out of `anthropic` when a second wire lands" (`anthropic/build.rs:99`) |
| `classify` + `ErrorBody` | **per-wire** | the error-body shapes and terminal heuristics genuinely differ (§4.7); sharing the *skeleton* would couple wording sniffs across vendors |

---

## 3. Key types & signatures

### 3.1 The `ToolSpec` freeform extension (protocol + tools — ask-first)

Today (`locode-protocol/src/lib.rs:246-253`): `ToolSpec { name, description,
parameters: Value }`. Codex's `apply_patch` is a **custom tool**: no JSON schema,
instead `{type:"custom", name, description, format:{type:"grammar",
syntax:"lark", definition}}` (codex `tools/src/tool_spec.rs:49-50`,
`responses_api.rs:11-23`; OpenAI spec `CustomToolParam` — required `type, name`;
grammar format required `type, syntax, definition`). Proposed change:

```rust
// locode-protocol
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input: ToolInputFormat,        // REPLACES `parameters: Value`
}

#[derive(…, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolInputFormat {
    /// A JSON-schema function tool (every tool today).
    JsonSchema {
        /// The derived JSON Schema for the tool's arguments.
        parameters: Value,
    },
    /// A freeform tool: raw text input constrained by a server-side grammar
    /// (OpenAI Responses `custom` tools — codex `apply_patch`).
    Freeform {
        syntax: GrammarSyntax,          // Lark | Regex
        definition: String,             // the grammar source, verbatim
    },
}
```

An enum field (not two optional fields, not a two-variant `ToolSpec` enum)
because: (a) a spec is *exactly one* of the two — optional-field encodings admit
invalid states; (b) keeping `ToolSpec` a struct preserves every `spec.name` /
`spec.description` call site (only `parameters` accesses change, all in-tree:
`anthropic/build.rs:58-66`, `Registry::specs`, `Event::Init` serialization);
(c) codex models it the same way — `ToolSpec::{Function(…), Freeform(…)}` is a
tagged enum whose variants carry format-specific payloads
(`tools/src/tool_spec.rs:15-51`).

`locode-tools` grows one **defaulted** method on both `Tool` and `DynTool` (no
existing tool changes):

```rust
fn input_format(&self) -> ToolInputFormat {
    ToolInputFormat::JsonSchema { parameters: self.parameters_schema() }
}
```

`Registry::specs()` calls `input_format()` instead of `parameters_schema()`.
A freeform tool (Task 19's `apply_patch`) overrides it with
`Freeform { syntax: Lark, definition: GRAMMAR }` — and **still implements the
typed `Tool` trait** with `Args` able to decode the raw text (§4.6), so the one
dispatch door is untouched (ADR-0008).

### 3.2 Config (`openai/mod.rs`)

```rust
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";

/// Endpoint family — auth is ALWAYS Bearer for the OpenAI family; the variants
/// select quirks only (mirrors anthropic::ApiBackend, minus the auth split).
pub enum OpenAiBackend { Native, OpenRouter, Proxy }
// detect(): host openrouter.ai → OpenRouter; api.openai.com → Native; else Proxy.

pub struct OpenAiModelConfig {
    pub model: String,                    // "gpt-5.x…" native; "openai/…"/"x-ai/…" via OpenRouter
    pub base_url: String,                 // request path appends "/v1/responses"
    pub backend: OpenAiBackend,
    pub bearer: String,                   // always Authorization: Bearer
    pub max_tokens_cap: u32,              // clamp for max_output_tokens (floor 16 — spec min)
    pub reasoning_summary: Option<String>,// None (default) | "auto"|"concise"|"detailed" (§4.3)
    pub prompt_cache_key: Option<String>, // None default (grok's behavior); facade may set session id (§4.5)
    pub custom_tools_supported: bool,     // false → freeform specs degrade to JSON fallback (§4.6; xAI 422s custom, probe P3)
    pub system_placement: SystemPlacement,// Instructions (default) | InputMessage (§4.2)
    pub extra_headers: Vec<(String, String)>,
    pub provider_prefs: Option<serde_json::Value>, // OpenRouter routing prefs (probe P5); default trio on OpenRouter
}

pub enum SystemPlacement { Instructions, InputMessage }

impl OpenAiModelConfig {
    pub fn new(model, base_url, key) -> Self;   // trims trailing '/', detects backend
    pub fn from_env() -> Result<Self, ProviderError>; // LOCODE_API_KEY (required),
        // LOCODE_BASE_URL (default native), LOCODE_MODEL (no default — REQUIRED for
        // this wire: there is no obvious "default OpenAI model" analog of
        // claude-sonnet-5; error if unset. Open question Q1.)
    pub fn effective_provider_prefs(&self) -> Option<Value>; // OpenRouter → configured
        // or the default trio {ignore:["amazon-bedrock" is Anthropic-specific — here:
        // no ignore], allow_fallbacks:false, require_parameters:true} — see §4.5
}
```

### 3.3 Wire DTOs (`responses/wire.rs`) — hand-rolled, minimal

Grok reuses `async_openai::types::responses` wholesale
(`xai-grok-sampling-types/src/lib.rs:28`); codex hand-rolls a lean struct set
(`codex-api/src/common.rs:215-239`, `protocol/src/models.rs:802-1033`). **We
hand-roll like codex** (§5.2). Sketch:

```rust
#[derive(Serialize)]
pub struct ResponsesRequest {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub input: Vec<InputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,          // v0: None (server default "auto")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningParam>,    // { effort, summary? }
    pub store: bool,                          // ALWAYS false
    pub stream: bool,                         // ALWAYS false (v0)
    pub include: Vec<String>,                 // ["reasoning.encrypted_content"]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<serde_json::Value>,  // OpenRouter prefs (probe P5); not OpenAI's
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputItem {
    Message { role: String, content: Vec<ContentPart> },          // system|developer|user|assistant
    FunctionCall { call_id: String, name: String, arguments: String },
    FunctionCallOutput { call_id: String, output: String },
    CustomToolCall { call_id: String, name: String, input: String },
    CustomToolCallOutput { call_id: String, output: String },
    /// Replayed VERBATIM as raw JSON — see §4.4. Never constructed field-by-field.
    Reasoning(serde_json::Value),
}
// ContentPart: input_text / input_image / output_text (codex models.rs:712-727)

#[derive(Serialize)]
#[serde(untagged)]
pub enum ToolDef {
    /// FLAT function shape — Responses is NOT nested like Chat Completions
    /// (OpenAI spec FunctionToolParam; codex responses_api.rs:25-38).
    Function { r#type: MustBe!("function"), name, description, parameters, strict: bool },
    Custom   { r#type: MustBe!("custom"), name, description,
               format: GrammarFormat /* {type:"grammar", syntax, definition} */ },
}

#[derive(Deserialize)]
pub struct ResponsesResponse {
    pub status: Option<String>,               // completed|incomplete|failed|…
    pub output: Vec<OutputItem>,
    pub incomplete_details: Option<IncompleteDetails>, // { reason }
    pub error: Option<ResponseErrorBody>,     // { code, message }
    pub usage: Option<ResponsesUsage>,        // Option — OpenRouter/non-OpenAI variance
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputItem {
    Message { content: Vec<OutputContent> },  // output_text{text} | refusal{refusal}
    FunctionCall { call_id: String, name: String, arguments: String },
    CustomToolCall { call_id: String, name: String, input: String },
    Reasoning(serde_json::Value),             // captured whole, opaquely (§4.4)
    #[serde(other)] Other,                    // hosted-tool items etc. — never fail parse
}

#[derive(Deserialize)]
pub struct ResponsesUsage {
    pub input_tokens: Option<u64>,
    pub input_tokens_details: Option<InputTokensDetails>,  // { cached_tokens, cache_write_tokens }
    pub output_tokens: Option<u64>,
    pub output_tokens_details: Option<OutputTokensDetails>,// { reasoning_tokens }
}
```

All response-side structs tolerate unknown fields (serde default) — probe P5
showed OpenRouter appends `cost`/`cost_details`/`is_byok`; the 2026 spec added
fields freely. `Option<u64>` usage counters repeat the Task-12 lesson (OpenRouter
returned null usage for some models on the Messages endpoint).

Note on `Reasoning(serde_json::Value)`: serde's internally-tagged enums cannot
hold an arbitrary-`Value` newtype variant directly; implement with a manual
`Deserialize` on `OutputItem`/`InputItem` (peek `type`, keep the raw map for
`reasoning`) or an intermediate `#[serde(flatten)] extra: Map` struct. This is a
known implementation detail, not a design risk — the acceptance test is
byte-preserving round-trip of a reasoning item with fields we've never heard of
(probe P4's `format` field is the concrete example).

### 3.4 The provider (`responses/mod.rs`)

```rust
pub struct OpenAiResponsesProvider {
    http: reqwest::Client,
    config: OpenAiModelConfig,
    retry: RetryPolicy,          // shared http::RetryPolicy
}

#[async_trait]
impl Provider for OpenAiResponsesProvider {
    fn api_schema(&self) -> &str { "openai-responses" }
    async fn complete(&self, req: &ConversationRequest) -> Result<Completion, ProviderError> {
        // 1. repair_pairing on a clone (same defensive pass as anthropic/mod.rs:147-149)
        // 2. build_request(&repaired, &self.config)
        // 3. http::run_with_retry(…, || send_once(…))
        // 4. (parse happens inside send_once, like the anthropic client)
    }
}
```

No `AuthRefresh` seam here in v0: static Bearer only (OpenAI/OpenRouter API keys
don't rotate mid-run; codex's ChatGPT-JWT refresh is an auth mode we don't
carry). 401/403 → terminal `Auth` immediately.

---

## 4. Behavior / algorithms

### 4.1 Statelessness + include — the non-negotiables

- **`store: false` on every request.** Grok forces it as a sampler default with
  the comment "default is true, but that breaks ZDR compliance" (grok
  `client.rs:1090-1093`); codex sends `store: provider.is_azure_responses_endpoint()`
  — i.e. false for OpenAI (`core/src/client.rs:898`); OpenRouter **rejects**
  `store:true` with a 400 (probe P1, OpenRouter Responses doc). The OpenAI
  default is `true` (spec `CreateResponse`), so this must be explicit.
- **`previous_response_id`: never serialized** (field doesn't exist on our
  request struct). Full history is resent each turn — codex's HTTP transport
  does exactly this (incremental input is WebSocket-only,
  `codex-api/src/common.rs:266-293`); grok never sets it
  (`conversation.rs:2146`).
- **`include: ["reasoning.encrypted_content"]` always** — codex hardcodes
  exactly this one element (`client.rs:871`); grok defaults it in
  (`client.rs:1095-1099`). The 2026 spec says encrypted content is now returned
  by default (probe P4 confirms: present with no `include` sent), and the flag
  is "kept for compatibility" — we send it anyway: zero cost, guards older
  gateways.

### 4.2 Input mapping — ADR-0013 roles → Responses items

Processed in stream order (order is load-bearing for the prefix KV-cache —
grok's "byte-stable input ordering" comments, `conversation.rs:2174-2176`):

- **`Role::System`** → default (`SystemPlacement::Instructions`): concatenate
  all System text (in order, `\n\n`-joined) into the top-level **`instructions`**
  string; System messages produce no input item. This is codex's shape — base
  prompt in `instructions`, everything else in `input`
  (`client.rs:862`, `ResponsesApiRequest.instructions`,
  `codex-api/src/common.rs:217-218`) — and the codex pack (Task 19) *requires*
  it for faithfulness. Probes P4/P5 confirm `instructions` works through
  OpenRouter for both OpenAI and xAI models. **`SystemPlacement::InputMessage`**
  (opt-in): emit a `role:"system"` message item instead — grok's shape
  (`instructions: None`, System → `EasyMessage{role: System}`,
  grok `conversation.rs:2140,2223-2230`); the escape hatch if some gateway
  mishandles `instructions`.
- **`Role::Developer`** → a `message` item with **`role:"developer"`** — the
  exact semantic match (ADR-0013 mapping table; OpenAI spec `EasyInputMessage`
  roles `user|assistant|system|developer`). Note grok never emits `developer`
  (it has no such role in its neutral model); we do, because our protocol has
  the role precisely for this (ADR-0013 "we borrow OpenAI's word").
- **`Role::User`** → walk blocks **in order**, splitting:
  - contiguous `Text`/`Image` runs → one `message{role:"user"}` item with
    `input_text`/`input_image` parts;
  - each `ToolResult{tool_use_id, content, is_error}` → a
    **`function_call_output { call_id: tool_use_id, output }`** item (or
    `custom_tool_call_output` when the paired call was freeform — §4.6), placed
    at its block position. `output` = concatenated text chunks; images inside
    tool results are deferred (spec allows a content list — reserved).
    **`is_error` has no wire slot** — the OpenAI convention is that the output
    text itself carries the error message; we prepend nothing (the engine's
    tool_result text already reads as an error message — grok/codex do the
    same: their neutral outputs are plain strings,
    grok `conversation.rs:2280-2309`, codex `FunctionCallOutputPayload`).
- **`Role::Assistant`** → walk blocks in order:
  - `Text` → `message{role:"assistant", content:[output_text{text}]}` (codex
    `ContentItem::OutputText`, `models.rs:712-727`; grok `2256-2262`);
  - `ToolUse{id, name, input}` → **`function_call { call_id: id, name,
    arguments: serialize(input) }`** with **no `id` field** — spec: only
    `type, call_id, name, arguments` are required on replay; codex clears item
    ids when `store=false` (`prepare_response_items_for_request`,
    `client.rs:910-924`); grok sends `id: None` (`conversation.rs:2269-2273`).
    `call_id` **verbatim** (ADR-0007; grok passes Responses ids through
    unsanitized — sanitization is Messages-only, `conversation.rs:2986-2996`).
    If `input` is `Value::String` and the named tool is freeform →
    `custom_tool_call { call_id, name, input: <the string> }` (§4.6).
  - `Thinking`/`RedactedThinking` → reasoning replay, §4.4.

Multiple `ToolUse` blocks in one assistant message → multiple sibling
`function_call` items (parallel calls are N sibling items on this wire, not an
array — codex/grok both emit one item per call).

### 4.3 Sampling & reasoning params

- `max_output_tokens` = `sampling_args.max_tokens.min(cap).max(16)` (spec
  minimum 16; it includes reasoning tokens).
- `temperature`/`top_p` pass through as given (no thinking-omit rule here —
  that quirk is Anthropic's; reasoning models may *ignore* sampling params, not
  400 on them per current docs — noted, not enforced).
- `reasoning_effort` mapping (grok's `to_responses_api` precedent —
  `conversation.rs:2150-2153`; codex `build_reasoning`, `client.rs:803-821`):

  | `ReasoningEffort` | wire |
  |---|---|
  | `None` (absent) | omit `reasoning` entirely (server default applies — `medium` on reasoning models) |
  | `Minimal` | `{"effort":"minimal"}` |
  | `Low` / `Medium` / `High` | `{"effort":"low"/"medium"/"high"}` |

  Note the asymmetry with the Anthropic wire (where `Minimal` = thinking off,
  grok `types.rs:812-820`): on Responses, reasoning models cannot be "off", and
  `"minimal"` is a real server-side level (2026 enum
  `none|minimal|low|medium|high|xhigh|max`). Mapping is per-wire by design
  (ADR-0007, `SamplingArgs` docs).
- `reasoning.summary`: **omitted by default** (`config.reasoning_summary =
  None`). Grok always requests `Concise` (`conversation.rs:2150-2153`); codex
  requests one only when the model supports the parameter (`client.rs:806-818`).
  We are headless/non-streaming — summaries cost output tokens and some
  providers reject the param on unsupported models; xAI populates summaries
  regardless (probe P5). Config knob; open question Q2.

### 4.4 Reasoning-item replay — the Responses analog of thinking signatures

**What the harnesses store and replay.** Grok stores the **whole native item**:
`ConversationItem::Reasoning(rs::ReasoningItem)` (`conversation.rs:57`, rationale
comment `:45-56`) and replays it with only the output-only `status` field
stripped ("everything else — summary, content, encrypted_content, id — passes
through", `conversation.rs:2239-2247`). Codex likewise keeps
`ResponseItem::Reasoning { id, summary, content, encrypted_content }` in history
and resends it (`models.rs:840-852`; ids conditionally cleared,
`client.rs:917-923`). The spec makes `id` + `summary` + `type` **required** on a
replayed reasoning item — so replay must be **lossless**, and probe P4/P5 show
gateways add fields we don't model (`format`). Whole-item-opaque is the only
robust representation.

**Protocol representation (decision to confirm — ask-first, ADR-0013-adjacent).**
Reuse `ContentBlock::Thinking { text, signature }` with a documented per-wire
encoding — **no new protocol variant**:

- **parse:** each `reasoning` output item → `Thinking {
    text: <concatenation of summary[].text, "\n\n"-joined — may be empty>,
    signature: Some(<the COMPLETE reasoning item as a compact JSON string>) }`.
- **build (replay):** for each `Thinking` block whose `signature` parses as a
  JSON object with `"type":"reasoning"` → emit that object **verbatim** as the
  input item (strip only `status`, grok's rule). A `Thinking` block that doesn't
  parse that way (e.g. Anthropic-signed thinking from another wire — impossible
  in a single-wire session, or `signature: None`) → **skip**, mirroring the
  Anthropic wire's drop-unsigned rule (`anthropic/build.rs:231-241`).
- `RedactedThinking` on this wire's build → skip (it is Anthropic's encrypted
  block; a session never crosses wires).

Why reuse instead of a new `ContentBlock::ReasoningItem` variant: the
`signature` field is *documented as* "opaque provider signature required to
replay the thinking block" (`locode-protocol/src/lib.rs:81-82`) — this is
exactly that, with the whole item as the opaque payload; the engine already
appends and replays `Thinking` verbatim (Task 6), the report/stream stay
readable (`text` = the human-visible summary), and zero downstream code
changes. The alternative (a first-class opaque variant) is cleaner typing at
the cost of an ADR-0013 amendment + engine/report churn for no behavioral
difference; ADR-0013's `RedactedThinking` precedent shows either is acceptable.
Proposed amendment text documenting the encoding: §8.2. **Open question Q3.**

Ordering: reasoning items are emitted **in block position** (before the
`function_call` items that follow them), preserving "reasoning stays contiguous
with the following tool call" — the same invariant as Anthropic signatures
(`sampling-comparison.md:71`) and the documented reason a dropped reasoning item
degrades or 400s multi-turn tool use (OpenAI cookbook: "you do need to include
the reasoning items"; exact 400 text unconfirmed officially — the parse must
not rely on it).

### 4.5 Caching & OpenRouter specifics

- **Prompt caching is automatic** (>1024-token prefixes; OpenAI guide) — no
  `cache_control` analog exists; `CacheHint` is a no-op on this wire
  (documented). The lever we do have:
- **`prompt_cache_key`:** codex sets it to the **session id**
  (`client.rs:469-473,888`); grok sets **nothing** and relies on byte-stable
  ordering (verified — every `prompt_cache_key` in grok is `None`;
  `conversation.rs:2148`. The survey's sampling-comparison.md:67 claim that
  grok sends one is **wrong** — flagged for a survey correction). We default
  `None` (byte-stable ordering is guaranteed by our deterministic build) and
  expose `config.prompt_cache_key` so the facade can pass the session id later
  (open question Q4).
- **Usage mapping** (authoritative, ADR-0007): `input_tokens → input_tokens`,
  `output_tokens → output_tokens`,
  `input_tokens_details.cached_tokens → cache_read_tokens`,
  `input_tokens_details.cache_write_tokens → cache_creation_tokens` (2026 spec
  addition; absent → 0). `output_tokens_details.reasoning_tokens` has no
  protocol slot — dropped in v0 (noted; adding it to `Usage` is a separate,
  additive protocol change nobody needs yet).
- **OpenRouter backend quirks** (auto-detected like the Messages wire,
  `anthropic/config.rs:56-65` pattern):
  - `provider` prefs injected into the body (probe P5 works on `/v1/responses`);
    default `{allow_fallbacks:false, require_parameters:true}` — the
    cc-reverse-proxy rationale carries over (`require_parameters` prevents
    routing to a backend that silently drops `tools`/`reasoning`); **no
    `ignore:["amazon-bedrock"]`** here (that trio entry was Anthropic-routing
    specific). Config-overridable, `None`-able.
  - Attribution headers are **not** sent in v0 (`HTTP-Referer` /
    `X-OpenRouter-Title` are leaderboard-only; `extra_headers` is the escape
    hatch).
  - OpenRouter model ids are namespaced (`openai/gpt-5-mini`, `x-ai/grok-4.5`)
    — the user sets `LOCODE_MODEL` accordingly, as with the Messages wire.

### 4.6 Freeform/custom tools — emission, dispatch, degradation

- **Emission:** `ToolInputFormat::JsonSchema` → the FLAT function shape
  `{type:"function", name, description, parameters:
  normalize_input_schema(...), strict:false}` (flatness per OpenAI spec
  `FunctionToolParam` and codex `ResponsesApiTool` — **not** Chat Completions'
  nested `function:{}`); `strict:false` because our schemars-derived schemas
  don't satisfy strict-mode's all-fields-required rule — matches codex, which
  sets `strict` explicitly per tool and defaults false.
  `ToolInputFormat::Freeform` → `{type:"custom", name, description,
  format:{type:"grammar", syntax:"lark", definition}}` — probe P2 validated
  this exact shape end-to-end through OpenRouter.
- **Parse:** a `custom_tool_call` output item → `ContentBlock::ToolUse { id:
  call_id, name, input: Value::String(input) }`. A `function_call` item →
  `ToolUse { id: call_id, name, input: parse(arguments) }`; if `arguments` is
  invalid JSON, keep `Value::String(arguments)` — dispatch will then produce a
  soft "invalid arguments" `tool_result` the model can react to (better than
  grok's silent `"{}"` substitution, `conversation.rs:1658-1674`, which hides
  the model's mistake from the model).
- **Dispatch:** `Value::String` args flow through the existing door untouched:
  Task 19's `apply_patch` declares `Args` deserializable from **both** a JSON
  string and the `{"input": string}` fallback object (untagged enum), so
  `serde_json::from_value(Value::String(patch))` decodes directly. No registry
  changes beyond §3.1's defaulted `input_format()`.
- **Replay:** the build side needs to know whether a historical `ToolUse` was
  custom (→ `custom_tool_call` + `custom_tool_call_output`) or function (→
  `function_call` + `function_call_output`). Rule: **a `ToolUse` whose `input`
  is `Value::String` AND whose name resolves to a `Freeform` spec in
  `req.tools`** is custom; everything else is function. (Both item families
  pair by `call_id`; codex notes `CustomToolCallOutput` even shares the
  function-output wire encoding, `models.rs:933-935` — but we emit the proper
  typed items per the spec.)
- **Degradation (`custom_tools_supported = false`, or any non-Responses wire):**
  a `Freeform` spec renders as a **JSON function tool** with the historical
  codex fallback schema:
  `{type:"object", properties:{input:{type:"string", description:"The entire
  contents of the …"}}, required:["input"], additionalProperties:false}` —
  this is the JSON-string framing ADR-0012 already reserved for the codex pack
  on the Anthropic wire ("delivered as a normal tool with a single JSON string
  arg `{patch}`… freeform-grammar delivery deferred to a Responses wire",
  ADR-0012 apply_patch clarification; the shape follows codex's *removed*
  historical JSON variant — at the pinned commit `ApplyPatchToolType` is
  Freeform-only, `protocol/src/openai_models.rs:284-288`, so the fallback shape
  is ours to define and `{input: string}` is its documented ancestor). The
  fallback is implemented **once**, as a helper in `locode-provider` shared by
  all three wires: `fn degrade_freeform(spec: &ToolSpec) -> wire-agnostic
  {name, description, parameters}`. xAI models via Responses need it too
  (probe P3: xAI 422s `type:"custom"`). Default `custom_tools_supported:
  true`; flip per-config for xAI (open question Q5 — auto-detect by model
  prefix?).

### 4.7 Error classification (`openai/mod.rs::classify`)

Error body shapes:
- OpenAI: `{"error": {"message", "type", "param", "code"}}` (spec
  `ErrorResponse`; `code` is a **string**).
- OpenRouter: `{"error": {"code": <number>, "message", "metadata"}}` (errors
  doc) — HTTP status mirrors `code`. Parse both (untagged or manual): sniff
  `code` as string-or-number, keep `message`.

| Condition | `ProviderError` | Note |
|---|---|---|
| transport | `Transport` (retry) | shared |
| 5xx | `Api{status}` (retry) | incl. OpenRouter 502 "provider bad response", 503 "no provider meets requirements" |
| **429 + code `insufficient_quota`** | **`Quota` (terminal)** | THE OpenAI-family trap: quota exhaustion arrives as 429 (error-codes guide "You exceeded your current quota" vs "Rate limit reached"); blind 429-retry hammers a dead account. Match `code == "insufficient_quota"` first, then wording "exceeded your current quota" |
| other 429 | `RateLimited{retry_after}` | cap-2 then surface (shared policy) — codex also refuses transport-level 429 retries (`retry_429: false`, `model-provider-info/src/lib.rs:262-268`) |
| 402 (OpenRouter "insufficient credits") | `Quota` | OpenRouter-specific |
| 400/413 + `context_length_exceeded` code or "context window"/"maximum context length" wording | `ContextOverflow` | official enum unconfirmed (spec only promises "a 400 error" with `truncation:"disabled"` default) — sniff code AND wording, never rely on exact text |
| 401/403 | `Auth` (terminal) | no refresh seam in v0 |
| other 4xx | `Api{status}` terminal | |
| 200 + `status:"failed"` + `response.error` | map `error.code`: `rate_limit_exceeded`→`RateLimited`, `server_error`→retryable `Api{500}` (grok converts in-stream failures to retryable 500s deliberately, `stream/responses.rs:85-88,290-326`), else terminal `Api{400}` | the non-streaming analog of codex's `response.failed` mapping (`sse/responses.rs:387-417`) |
| 200 undeserializable | `Decode` | terminal |

No `x-should-retry` (Anthropic-only header) → `HttpFailure.force_terminal`
always false here. `Retry-After` honored via the shared parser; OpenRouter sends
`X-RateLimit-*` only on 429 responses (limits doc) — informational, unparsed in
v0.

### 4.8 The transport hoist — mechanics

Pure move + generalization, no behavior change to the Anthropic wire (its 60
tests must pass untouched):

1. Create `http.rs` with the §2 table's contents; `run_with_retry<T>`
   parameterized on the success type.
2. `anthropic/retry.rs` / `anthropic/error.rs` keep their public paths as thin
   re-exports (`pub use crate::http::{RetryPolicy, …}`) so
   `locode_provider::anthropic::{RetryPolicy, run_with_retry, HttpFailure}`
   stay valid — or re-point the `anthropic/mod.rs` re-exports and drop the
   inner modules (preferred; the facade re-exports are the public surface).
3. `normalize_input_schema` moves; `anthropic::build` imports it.
4. `ErrorBody`/`classify` stay wire-local (Anthropic keeps its wording sniffs
   + `x-should-retry`; OpenAI-family gets §4.7).

### 4.9 Deferred-streaming seam (documented, not built)

Event names to handle when streaming lands (grok's behavioral set,
`stream/responses.rs:184-391`; full spec list in the OpenAPI `x-oaiMeta`):
`response.created`, `response.output_item.added/done`,
`response.output_text.delta/done`, `response.function_call_arguments.delta/done`,
`response.custom_tool_call_input.delta/done`,
`response.reasoning_summary_text.delta/done`, `response.reasoning_text.delta/done`,
`response.completed`, `response.incomplete`, `response.failed`, `error`.
`ToolCallAssembler` (Task 5) already covers the args-accumulation pattern.
Codex's idle-timeout discipline (`process_sse_with_treatment`,
`sse/responses.rs:492-529`) is the reference for the eventual reader.

---

## 5. Design decisions (each: source `file:line` · why · why-not · differences)

1. **Stateless always (`store:false`, no `previous_response_id`).** — *Source:*
   grok `client.rs:1090-1093` ("breaks ZDR compliance"); codex `client.rs:898`
   (false for OpenAI, true only Azure); OpenRouter 400s stateful requests
   (probe P1 + doc). *Why:* the one mode all three targets share; history is
   already client-owned in our engine. *Why-not (stateful+item refs):* ties the
   transcript to a server store, breaks OpenRouter, adds an id-lifetime state
   machine for zero benefit at our scale. *Difference:* codex's WebSocket path
   does incremental input — an optimization we explicitly defer.

2. **Hand-rolled minimal DTOs, not an SDK crate.** — *Source:* codex hand-rolls
   (`codex-api/src/common.rs:215-239`); grok pulls `async_openai` types
   (`xai-grok-sampling-types/src/lib.rs:28`) *and then has to patch its
   serialization* (`patch_reasoning_text_types`, `conversation.rs:2200-2218` —
   the crate omits the required `type:"reasoning_text"` discriminator). *Why:*
   ADR-0007 rejected SDK delegation for the Anthropic wire; the same reasoning
   holds, and grok's patch dance is the cautionary tale — a dependency that
   must be post-patched is worse than 300 lines of serde. Our opaque
   reasoning replay (§4.4) sidesteps the entire content-discriminator problem.
   *Why-not (`async_openai`):* huge dep, ask-first, and demonstrably not
   faithful to the wire without patching. *Difference:* we model only what we
   send/read; `#[serde(other)] Other` absorbs the rest.

3. **System → `instructions` (default), input-message opt-out.** — *Source:*
   codex `client.rs:862` + `common.rs:217-218` vs grok `conversation.rs:2140,
   2223-2230`; probes P4/P5. *Why:* the codex pack must reproduce codex's
   request shape (`instructions` carries the base prompt); works through
   OpenRouter for OpenAI **and** xAI models (probes). *Why-not
   (grok's input-message shape as default):* would make the codex pack's
   requests observably non-codex; the knob preserves grok's shape for anyone
   who needs it. *Difference:* the two model harnesses disagree; we side with
   the harness this wire exists to serve, and note `instructions` is NOT
   carried over server-side turns (irrelevant — we're stateless).

4. **Whole-item-opaque reasoning replay via `Thinking.signature`.** — *Source:*
   grok stores the native item whole (`conversation.rs:45-57`) and strips only
   `status` on replay (`:2239-2247`); codex resends
   `ResponseItem::Reasoning{id, summary, content, encrypted_content}`
   (`models.rs:840-852`); spec requires `id`+`summary` on input reasoning
   items; probes P4/P5 show unmodeled fields (`format`). *Why:* lossless replay
   without a protocol change; `signature` is by-contract the opaque replay
   payload (`locode-protocol/src/lib.rs:81-82`). *Why-not (new ContentBlock
   variant):* same behavior, more churn (ADR-0013 amendment, report/engine
   touchpoints); revisit only if a second consumer of structured reasoning
   items appears. *Difference:* Anthropic thinking replays a *block* inside the
   assistant turn; Responses replays a *sibling item* — both encode as
   `Thinking` blocks in the assistant message, and each wire re-emits its own
   native shape.

5. **`call_id` verbatim, item `id` omitted.** — *Source:* grok uses `call_id`
   both directions with `id: None` (`conversation.rs:2269-2273,2303-2306`,
   parse `:1962-1972`); codex clears ids under `store=false`
   (`client.rs:910-924`); spec requires only `call_id` on replayed
   calls/outputs. *Why:* ADR-0007 verbatim-id rule; ids are server-store
   bookkeeping we don't participate in. *Why-not (send ids):* they're
   meaningless without `store` and codex actively strips them. *Difference:*
   none — all sources agree; note Responses ids can be `call-…` (xAI, probe
   P3) not just `call_…` — no format assumptions.

6. **Freeform tools in `ToolSpec` as a two-variant input format.** — *Source:*
   codex `ToolSpec::Freeform(FreeformTool)` (`tools/src/tool_spec.rs:49-50`,
   `responses_api.rs:11-23`); OpenAI `CustomToolParam` + grammar format; probe
   P2. *Why:* the protocol must express "raw text constrained by a grammar" or
   the codex pack is unimplementable (todo.md Task 18 criterion). *Why-not
   (optional `format` field beside `parameters`):* invalid states (both set /
   neither meaningful); *why-not (ToolSpec as enum):* churns every
   `spec.name` call site for nothing. *Difference:* codex's enum also carries
   hosted-tool variants (web_search etc.) — ours stays two-variant; hosted
   tools are a different concept (server-side, no dispatch) and out of scope.

7. **Function-tool shape is FLAT; `strict:false`.** — *Source:* OpenAI spec
   `FunctionToolParam` (flat `{type,name,description,parameters,strict}` —
   explicitly unlike Chat's nested `function:{}`); codex `ResponsesApiTool`
   (`responses_api.rs:25-38`). *Why:* it's the wire format; `strict:true` would
   demand schema transformations (all-required, no defaults) our derived
   schemas don't satisfy. *Why-not (strict):* schema rewriting for marginal
   arg-validity gains the dispatch door already soft-handles. *Difference:*
   codex sets per-tool `strict`; grok (via async_openai defaults) doesn't
   either.

8. **`insufficient_quota` 429 → `Quota`, not `RateLimited`.** — *Source:*
   OpenAI error-codes guide (two distinct 429s); our exhaustive
   `ProviderError` taxonomy with terminal `Quota` (`provider.rs:66-69`). *Why:*
   retrying a billing failure is the classic OpenAI-client bug; grok's
   taxonomy treats quota as fatal the same way. *Why-not (uniform 429):* the
   Anthropic wire never had this ambiguity — it's new with this family.
   *Difference:* OpenRouter signals credits exhaustion as **402** — both routes
   land on `Quota`.

9. **Non-streaming primary; `response.failed`-in-200 mapped like grok.** —
   *Source:* grok's non-streaming `create_response` (`client.rs:1196-1205`) and
   its deliberate failed→retryable-500 conversion (`stream/responses.rs:85-88`);
   codex streaming-only (`client.rs:899`). *Why:* ADR-0005 non-streaming loop;
   probes prove the path live. *Why-not (streaming because codex does):* Task
   12 already inverted this for Claude Code; same trade. *Difference:* we read
   `status` + `incomplete_details` off the terminal object instead of
   `response.completed`/`response.incomplete` events.

10. **Shared transport layer under `locode-provider::http`.** — *Source:* the
    Task-12 modules were written for this (`normalize_input_schema` doc:
    "hoist … when a second wire lands", `anthropic/build.rs:99`); grok's single
    sampler serves three backends with one retry loop + per-shape error
    conversion (`sampler/src/retry.rs:144-245` is backend-agnostic). *Why:*
    Task 17 consumes it next; duplicating retry semantics is how wires drift.
    *Why-not (a `locode-http` crate):* one consumer crate; module suffices
    (ADR-0002 boundary discipline — "ask first" on new crates).

11. **`reasoning_effort` per-wire asymmetry (`Minimal` is real here).** —
    *Source:* grok `to_responses_api` vs `to_messages_api` split
    (`types.rs:812-820`, request docs `request.rs:54-58`); OpenAI 2026 effort
    enum incl. `minimal`. *Why:* the neutral enum maps per-wire by design;
    collapsing Minimal to "off" is an Anthropic-ism. *Difference:* documented
    in both wires' rustdoc so A/B users aren't surprised.

---

## 6. Tests

### 6.1 Request shape (`responses_request_shape.rs`, on serialized JSON)

- **Stateless invariants:** every built request has `store == false`; the JSON
  contains **no** `previous_response_id` key; `include ==
  ["reasoning.encrypted_content"]`.
- **System hoist:** `[System, User]` → `instructions` set, no system message
  item; two System messages concatenate in order; `SystemPlacement::InputMessage`
  → no `instructions`, leading `role:"system"` message item.
- **Developer** → `role:"developer"` message item at its stream position.
- **tool_result explosion + ordering:** a User message
  `[ToolResult(a), ToolResult(b)]` after an Assistant
  `[ToolUse(a), ToolUse(b)]` → two `function_call` items then two
  `function_call_output` items, `call_id`s verbatim, order preserved.
- **Reasoning replay:** a `Thinking{text, signature:Some(item-json)}` block
  (fixture item carrying an unknown `format` field) → the input contains that
  reasoning item **byte-equivalent** (minus `status`), positioned before the
  following `function_call`. A `Thinking{signature: None}` → no item emitted.
- **Freeform emission:** a `Freeform` ToolSpec → `{type:"custom", …,
  format:{type:"grammar", syntax:"lark", definition}}`; with
  `custom_tools_supported=false` → the `{input: string}` function fallback,
  `additionalProperties:false`.
- **Custom-call replay:** `ToolUse{input: Value::String}` + freeform spec →
  `custom_tool_call` item + paired `custom_tool_call_output`.
- **Function shape flatness:** JSON-schema tool → flat
  `{type:"function", name, description, parameters, strict:false}`, `$schema`
  stripped, no nested `function` key.
- **Effort mapping table** incl. `None` → no `reasoning` key; `max_output_tokens`
  floor 16; OpenRouter backend → `provider` prefs injected (default trio),
  Native → absent.

### 6.2 Parse (`responses_parse.rs`, fixtures modeled on probes P2–P5)

- output `[reasoning, message]` → `Thinking` (text = summary concat; signature
  = whole item incl. unknown fields) + `Text`; empty summary → `Thinking{text:""}`.
- `function_call` → `ToolUse` id/name verbatim, arguments parsed to object;
  **invalid-JSON arguments** → `input == Value::String(raw)` (no silent `{}`).
- `custom_tool_call` → `ToolUse{input: Value::String}`.
- usage: all four mapped (incl. `cache_write_tokens`); missing details → 0;
  OpenRouter extras (`cost`, `is_byok`) ignored.
- stop mapping: completed+calls → `ToolUse`; completed w/o → `EndTurn`;
  `incomplete{max_output_tokens}` → `MaxTokens`; `incomplete{content_filter}` →
  `Unknown("content_filter")`; unknown output item type (`web_search_call`) →
  skipped, parse succeeds.
- `status:"failed"` + `error{code:"rate_limit_exceeded"}` → `RateLimited`;
  `server_error` → retryable `Api{500}`.

### 6.3 Classification + transport (`openai_classify.rs` + provider tests)

- 429 + `insufficient_quota` → `Quota` terminal (no retry); 429 plain →
  `RateLimited`, `Retry-After` honored, surfaced after cap 2.
- 402 OpenRouter body (numeric `code`) parses → `Quota`; OpenRouter 502/503 →
  retryable; 400 + "maximum context length" → `ContextOverflow`.
- Hoist regression: the **anthropic suite runs unchanged** (60 tests) against
  the relocated `http` layer.
- Canned `TcpListener` end-to-end (pattern: `tests/anthropic_provider.rs`):
  happy path asserting method/path `/v1/responses`, `authorization: Bearer`,
  body invariants (`store:false`, `include`), completion round-trip; a 429→200
  retry script; a quota-429 script (single attempt).

### 6.4 Live smoke (`#[ignore]`, manual, OpenRouter — never CI)

1. `openai/gpt-5-mini`: freeform grammar tool → `custom_tool_call` → replay
   output + reasoning item → second turn completes (codifies P2/P4).
2. `x-ai/grok-4.5` with `custom_tools_supported=false`: function tools +
   encrypted reasoning replay across 2+ turns, no 400 (codifies P3/P5).
3. Cache proof: `cached_tokens > 0` on turn 2 with byte-stable prefix.
4. One real error body through `classify` (bad model id).

---

## 7. Dependencies to add

**None.** `reqwest`/`tokio`/`serde`/`serde_json`/`thiserror`/`async-trait`/
`rand` are already in `locode-provider` (Task 12, approved §9.1). The DTOs are
hand-rolled precisely to avoid an `async_openai`-class dependency (§5.2).
(`locode-exec` gains only the `openai-responses` value on the existing
`--api-schema` clap enum — no new crate deps there either.)

---

## 8. Proposed ADR/SPEC deltas (apply at implementation time — do NOT edit now)

### 8.1 ADR-0003 (typed tool contract) — dated amendment

> **Amendment (Task 18): freeform tool input.** `ToolSpec.parameters` becomes
> `ToolSpec.input: ToolInputFormat`, a two-variant enum: `JsonSchema {
> parameters }` (every schemars-derived tool, unchanged semantics) and
> `Freeform { syntax, definition }` (raw text constrained by a server-side
> grammar — OpenAI Responses `custom` tools; the codex pack's `apply_patch`).
> `Tool`/`DynTool` gain a defaulted `input_format()` returning `JsonSchema` from
> the derived schema, so existing tools are untouched. A freeform tool still
> implements the typed contract — its `Args` deserializes from a JSON string
> (and from the `{input: string}` fallback object) — so dispatch remains the
> one door. Wires without custom-tool support (Anthropic Messages, OpenAI Chat,
> xAI-through-Responses) render a `Freeform` spec as a JSON function tool with
> the single-string `{input}` schema (the ADR-0012 JSON-string framing).

### 8.2 ADR-0013 (conversation protocol) — dated amendment

> **Amendment (Task 18): OpenAI Responses reasoning items ride in
> `Thinking.signature`.** On the `openai-responses` wire, a response `reasoning`
> item is parsed into `ContentBlock::Thinking` with `text` = the concatenated
> summary texts (possibly empty) and `signature` = the **complete reasoning
> item serialized as a JSON string** (id, summary, content, encrypted_content,
> and any provider extras, verbatim). On build, a `Thinking` block whose
> signature parses as a `"type":"reasoning"` object is re-emitted as that item
> (minus the output-only `status`); other `Thinking` blocks are skipped by this
> wire. This is the Responses analog of Anthropic's signature replay: the
> signature field is the opaque, wire-owned replay payload; no other crate
> interprets it. Sessions are single-wire, so cross-wire signatures never mix.

### 8.3 ADR-0007 (provider trait) — dated amendment

> **Amendment (Task 18): second/third wires + shared transport.** The OpenAI
> Responses wire (`api_schema = "openai-responses"`) lands as designed: Bearer
> auth always, stateless (`store:false`, never `previous_response_id`),
> `include:["reasoning.encrypted_content"]`, System→`instructions` hoist
> (configurable), OpenRouter backend auto-detection with `provider`-prefs
> injection. The transport tier (`RetryPolicy`, `run_with_retry`, `backoff`,
> `HttpFailure`, `Retry-After` parsing) and the schema-normalization helper are
> hoisted to a shared `locode-provider::http` layer consumed by all wires;
> error-body classification stays per-wire. New family-specific terminal rule:
> an OpenAI 429 with code `insufficient_quota` (and OpenRouter 402) classifies
> as `Quota`, not `RateLimited`.

### 8.4 SPEC.md

- Tech-stack table row "First provider wire": note the three shipped schemas
  (`anthropic`, `openai-responses` (Task 18), `openai-chat` (Task 17)).
- Open Question 3's "verify whether Anthropic and OpenAI accept the same
  derived JSON Schema": record the answer (shared `normalize_input_schema`,
  verified live) once the smoke passes.
- Deferred list: remove "freeform-grammar `apply_patch` (OpenAI Responses
  wire)" when this lands.

### 8.5 Survey erratum (separate repo, note only)

`survey/05-comparative/sampling-comparison.md:67` claims grok sends a stable
`prompt_cache_key`; the grok Responses code sets none
(`conversation.rs:2148`, repo-wide grep). File a correction in
`coding-cli-survey` when convenient.

---

## 9. Open questions (for user sign-off before implementation)

1. **Default model for the OpenAI wires.** The Anthropic wire defaults
   `LOCODE_MODEL` to `claude-sonnet-5`. Proposal: this wire has **no default**
   — `LOCODE_MODEL` is required when `--api-schema openai-responses` (error
   otherwise), since any default (gpt-5.x? o-series?) would be arbitrary and
   the user's models are OpenRouter-namespaced anyway. Alternative: default
   `openai/gpt-5-mini` for frictionless smoke runs. Which?
2. **`reasoning.summary` default.** Proposed `None` (omit; cheapest, most
   compatible — codex gates it per-model, xAI ignores it and summarizes anyway,
   probe P5). Alternative: `"auto"` for richer `Thinking.text` in reports at
   the cost of output tokens. Which default?
3. **Reasoning-replay representation.** Confirm the reuse-`Thinking.signature`
   encoding (§4.4, amendment 8.2) over a new `ContentBlock` variant. This is
   the plan's one protocol-semantics call.
4. **`prompt_cache_key`.** Default `None` (grok's behavior). Codex sets the
   session id — but wiring that means the facade passes the session id into
   provider config at construction (small `locode-exec` change). Do it now, or
   leave the config field dormant?
5. **xAI custom-tool degradation trigger.** Manual config
   (`custom_tools_supported=false`) vs auto-detection (model id starts with
   `x-ai/` / `grok`). Proposal: manual in v0 + a doc note; auto-detection is a
   heuristic that will rot. Confirm?
6. **`ToolInputFormat` serde compat for `Event::Init.tools`.** `Init.tools` is
   `Vec<Value>` (serialized ToolSpecs). The rename `parameters` →
   `input.{type:"json_schema", parameters}` changes that JSON shape — stream
   consumers (none exist yet outside our own tests) would see the new form. OK
   to change without an Event-schema version bump (Event is `#[non_exhaustive]`
   and pre-1.0)?
