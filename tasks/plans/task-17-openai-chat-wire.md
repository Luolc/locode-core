# Task 17 — `locode-provider` OpenAI Chat Completions wire (`api_schema = "openai-chat"`)

> Implementation plan, written **before** code. The second OpenAI-family wire,
> implemented **after** Task 18 (order 18 → 17, user decision 2026-07-18) because it
> **consumes the shared transport layer Task 18 hoists** (`locode-provider::http` +
> the `openai/` family module). Chat Completions is the **broadest
> lowest-common-denominator schema** — OpenRouter's default surface for every
> model/provider (no Messages/Responses translation in the path), grok build's
> `ApiBackend` **default** (`types.rs:1015`), and the shape spoken by vLLM/Ollama/
> Together/Groq via base-URL override.
>
> Cites grok build's Chat Completions backend (submodule `b189869b`) as the working
> model — codex **cannot** be cited here: it deleted its chat path entirely
> (`wire_api = "chat"` is a hard config error, `model-provider-info/src/lib.rs:50,80`)
> — plus the official OpenAI OpenAPI spec (pulled 2026-07-16) and OpenRouter docs.
> Repo ADRs: ADR-0007, ADR-0013 (OpenAI mapping table), ADR-0004.

---

## 1. Purpose & scope

Implement `OpenAiChatProvider`: `api_schema() = "openai-chat"`,
`POST {base_url}/v1/chat/completions`, non-streaming, always-Bearer. It maps the
4-role protocol onto chat messages (System → `role:"system"`, Developer →
`role:"developer"`, tool_result → exploded `role:"tool"` messages), emits
**nested** function-tool definitions, groups an assistant turn's `ToolUse` blocks
into one `tool_calls` array, and parses `choices[0]` back into a `Completion`.

**Why this wire exists given Task 18** (both talk to OpenAI-family models): it
tests non-Anthropic models **without OpenRouter's Responses beta translation in
the path** (the user's motivation of record, todo.md Task 17), reaches every
OpenAI-compatible gateway that will never implement `/v1/responses`, and gives
the A/B bed a "reasoning-blind" control wire (§4.4).

**In scope (v0):**
- Build/parse per §4; verbatim `tool_calls[].id` round-trip; usage mapping
  (incl. `cached_tokens`/`cache_write_tokens`); `reasoning_effort` param.
- Freeform-`ToolSpec` degradation to the `{input: string}` function fallback
  (always, on this wire — via Task 18's shared `degrade_freeform` helper).
- Reuse of `http::{RetryPolicy, run_with_retry, HttpFailure}`,
  `openai::{OpenAiModelConfig-family config, classify}` (Task 18), incl. the
  429-`insufficient_quota` → `Quota` rule and OpenRouter's numeric-code error
  body.
- Non-standard reasoning **capture** (OpenRouter `message.reasoning`, xAI/
  DeepSeek-style `message.reasoning_content`) into `Thinking{signature: None}`
  — report-visible, **never replayed** (§4.4).
- Fixture + canned-server tests; `--api-schema openai-chat` in `locode-exec`;
  manual live smoke.

**Deferred:**
- Streaming (`choices[].delta`, `tool_calls` index-fragment assembly — the
  `ToolCallAssembler` (Task 5) was literally built for this shape; seam
  reserved).
- Chat-side custom tools (the 2026 spec added `CustomToolChatCompletions`; we
  degrade freeform specs instead — one framing across all non-Responses wires;
  revisit if a chat-only deployment ever needs real grammar tools).
- `response_format`/structured outputs (with `--json-schema`, SPEC Open Q3;
  note the chat shape is **nested** `json_schema:{…}` unlike Responses' flat
  form — recorded so the future task doesn't assume symmetry).
- `store`, `prediction`, penalties, `n>1`, logprobs, audio; OpenRouter
  `models`-fallback arrays / `route` / plugins.
- OpenRouter `reasoning: {max_tokens}` budget encoding (their Anthropic-style
  extension) — we send only the OpenAI-standard `reasoning_effort`.

---

## 2. Module layout

```
crates/locode-provider/src/openai/
├── mod.rs / common.rs        # (Task 18) config, backend detect, classify, error DTOs
└── chat/
    ├── mod.rs                # OpenAiChatProvider + impl Provider
    ├── wire.rs               # serde DTOs: ChatRequest, ChatMessage, ToolCall,
    │                         #   ChatResponse, Choice, ChatUsage, FinishReason
    ├── build.rs              # build_request(&ConversationRequest, &config)
    └── parse.rs              # ChatResponse -> Completion; finish_reason mapping
crates/locode-provider/tests/
├── chat_request_shape.rs
├── chat_parse.rs
├── chat_provider.rs          # canned TcpListener
└── chat_live_smoke.rs        # #[ignore]d manual
```

Same wire/build/parse split as `anthropic/` and `responses/`; transport, config,
and classification all come from the shared layers — this crate-internal reuse
is the point of doing 18 first. Grok's layout agrees: one `ChatCompletionRequest`
type set (`xai-grok-sampling-types/src/types.rs:63-112`) + one conversion
(`conversation.rs:2033-2098`) + the shared sampler.

## 3. Key types & signatures

### 3.1 Config

Reuses `OpenAiModelConfig` (Task 18 §3.2) — same env resolution
(`LOCODE_BASE_URL`/`LOCODE_API_KEY`/`LOCODE_MODEL`), same
`OpenAiBackend::{Native, OpenRouter, Proxy}` detection, same
`provider_prefs`. Two chat-only knobs join it (or a small
`ChatConfig` wrapper embedding the shared record — implementation's choice):

```rust
/// Which token-limit parameter to send (§4.3).
pub enum TokenLimitParam {
    /// `max_completion_tokens` — current OpenAI (default).
    MaxCompletionTokens,
    /// legacy `max_tokens` — pre-2024 gateways (vLLM/older proxies).
    MaxTokens,
}

/// How Role::Developer is rendered (§4.2).
pub enum ChatDeveloperRendering {
    /// `role:"developer"` message — OpenAI-native (default; ADR-0013 exact match).
    DeveloperRole,
    /// portable fallback: `role:"user"` + `<system-reminder>` wrapper
    /// (same rendering as the Anthropic wire's default).
    SystemReminder,
}
```

### 3.2 Wire DTOs (`chat/wire.rs`)

Modeled on grok's types (they are lean and battle-tested), trimmed to what we
send/read:

```rust
#[derive(Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,       // or max_tokens per TokenLimitParam
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ChatToolDef>>,          // NESTED shape
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,              // v0: None (server "auto" w/ tools)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,         // top-level (grok types.rs:89)
    pub stream: bool,                             // false in v0
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<serde_json::Value>,      // OpenRouter prefs
}

#[derive(Serialize)]
pub struct ChatMessage {                          // grok ChatRequestMessage (types.rs:230-246)
    pub role: String,                             // system|developer|user|assistant|tool
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,                  // None allowed w/ tool_calls (spec)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCallDef>,             // assistant replay
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,             // role:"tool" pairing
}

#[derive(Serialize, Deserialize)]
pub struct ToolCallDef {                          // grok ToolCallRequest (types.rs:435-457)
    pub id: String,
    #[serde(rename = "type")] pub kind: String,   // "function"
    pub function: FunctionCallDef,                // { name, arguments: String }
}

#[derive(Serialize)]
pub struct ChatToolDef {                          // NESTED (spec ChatCompletionTool)
    #[serde(rename = "type")] pub kind: String,   // "function"
    pub function: FunctionDef,                    // { name, description, parameters }
}

#[derive(Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,                     // we read choices[0]
    pub usage: Option<ChatUsage>,
}
#[derive(Deserialize)]
pub struct Choice {
    pub message: ChatResponseMessage,
    pub finish_reason: Option<FinishReason>,
}
#[derive(Deserialize)]
pub struct ChatResponseMessage {
    pub content: Option<String>,
    pub refusal: Option<String>,
    #[serde(default)] pub tool_calls: Vec<ToolCallDef>,
    // Non-standard reasoning surfaces, capture-only (§4.4):
    pub reasoning: Option<String>,                // OpenRouter normalized field
    pub reasoning_content: Option<String>,        // xAI/DeepSeek-style (grok types.rs:491-503)
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {                           // grok types.rs:480-488 + catch-all
    Stop, Length, ToolCalls, ContentFilter, FunctionCall,
    #[serde(other)] Unknown,
}
#[derive(Deserialize)]
pub struct ChatUsage {                            // grok types.rs:535-557 + 2026 fields
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub prompt_tokens_details: Option<PromptTokensDetails>,   // { cached_tokens, cache_write_tokens }
    pub completion_tokens_details: Option<CompletionTokensDetails>, // { reasoning_tokens }
}
```

All response structs tolerate unknown fields; usage counters are `Option`
(Task-12 OpenRouter lesson).

### 3.3 Provider

```rust
pub struct OpenAiChatProvider { http, config, retry }   // mirror of responses/mod.rs

impl Provider for OpenAiChatProvider {
    fn api_schema(&self) -> &str { "openai-chat" }
    // complete(): repair_pairing clone → build → http::run_with_retry(send_once)
}
```

---

## 4. Behavior / algorithms

### 4.1 Message mapping — ADR-0013 roles → chat messages

Per the ADR-0013 OpenAI table, with grok's conversion
(`conversation_item_to_chat_message`, `conversation.rs:~1690-1810`) as the
reference:

- **`Role::System`** → `role:"system"` message(s), one per System message, in
  position (leading in practice). No hoisting needed — chat has a native
  system role (the spec keeps accepting `system`; "developer replaces system"
  is o1+ *guidance* and the server aliases them — we send `system`, the
  broadest form; no knob unless a gateway forces one).
- **`Role::Developer`** → default `role:"developer"` (ADR-0013: "exact semantic
  match"). Fallback knob `ChatDeveloperRendering::SystemReminder` → a
  `role:"user"` message wrapped in `<system-reminder>…</system-reminder>`,
  byte-identical to the Anthropic wire's portable rendering
  (`anthropic/build.rs:135-141`) — for gateways/models that reject the
  developer role (grok has no developer concept at all; its `Role` enum is
  `System|User|Assistant|Tool`, `types.rs:357-362`).
- **`Role::User`** → walk blocks in order, splitting exactly like the
  Responses wire: contiguous `Text` runs → one `role:"user"` message;
  each `ToolResult{tool_use_id, content, is_error}` → one **`role:"tool"`
  message `{tool_call_id: tool_use_id, content: <joined text chunks>}`**
  (grok `ChatRequestMessage::tool`, `conversation.rs:1765-1790`; spec requires
  all three fields). `is_error` has no wire slot — the text carries it (same
  note as Task 18 §4.2). Images in tool results deferred (spec limits tool
  content parts to text anyway).
  Ordering invariant: the spec demands every `tool_call_id` be answered by a
  tool message **directly after** the assistant turn that called it — our
  engine appends exactly one User message of ToolResults right behind the
  assistant turn, and `repair_pairing` (run defensively pre-send, as in every
  wire) guarantees completeness, so explosion preserves validity by
  construction (ADR-0004).
- **`Role::Assistant`** → **one** `role:"assistant"` message per protocol
  message: `content` = concatenated `Text` blocks (or `None` if none — spec:
  content "Required unless `tool_calls` … is specified"), `tool_calls` = every
  `ToolUse{id, name, input}` in block order as
  `{id, type:"function", function:{name, arguments: serialize(input)}}` —
  **arguments stringified** (ADR-0013 table; grok
  `ToolCallRequest::function(name, args).with_id(tc.id)`,
  `conversation.rs:1740-1748`), `id` verbatim. Chat cannot represent
  text/tool-call interleaving; flattening to (all text, then calls) is the
  universal chat-wire convention (grok does the same). `Thinking` /
  `RedactedThinking` blocks: **dropped on build** — §4.4.

### 4.2 Tools

- `ToolInputFormat::JsonSchema` → the **nested** definition
  `{type:"function", function:{name, description, parameters:
  normalize_input_schema(...)}}` (spec `ChatCompletionTool`; contrast the flat
  Responses form — the two shapes are the classic cross-wire trap, called out
  in `locode-protocol`'s `ToolSpec` docs already). `strict` omitted (default
  false).
- `ToolInputFormat::Freeform` → **always degraded** via the shared
  `degrade_freeform` helper (Task 18 §4.6) to the `{input: string}` function
  tool. The codex pack thus *runs* on this wire — with the JSON-string framing
  ADR-0012 reserved — but its apply_patch delivery is not grammar-constrained;
  the A/B report shows that through `api_schema`.
- `tool_choice`: omitted (server default: `auto` when tools present — grok
  only sets it when tools exist, `conversation.rs:2052-2061`; we don't need
  even that since omission is valid with tools).

### 4.3 Sampling & token params

- `TokenLimitParam::MaxCompletionTokens` (default) → `max_completion_tokens =
  min(sampling.max_tokens, cap)`; `MaxTokens` (legacy knob) → `max_tokens`
  (deprecated in the spec, "not compatible with o-series" — but the LCD wire
  must reach pre-2024 gateways; grok itself still sends `max_tokens`,
  `types.rs:63-112`). Never both.
- `temperature`/`top_p` pass through (reasoning models ignore/limit them
  per-model; not schema-enforced — we don't special-case).
- `reasoning_effort` = `Minimal→"minimal"`, `Low/Medium/High` likewise, `None` →
  omit (grok forwards its neutral effort directly, `conversation.rs:2087`;
  top-level param per spec). Same per-wire asymmetry note as Task 18 §4.3.

### 4.4 Reasoning on the chat wire — capture-only, never replayed

Chat Completions has **no reasoning-replay mechanism**: OpenAI's schema carries
no reasoning content on the assistant message at all (verified against the
spec — `reasoning_content` is a DeepSeek-ism; OpenRouter's `message.reasoning`
/ `reasoning_details` is a gateway extension). Consequences, made explicit:

- **Parse:** if `choices[0].message.reasoning` (OpenRouter) or
  `.reasoning_content` (xAI-style) is present and non-empty → prepend
  `ContentBlock::Thinking { text, signature: None }` to the completion, so
  reports/streams show the reasoning. Grok does the mirror-image on its side
  (folds stored reasoning into the assistant's `reasoning_content` field —
  `conversation.rs:1804-1809` — an xAI-specific replay we do NOT copy, because
  it is not OpenAI-portable).
- **Build:** `Thinking{signature: None}` blocks are **skipped** (nothing valid
  to send); `Thinking` with a Responses-envelope signature or
  `RedactedThinking` likewise skipped (foreign wires' payloads; single-wire
  sessions make this a non-event).
- **Documented caveat (the A/B story):** multi-turn tool use with reasoning
  models over chat **forfeits chain-of-thought continuity** — OpenAI's own
  cookbook quantifies the cost of dropping reasoning items (~3% SWE-bench,
  cache hit rate 80%→40%). This is precisely why Task 18 outranks Task 17 and
  why the codex pack pairs with `openai-responses`. The chat wire is the
  control condition, not the recommended path for reasoning models.

### 4.5 Parse → `Completion`

- Read `choices[0]` (`n` never sent → exactly one; absent → `Decode`).
- `content: Some(text)` non-empty → `Text` block. `refusal: Some(text)` →
  `Text` block carrying the refusal wording **and** stop forced to
  `StopReason::Refusal` (chat encodes refusals in-band, not as a
  finish_reason; our normalization gives the engine one consistent signal —
  the Anthropic wire's refusal handling precedent, Task 12 §9.1).
- Each `tool_calls[i]` → `ToolUse { id (verbatim), name, input: parse(arguments) }`;
  invalid JSON → `input = Value::String(raw)` (dispatch soft-errors it back to
  the model — same rule as Task 18 §4.6; NOT grok's silent `"{}"`,
  `conversation.rs:1658-1674`).
- Usage: `prompt_tokens→input_tokens`, `completion_tokens→output_tokens`,
  `prompt_tokens_details.cached_tokens→cache_read_tokens`,
  `prompt_tokens_details.cache_write_tokens→cache_creation_tokens`
  (2026 field; absent → 0). `reasoning_tokens` dropped (no protocol slot —
  same note as Task 18).
- `finish_reason` mapping: `stop→EndTurn`, `length→MaxTokens`,
  `tool_calls→ToolUse`, `content_filter→Unknown("content_filter")`,
  `function_call→ToolUse` (deprecated alias), unknown → `Unknown(raw)`,
  absent → `Unknown("(missing finish_reason)")` (never fail parse — the
  Task-12 discipline).

### 4.6 Transport & errors

Entirely shared with Task 18: Bearer header, `POST {base_url}/v1/chat/completions`,
`http::run_with_retry` (429 cap-2 surfaced, `Retry-After`, backoff+jitter),
`openai::classify` (429-`insufficient_quota`/402 → `Quota`; 400 context wording
→ `ContextOverflow`; OpenRouter numeric-code bodies; 5xx retryable; no
`x-should-retry`). OpenRouter backend → `provider` prefs injected
(`{allow_fallbacks:false, require_parameters:true}` default — on the chat
endpoint this is the *documented* home of provider preferences). Prompt caching
is automatic server-side; `CacheHint` is a no-op (documented), `cached_tokens`
reported back is the proof.

---

## 5. Design decisions (source · why · why-not · differences)

1. **Grok's chat backend is the reference; codex is a negative citation.** —
   *Source:* grok `ChatCompletionRequest`/conversion
   (`types.rs:63-112`, `conversation.rs:2033-2098`) — a live, maintained chat
   path; codex deleted chat (`model-provider-info/src/lib.rs:50,80` hard
   error). *Why:* pattern-match a harness that still ships this wire. *Why-not
   (skip chat like codex):* codex targets exactly one vendor; our wire exists
   for gateway breadth (grok's own default backend is ChatCompletions,
   `types.rs:1015`).

2. **Nested tool shape + stringified arguments.** — *Source:* spec
   `ChatCompletionTool`/`FunctionObject`; ADR-0013 mapping table ("`input` JSON
   → **stringified** `arguments`"); grok `ToolCallFunction{arguments: String}`
   (`types.rs:513-517`). *Why:* it is the wire; the flat/nested split vs
   Responses is the family's known trap — both wires cite it in rustdoc.
   *Difference:* arguments-as-string means invalid JSON is *possible* (spec
   even warns "the model does not always generate valid JSON") — handled by
   the `Value::String` soft-error path.

3. **`role:"tool"` explosion straight after the assistant turn.** — *Source:*
   spec (tool message requires `tool_call_id`, follows the calling turn);
   ADR-0013 ("a User message's `tool_result` blocks are exploded into separate
   `role:"tool"` messages"); grok `conversation.rs:1765-1790`. *Why:*
   structural requirement; our engine's append discipline + `repair_pairing`
   satisfies it by construction. *Difference vs anthropic wire:* tool results
   there stay inside a user turn; here they become first-class messages —
   both are renderings of the same protocol shape.

4. **Developer → `developer` role by default, portable fallback knob.** —
   *Source:* ADR-0013 table (exact match; the role exists in the spec's
   message union); the Anthropic wire's `DeveloperRendering` precedent
   (`anthropic/config.rs:87-95`). *Why:* fidelity to the protocol's semantics
   on the wire that natively has the role. *Why-not (fallback as default,
   like the Anthropic wire):* there the *native* form needs a beta; here the
   native form is standard — defaults follow the native capability.
   *Difference:* grok can't express this at all (no developer role in its
   model).

5. **`max_completion_tokens` default with a legacy `max_tokens` knob.** —
   *Source:* spec (`max_tokens` deprecated, o-series-incompatible); grok still
   sends `max_tokens` (`types.rs:63-112`). *Why:* current-OpenAI correctness
   by default; the knob keeps the LCD promise for older gateways. *Why-not
   (send both):* servers reject the pair.

6. **Reasoning capture-only; no `reasoning_content` replay.** — *Source:*
   OpenAI spec (no reasoning field on assistant messages — verified absent);
   grok's replay via `reasoning_content` is an xAI extension
   (`conversation.rs:1804-1809`, field `types.rs:244`); OpenRouter surfaces
   `message.reasoning`/`reasoning_details` (docs). *Why:* replaying a
   non-standard field to arbitrary OpenAI-compatible backends is undefined
   behavior; capture costs nothing and keeps reports informative. *Why-not
   (grok's fold-back):* portable-wire contract; xAI users get real replay on
   `openai-responses` (Task 18), which is the recommended wire for them
   anyway. *Difference:* the chat wire is deliberately the reasoning-blind
   control in the A/B bed.

7. **Refusal → `Text` + `StopReason::Refusal`.** — *Source:* spec
   `message.refusal`; Task 12 §9.1 (refusal = a normal Completion, engine
   maps it). *Why:* one refusal signal across wires despite chat's in-band
   encoding. *Why-not (finish_reason only):* chat has no refusal
   finish_reason; without normalization the engine would need per-wire
   knowledge, violating ADR-0007.

8. **Shared config/classify from `openai/` (no duplication).** — *Source:*
   Task 18 §2; grok's one-sampler-three-backends layout
   (`survey/03-grok-build/provider-api.md`). *Why:* the classification traps
   (`insufficient_quota`, OpenRouter 402/numeric codes) are family-wide, and
   fixing them twice guarantees drift.

---

## 6. Tests

**Request shape (`chat_request_shape.rs`):**
- Role mapping: `[System, Developer, User]` → `system` + `developer` + `user`
  messages in order; `SystemReminder` knob → developer becomes a
  `<system-reminder>`-wrapped user message (byte-compare with the anthropic
  wire's rendering).
- Assistant grouping: `[Text, ToolUse(a), ToolUse(b)]` → ONE assistant message,
  `content` set, `tool_calls` len 2, ids verbatim, arguments stringified
  (parse them back and compare `Value`s).
- Explosion: following User `[ToolResult(a), ToolResult(b)]` → two consecutive
  `role:"tool"` messages with matching `tool_call_id`s, directly after the
  assistant message.
- Thinking blocks (`signature: None`, Responses-envelope, `RedactedThinking`)
  → nothing emitted; request JSON contains no reasoning fields.
- Tools nested: `{type:"function", function:{name, description, parameters}}`,
  `$schema` stripped; freeform spec → `{input: string}` fallback,
  `required:["input"]`.
- `reasoning_effort` mapping incl. `None` → key absent; token param: default
  emits `max_completion_tokens` only, knob flips to `max_tokens` only.
- OpenRouter backend → `provider` prefs present; Native → absent.

**Parse (`chat_parse.rs`, fixtures):**
- tool_calls → `ToolUse` ids verbatim; parallel calls ordered; invalid
  `arguments` → `Value::String(raw)`.
- usage incl. `cached_tokens` + `cache_write_tokens`; null/absent usage → zeros.
- finish_reason table (stop/length/tool_calls/content_filter/function_call/
  unknown/absent — never panics).
- refusal fixture → `Text` + `StopReason::Refusal`.
- OpenRouter `message.reasoning` fixture → leading
  `Thinking{signature: None}`; xAI-style `reasoning_content` likewise.
- Empty `choices` → `Decode`.

**Canned server (`chat_provider.rs`):** happy path (method/path
`/v1/chat/completions`, Bearer header, body invariants); 429→200 retry;
`insufficient_quota` 429 → single attempt, `Quota`.

**Live smoke (`#[ignore]`, manual, OpenRouter):** one multi-turn tool run on
`openai/gpt-5-mini` (tool_calls round-trip, `cached_tokens > 0` on turn 2);
one turn on a non-OpenAI model (e.g. `x-ai/grok-4.5`) proving the LCD claim +
`message.reasoning` capture; one invalid-model error through `classify`.

---

## 7. Dependencies to add

**None.** Everything rides on the Task-12/18 dependency set. `locode-exec`
adds the `openai-chat` variant to the existing `--api-schema` enum.

---

## 8. Proposed ADR/SPEC deltas (apply at implementation time — do NOT edit now)

### 8.1 ADR-0007 — extend the Task-18 amendment (or a sibling dated note)

> **Amendment (Task 17): the Chat Completions wire.** `api_schema =
> "openai-chat"` lands as the third wire: nested function-tool definitions,
> stringified `arguments`, tool results exploded to `role:"tool"` messages,
> `max_completion_tokens` (legacy `max_tokens` knob), top-level
> `reasoning_effort`. Reasoning is capture-only on this wire (no standard
> replay exists in Chat Completions); multi-turn reasoning continuity requires
> `openai-responses` — the chat wire is the deliberate lowest-common-denominator
> / control wire. Freeform tool specs always degrade to the `{input: string}`
> JSON framing here.

### 8.2 ADR-0013 — no change needed

The OpenAI mapping table already specifies every rendering this wire performs
(developer role, tool-result explosion, argument stringification). The plan
implements the table; cite it, don't amend it. (If the
`ChatDeveloperRendering::SystemReminder` fallback ships, add one line to the
Developer row noting the chat-wire fallback mirrors the Anthropic rendering.)

### 8.3 SPEC.md

Same rows as Task 18 §8.4 (wire list); additionally the Commands section's
example gains nothing (flag values are self-documenting via clap).

---

## 9. Open questions (for user sign-off)

1. **System role knob?** v0 sends `role:"system"` always (broadest; servers
   alias it for o-series). Add a `SystemRole::{System, Developer}` knob now,
   or only when a gateway actually demands `developer`? Proposal: no knob —
   YAGNI until observed.
2. **Refusal normalization** (§4.5): refusal text lands in `final_message` AND
   stop = `Refusal`. Alternative: suppress the text, engine reports
   error-shaped. Proposal as written — confirm.
3. **OpenRouter `reasoning` object.** OpenRouter's unified `reasoning:
   {effort|max_tokens}` param is richer than OpenAI's `reasoning_effort` and
   works across vendors. v0 sends only the OpenAI-standard param; OpenRouter
   translates it for OpenAI models but non-OpenAI models may ignore it. Accept
   for v0 (LCD contract), or add an OpenRouter-conditional `reasoning` body
   field? Proposal: accept for v0; the config already has `provider_prefs` as
   the OpenRouter-shaped escape hatch, and xAI reasoning users belong on
   Task 18's wire.
4. **Live-smoke model choice** for the non-OpenAI leg (grok-4.5 vs a cheaper
   OSS model) — whatever is in your OpenRouter allowlist; name it and I'll pin
   the smoke script.
