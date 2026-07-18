# Task 5 — `locode-provider`: the `Provider` trait, normalized `Completion`, `ProviderError`, `MockProvider`, `ToolCallAssembler` (+ `repair_pairing`)

**Retrospective plan.** This crate is already implemented and merged; `repair_pairing`
landed alongside Task 6 but *lives here* (a provider-layer concern per ADR-0004). This is the
pre-implementation plan we skipped, written after the fact to reflect **what is actually built
and why**, grounded in the merged source and the four studied harnesses. It does not propose
changes.

Source of truth: `SPEC.md`, ADR-0007 (provider trait / two-tier retry / wire-first), ADR-0004
(error taxonomy + pairing), ADR-0013 (conversation protocol / no `system` field), `tasks/todo.md`
Task 5. Every non-obvious decision is grounded in the studied harnesses with `file:line`
citations. **Grok Build is the primary model** for the request/response normalization split.

Submodule roots (abbreviated in citations):
- `grok` = `~/dev/coding-cli-survey/submodules/grok-build/crates/codegen/xai-grok-sampling-types/src`
- `codex` = `~/dev/coding-cli-survey/submodules/codex/codex-rs`
- survey = `~/dev/coding-cli-survey/survey`

Merged crate under review: `crates/locode-provider/src/{provider,request,completion,mock,assemble,repair,lib}.rs`.

---

## 1. Purpose & scope

Provide the **sampling seam** the engine talks to: one API-agnostic `ConversationRequest` in,
one normalized `Completion` out, behind a `Provider` trait — plus the taxonomy (`ProviderError` +
`retryable()`), the zero-spend `MockProvider` double that unblocks the whole loop (Checkpoint B),
the streaming `ToolCallAssembler` the future wire drops in, and the pre-send transcript-hygiene
pass (`repair_pairing`). **No live wire ships here** — the Anthropic Messages wire is Task 12
(ADR-0007). This crate is the contract; wires are additive impls of it.

The crate is deliberately network-free and provider-agnostic: it defines *shapes and
classification*, not HTTP. The one behavioral primitive that runs today is `MockProvider`
(scripted replay) + the two pure helpers (`ToolCallAssembler`, `repair_pairing`).

Per the SPEC dependency graph, `provider → protocol` only — it must **not** depend on
`locode-tools` (`ToolSpec` was hoisted into `locode-protocol` precisely to keep `provider ↛ tools`,
todo Task 5 note; `crates/locode-protocol/src/lib.rs:237`).

### In scope (v0, as built)
- **`Provider` trait** (`provider.rs`): `api_schema() -> &str` + `async complete(&ConversationRequest) -> Result<Completion, ProviderError>`. Object-safe (`async_trait`), `Send + Sync`.
- **`ConversationRequest`** (`request.rs`): `{ messages, tools: Vec<ToolSpec>, sampling_args, cache_hint }` — **no `system` field** (ADR-0013: the wire hoists leading System messages).
- **`SamplingArgs`** (`request.rs`): the neutral **common core only** — `{ max_tokens, temperature: Option, top_p: Option, reasoning_effort: Option }` + `ReasoningEffort{Minimal,Low,Medium,High}` + `CacheHint{Off,Standard}`.
- **`Completion`** (`completion.rs`): normalized `{ content: Vec<ContentBlock>, usage: Usage, stop: StopReason }` — an ordered block list (preserving `Thinking{signature}`), **not** split text+tool_calls. Convenience: `tool_uses()`, `has_tool_calls()`, `text()`.
- **`StopReason`** (`completion.rs`): `#[non_exhaustive]` with an `Unknown(String)` catch-all (open wire enum).
- **`ProviderError`** (`provider.rs`): **exhaustive** taxonomy + `retryable() -> bool` matched with no wildcard.
- **`MockProvider`** (`mock.rs`): scripted `VecDeque<Result<Completion, ProviderError>>`; **panics** on over-consumption. A real, shippable `--provider mock` mode, not test-only.
- **`ToolCallAssembler`** (`assemble.rs`): accumulate partial-JSON args as a raw string per content-block **index**, parse **once** at `finish()` → `Vec<ContentBlock::ToolUse>`. Built + tested standalone though v0 is non-streaming.
- **`repair_pairing(&mut Vec<Message>) -> RepairStats`** (`repair.rs`): dedup duplicate `tool_result`s then synthesize `is_error` results for dangling `tool_use`s. Idempotent.

### Out of scope / deferred (reserved seams, not built here)
- **Any live wire / HTTP.** Anthropic Messages is Task 12 (ADR-0007); `SamplingArgs` per-wire supersets, `cache_control` placement, and the raw serde mirrors all live there. The crate ships only `mock`.
- **The per-model gateway config record** `{ base_url, api_backend, extra_headers, auth/env_key }`. ADR-0007 names it (Grok/Codex shape) but nothing here builds it; `api_schema()` returns only the schema id, not an endpoint. See §8.
- **Transport-tier retry** (backoff+jitter, `Retry-After` honoring, WS→HTTPS fallback, 401 refresh-and-resubmit). Wire's job (Task 12). This crate fixes only the **taxonomy + `retryable()` classifier**; even the bounded loop-level resample lives in the *engine* (Task 6), not here (todo Task 5 note; ADR-0007).
- **Streaming end-to-end.** `ToolCallAssembler` is the streaming primitive, but there is no SSE reader, no `content_block_start/delta/stop` event loop, no idle-timeout — those are the wire's. v0 is non-streaming (ADR-0007; SPEC §37).
- **`tool_choice` / parallel-tool flags / `response_format` (structured output).** `ConversationRequest` carries none of these; Grok's request has `tool_choice` and `json_schema` (`grok conversation.rs:524,547`). Deferred (SPEC Open Q3, envelope-only). See §8.
- **Usage normalization nuance / cost.** `Completion.usage` is `locode_protocol::Usage` (4 fields); no cost table, no client-side token estimate (`bytes/4`, survey `sampling-comparison.md:79`). Cost is a documented TODO (ADR-0014).
- **Client-side token estimation, image stripping, 413 recovery, encrypted-content/model-family guards.** All Grok wire recovery strategies (`grok conversation.rs:557` `strip_images`, `error.rs` `is_encrypted_content_error`). Not modeled.

---

## 2. Module layout (`crates/locode-provider/src/`, as built)

```
lib.rs         Crate docs + module wiring + public re-exports; the crate's inline test module.
provider.rs    `Provider` trait (api_schema + complete) + `ProviderError` (exhaustive) + retryable().
request.rs     `ConversationRequest` + `SamplingArgs` + `ReasoningEffort` + `CacheHint`.
completion.rs  `Completion` (Vec<ContentBlock> + usage + stop) + helpers + `StopReason` (#[non_exhaustive]).
mock.rs        `MockProvider` — scripted VecDeque, panics on exhaustion.
assemble.rs    `ToolCallAssembler` + `AssembleError` (streaming partial-JSON accumulation).
repair.rs      `repair_pairing` + `RepairStats` (+ private dedup/dangling passes) + inline tests.
```

Public surface, from `lib.rs:18-23`:

```rust
pub use assemble::{AssembleError, ToolCallAssembler};
pub use completion::{Completion, StopReason};
pub use mock::MockProvider;
pub use provider::{Provider, ProviderError};
pub use repair::{RepairStats, repair_pairing};
pub use request::{CacheHint, ConversationRequest, ReasoningEffort, SamplingArgs};
```

**Test placement (as built).** All non-`repair` tests are a single inline `#[cfg(test)] mod tests`
in `lib.rs` (11 tests: mock ordering/errors/panic, `api_schema`, `retryable()` classification,
thinking preservation, and the five assembler tests). `repair.rs` carries its own four inline
tests. There is no `tests/` integration directory — the crate has no I/O to integration-test; the
loop-level integration lives in `locode-engine` (Task 6).

Cargo deps (§7): `locode-protocol`, `async-trait`, `thiserror`, `serde_json`; `tokio` (dev, for
`#[tokio::test]`). No `locode-tools`, no `tokio`-runtime dependency in the lib itself.

---

## 3. Key types & signatures (actual, quoted)

### 3.1 `Provider` + `ProviderError` (`provider.rs`)

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    /// The wire-schema id this provider speaks — e.g. `"anthropic"`, `"mock"`.
    fn api_schema(&self) -> &str;

    async fn complete(&self, request: &ConversationRequest) -> Result<Completion, ProviderError>;
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("transport error: {0}")]              Transport(String),
    #[error("rate limited")]                      RateLimited { retry_after: Option<Duration> },
    #[error("api error (status {status}): {message}")]
                                                  Api { status: u16, message: String },
    #[error("context window exceeded")]           ContextOverflow,
    #[error("quota exceeded")]                     Quota,
    #[error("authentication error: {0}")]          Auth(String),
    #[error("failed to decode provider response: {0}")] Decode(String),
}

impl ProviderError {
    #[must_use]
    pub fn retryable(&self) -> bool {
        match self {
            ProviderError::Transport(_) | ProviderError::RateLimited { .. } => true,
            ProviderError::Api { status, .. } => matches!(status, 500 | 502 | 503 | 504 | 520 | 529),
            ProviderError::ContextOverflow
            | ProviderError::Quota
            | ProviderError::Auth(_)
            | ProviderError::Decode(_) => false,
        }
    }
}
```

Note the **exhaustive** match — no `_ =>` — so a new variant cannot be added without classifying
it (the invariant §5.4 is built to preserve).

### 3.2 `ConversationRequest` + `SamplingArgs` (`request.rs`)

```rust
#[derive(Debug, Clone)]
pub struct ConversationRequest {
    pub messages: Vec<Message>,      // full role-tagged stream (System/Developer/User/Assistant)
    pub tools: Vec<ToolSpec>,        // from locode-protocol; wire maps to its tool format
    pub sampling_args: SamplingArgs,
    pub cache_hint: CacheHint,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SamplingArgs {
    pub max_tokens: u32,
    pub temperature: Option<f32>,     // wire drops it when Anthropic thinking is on (temp must=1)
    pub top_p: Option<f32>,
    pub reasoning_effort: Option<ReasoningEffort>,
}
// Default: max_tokens 4096, everything else None.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort { Minimal, Low, Medium, High }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheHint { Off, #[default] Standard }
```

There is deliberately **no `system: String` field** (ADR-0013): `messages` carries System/Developer
inline and each wire hoists them into its own slot.

### 3.3 `Completion` + `StopReason` (`completion.rs`)

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub content: Vec<ContentBlock>,   // ordered: Text / Thinking{signature} / ToolUse
    pub usage: Usage,
    pub stop: StopReason,
}
impl Completion {
    pub fn tool_uses(&self) -> impl Iterator<Item = &ContentBlock>;  // ToolUse blocks, in order
    #[must_use] pub fn has_tool_calls(&self) -> bool;
    #[must_use] pub fn text(&self) -> Option<String>;               // concat of all Text blocks
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopReason {
    EndTurn, MaxTokens, ToolUse, StopSequence, Refusal, PauseTurn,
    Unknown(String),   // a reason this client does not model, carried verbatim
}
```

`StopReason` has no serde derive: it is *mapped* to `locode_protocol::Status` by the engine, never
serialized (`completion.rs:59-60`).

### 3.4 `MockProvider` (`mock.rs`)

```rust
pub struct MockProvider { script: Mutex<VecDeque<Result<Completion, ProviderError>>> }

impl MockProvider {
    #[must_use] pub fn new(script: Vec<Completion>) -> Self;                          // all-Ok
    #[must_use] pub fn with_results(script: Vec<Result<Completion, ProviderError>>) -> Self; // inject errors
}
// api_schema() == "mock". complete() pops the front; panics if the script is exhausted.
```

### 3.5 `ToolCallAssembler` + `AssembleError` (`assemble.rs`)

```rust
#[derive(Debug, Default)]
pub struct ToolCallAssembler { partials: BTreeMap<usize, Partial> }  // keyed by content-block index

pub fn new() -> Self;
pub fn begin(&mut self, index: usize, id: impl Into<String>, name: impl Into<String>);
pub fn push_json(&mut self, index: usize, fragment: &str) -> Result<(), AssembleError>;  // never parses
pub fn finish(self) -> Result<Vec<ContentBlock>, AssembleError>;                         // parse once, index order

#[derive(Debug, Error)]
pub enum AssembleError {
    #[error("no tool call started at content-block index {0}")] MissingStart(usize),
    #[error("invalid tool-call arguments JSON at index {index}: {source}")]
    InvalidJson { index: usize, source: serde_json::Error },
}
```

### 3.6 `repair_pairing` + `RepairStats` (`repair.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RepairStats { pub synthesized: usize, pub deduped: usize }
impl RepairStats { #[must_use] pub fn is_noop(self) -> bool; }

/// Dedup duplicate tool_results (keep last per id), then synthesize is_error results
/// for dangling tool_use blocks. Idempotent.
pub fn repair_pairing(messages: &mut Vec<Message>) -> RepairStats;
```

---

## 4. Behavior & algorithms (as built) + edge cases

### 4.1 `MockProvider::complete`
Pops the front of the `VecDeque` under a `Mutex` (recovering a poisoned lock via
`PoisonError::into_inner` rather than `unwrap`/`expect`, which are denied lints; the mutex only
guards a cursor and is never held across an await — `mock.rs:52-54`). It **ignores the request**
entirely and returns the next scripted `Result` in order. Over-consumption `panic!`s with
`"MockProvider script exhausted: complete() was called more times than scripted"` — a loud signal
that the loop ran an unexpected extra turn (`mock.rs:56-59`).

- **Edge — empty script:** `MockProvider::new(vec![])` is valid; the first `complete()` panics. Used by the `api_schema` test which never calls `complete`.
- **Edge — error injection:** `with_results` lets a test script `Err(RateLimited{..})` etc. to drive the `ModelError` terminal in the engine.
- **Concurrency:** `Mutex<VecDeque>` makes it `Send + Sync` so it satisfies the `Provider` bound and can sit behind `Arc<dyn Provider>` in the engine.

### 4.2 `ToolCallAssembler` — the universal partial-JSON accumulation pattern
`begin(index, id, name)` inserts a `Partial{ id, name, json: String::new() }` into a `BTreeMap`
keyed by the content-block index. `push_json(index, fragment)` **appends the raw fragment** to that
buffer and **never parses** — a single delta is not valid JSON (survey `sampling-comparison.md:27`).
`finish()` consumes the map (already sorted by index → stream order) and parses each buffer **once**:

- Empty/whitespace buffer → `Value::Object({})` — Anthropic sends empty input for a no-argument tool call (`assemble.rs:91-93`).
- Non-empty buffer → `serde_json::from_str`, mapping a parse failure to `AssembleError::InvalidJson{index, source}`.
- Each becomes `ContentBlock::ToolUse{ id, name, input }`.

Edge cases (all tested): `push_json` to an index with no `begin` → `MissingStart(index)`; buffer
that never becomes valid JSON → `InvalidJson`; out-of-order `begin(1,..); begin(0,..)` still yields
index order at `finish` (BTreeMap guarantee).

### 4.3 `repair_pairing` — pre-send transcript hygiene (dedup then dangling)
`repair_pairing` runs **dedup first, then dangling-synthesis** (`repair.rs:44-51`). Order matters:
dedup can remove blocks and even empty a message; doing it first means the dangling pass sees the
final answered-id set.

**`dedup_duplicate_results` (`repair.rs:55-94`).** Builds a `HashMap<&str tool_use_id, (msg_idx,
block_idx)>` recording the **last** position of each id (the "winner" — Grok's keep-the-last
semantics). Cheap early exit: if the number of distinct ids equals the total `ToolResult` count,
nothing repeats → return 0. Otherwise `retain` each message's blocks, dropping any `ToolResult`
whose `(mi, bi)` is not the winner, counting removals. Finally `messages.retain(|m|
!m.content.is_empty())` drops any message emptied by dedup (the wire would reject an empty message).

**`repair_dangling` (`repair.rs:98-155`).** Collects the set of *answered* ids from every
`ToolResult` in the whole transcript. Then walks messages; for each `Assistant` message, gathers its
`ToolUse` ids not in `answered`. For each such dangling id, synthesizes a
`ToolResult{ tool_use_id, content:[Text(DANGLING_RESULT_TEXT)], is_error:true }`. Placement follows
Anthropic shape: if the **immediately following** message is a `User` turn, the synthesized results
are **prepended** to its content (dangling results go first, then the existing results); otherwise a
fresh `User` message is inserted at `i+1`.

`RepairStats{ synthesized, deduped }` reports what changed; `is_noop()` is true when both are zero.

Edge cases (tested in `repair.rs`): dangling tail `tool_use` → one synthesized `is_error` result in
a new trailing `User` message; two results for one id → dedup keeps the one whose text is `"second"`;
a fully-paired transcript is returned byte-identical (`assert_eq!(messages, before)`); a second pass
is a no-op (idempotent).

**Structural adaptation from Grok.** Grok's history is a *flat* `Vec<ConversationItem>` with
`Assistant` and `ToolResult` as sibling items, and its dedup/dangling passes scan the
**immediately-following run** of `ToolResult` items (`grok conversation.rs:2807-2811, 2929-2938`).
Ours **nests**: `ToolUse` inside an `Assistant` `Message`, `ToolResult` inside the following `User`
`Message`(s). So the port scans messages and inner content blocks rather than a flat item list, and
the "answered" set is gathered transcript-wide (our `repair_dangling` collects *all* answered ids
up front, `repair.rs:99-106`) rather than only from the adjacent run — a slightly stronger match that
tolerates results split across messages.

### 4.4 `retryable()` — the classifier that *is* the deliverable
Task 5 fixes only the taxonomy and its `retryable()`; no retry *loop* lives here. The classifier
(`provider.rs:82-93`) returns:

| Variant | `retryable()` | Rationale (cite) |
|---|---|---|
| `Transport(_)` | **true** | connection reset / DNS / TLS is transient (codex `is_retryable` treats `ConnectionFailed`/`Io` retryable, `codex protocol/src/error.rs:200-207`) |
| `RateLimited{..}` | **true** | a bounded retry honoring `retry_after` is legitimate, but surfaced not hammered (grok `is_retryable` includes 429, `grok error.rs:245`; survey `sampling-comparison.md:49`) |
| `Api{status}` | **true iff** 500/502/503/504/520/529 | matches grok's `429|500|502|503|504|520` (`grok error.rs:245`) plus Anthropic's `529 overloaded` (survey `sampling-comparison.md:44`) |
| `ContextOverflow` | **false** | deterministic; replaying the same payload can't help (codex `ContextWindowExceeded → false`, `codex error.rs:190`; grok `is_context_length_error`) |
| `Quota` | **false** | terminal until the user acts (codex `QuotaExceeded`/`UsageLimitReached → false`, `codex error.rs:194-195`) |
| `Auth(_)` | **false** | refresh-once is the *wire's* job before surfacing this; once surfaced it's terminal (grok `is_retryable` Auth→false, `grok error.rs:242`) |
| `Decode(_)` | **false** | a parse failure won't fix itself on resend (grok `Serialization → false`, `grok error.rs:246`) |

The engine's bounded loop-level resample keys off exactly this boolean (Task 6); the wire's
transport tier keys off it too (Task 12).

---

## 5. Design decisions (each: harness `file:line` · why · why-not · how harnesses differ)

### 5.1 `Provider` = one **wire schema**, not a gateway; `api_schema()` not `name()`
- **Source.** Grok separates the **wire protocol** — `ApiBackend { ChatCompletions, Responses, Messages }` (`grok types.rs:1013-1020`) — from an un-enumerated `base_url`/auth per model; `build_messages_request` (`grok conversation.rs:2973`) is one schema's builder. Codex's `WireApi { Responses }` (`codex model-provider-info/src/lib.rs:57`) is likewise the *protocol*, while `ModelProviderInfo { base_url, env_key, http_headers, wire_api, … }` (`codex …lib.rs:89-135`) is the *gateway config* pointed at it. Survey `sampling-comparison.md:59,62`: "per-model `{ base_url, api_backend, extra_headers }` record that also switches wire protocol."
- **Why.** There are ~3 real schemas (Anthropic Messages, OpenAI Chat, OpenAI Responses) + `mock`. Gateways (OpenRouter, Bedrock, a proxy, a local model, Vertex) are **configuration** aimed at one of those schemas — not separate `Provider` impls. Making `Provider` == wire schema keeps "point model X at OpenRouter" a config change, not a new file. `api_schema()` (renamed from a generic `name()`) names the *protocol shape* and is what stamps the report's `provider` field.
- **Why not "provider = endpoint/gateway".** That is the SDK-breadth model (OpenCode's ~25 providers via `@ai-sdk/*`, survey `sampling-comparison.md:10`) and produces per-provider special-case sprawl; even OpenCode is walking it back to a native client (ADR-0007 "Delegate to an SDK — Rejected").
- **Harness diff.** Grok = 3 wires behind one gateway; Codex = 1 wire (Responses only, Chat removed — `codex …lib.rs:80` deserialize error) across many endpoints; Claude = 1 wire (Anthropic) across first-party/Bedrock/Vertex; we = 1 trait, ~3 future schemas, gateways as config.

### 5.2 `Completion` = ordered `Vec<ContentBlock>`, not split `text` + `tool_calls`
- **Source.** Grok's `ConversationResponse.items` is "a flat ordered list … interleaved `Reasoning`, `BackendToolCall`, and a single trailing `Assistant`" (`grok conversation.rs:686-707`). Survey `sampling-comparison.md:71`: reasoning "must keep reasoning contiguous with the following tool_use for trajectory integrity"; codex replays `encrypted_content`.
- **Why.** `Completion` is the **normalized response**, not any wire's raw shape (Anthropic interleaves content blocks; OpenAI splits `content` + `tool_calls`). An ordered `Vec<ContentBlock>` preserves Text/Thinking/ToolUse **order** and keeps `Thinking{signature}` intact, so the engine appends `completion.content` straight into an assistant `Message` (ADR-0013) and replays the exact block order + opaque signature on the next request. **Thinking is not deferred** — the type carries it from day one (tested, `lib.rs:135-156`).
- **Why not split fields.** A `{ text, tool_calls }` shape loses interleaving and drops thinking signatures, breaking extended-thinking replay and prompt-cache continuity (survey `sampling-comparison.md:71`; the engine plan §5.5).
- **Harness diff.** Grok normalizes to a flat item list; Anthropic's raw wire is already block-ordered; OpenAI's raw wire is split and must be re-interleaved by the wire into our shape. We match Grok's normalized-list posture.

### 5.3 `SamplingArgs` = neutral **common core** only; per-wire extras live in the wire
- **Source.** Grok layers a neutral `ReasoningEffort` enum with per-backend mappings —
  `to_responses_api()` → OpenAI effort, `to_messages_api()` → Anthropic `output_config.effort`
  (`grok types.rs:776-822`) — while wire-specific params ride each wire's request builder
  (`grok conversation.rs:2973` `build_messages_request` for Anthropic; the Responses builder for OpenAI).
- **Why.** Keeping `ConversationRequest` API-agnostic means the neutral type holds only what every
  wire understands: `max_tokens`, optional `temperature`/`top_p`, and a neutral `reasoning_effort`.
  Anthropic `top_k`/`stop_sequences`/thinking `budget_tokens`, OpenAI `frequency_penalty`/
  `presence_penalty` are each wire's concern (Task 12). `temperature` is `Option` specifically so a
  wire can drop it (Anthropic requires temp=1 when thinking is on — `request.rs:33-35`, survey
  `sampling-comparison.md:22`).
- **Why not a grand superset.** A superset struct carrying every provider's knobs would leak wire
  detail into the neutral request and rot as providers add params; the neutral-core-plus-per-wire-
  superset split is exactly Grok's (`grok conversation.rs:516` request has only common fields +
  tracing; wire builders add the rest).
- **Harness diff.** Grok's `ReasoningEffort` has `{None, Minimal, Low, Medium(default), High, Xhigh}`
  (`grok types.rs:765-774`); ours is `{Minimal, Low, Medium, High}` — a trimmed neutral set (no
  `None` because absence is `Option::None`; no `Xhigh` because no v0 wire needs it — Grok maps
  `Xhigh → "max"` for Anthropic, `types.rs:818`). Confirm the set in §8.

### 5.4 `ProviderError` **exhaustive** + `retryable()`; `StopReason` `#[non_exhaustive]` + `Unknown`
- **Source.** Grok's `SamplingError::is_retryable` matches every variant with no wildcard
  (`grok error.rs:240-262`); Codex's `CodexErr::is_retryable` likewise (`codex protocol/src/error.rs:176-214`).
  Both keep **distinct terminal variants** (grok context-length via `is_context_length_error`; codex
  `ContextWindowExceeded`/`QuotaExceeded`/`UsageLimitReached`/`ServerOverloaded` all `false`,
  `codex error.rs:190-201`). Conversely Grok's response `StopReason` is a small **closed** enum
  `{Stop, Length, ToolCalls, ContentFilter}` (`grok conversation.rs:606-615`) — but that is Grok's
  own normalized value; a *client* consuming arbitrary servers wants forward-compat.
- **Why (error exhaustive).** Errors drive control flow (retry vs terminal, exit code). An
  exhaustive match means **a new error variant cannot be added without classifying it** — the
  compiler forces the decision. Distinct terminal variants (`ContextOverflow`, `Quota`, `Auth`)
  follow Codex; the general `Api{status}` is the escape hatch for unclassified HTTP statuses.
- **Why (`StopReason` open).** A provider can return a stop reason we don't model; making it
  `#[non_exhaustive]` with `Unknown(String)` means an unknown value never fails the parse — codex's
  forward-compat posture. The engine keys the continue-vs-Completed decision off **presence of
  ToolUse blocks**, not off `stop`, so an `Unknown` is harmless.
- **Why not the inverse.** A `#[non_exhaustive]` error would let a wire add an unclassified variant
  that silently falls through a wildcard to "terminal" (or "retryable") — exactly the bug the
  exhaustive match prevents. A closed `StopReason` would make a new server value a hard parse error.
- **Harness diff.** Grok/Codex both hand-write exhaustive `is_retryable`; Grok additionally exposes
  `retry_after()` and `should_retry_header()` (`grok error.rs:265-280`) that our `RateLimited{ retry_after }`
  field anticipates but the wire (not this crate) will populate.

### 5.5 `retryable()` classification specifics
Covered as behavior in §4.4. The design point: the **boolean is the contract**, computed here once
and consumed by both the wire's transport tier and the engine's resample tier. `429/quota/context/
auth/decode` are the terminal set (grok `error.rs:240`, codex `error.rs:176`); transient `5xx/520/
529/transport/rate-limit` are retryable. `429` being *retryable-but-surfaced* (not silently
hammered) is a survey finding (`sampling-comparison.md:49`) that the `RateLimited{ retry_after }`
shape supports without this crate implementing the wait.

### 5.6 `MockProvider` panics on over-consumption (loud, not silent)
- **Source.** No single harness "mock" to cite; this is a test-double design choice. The nearest
  principle: the studied loops treat an unexpected extra sample as a bug, and Codex's whole retry
  machinery assumes a bounded, scripted turn count (`codex responses_retry.rs:22`).
- **Why.** A scripted mock that ran *out* and returned a default `Completion` would mask the most
  common loop bug: an extra sample the test didn't expect. Panicking turns "the loop looped once too
  many" into a hard, located failure. The `#[should_panic(expected = "script exhausted")]` test
  (`lib.rs:97-104`) pins the contract.
- **Why not return an error / a sentinel.** An `Err` would be *scriptable* behavior (indistinguishable
  from an injected provider error); a sentinel `Completion` would silently continue. Panic is the only
  option that is unmistakably "test harness misuse," not "provider behavior."
- **Harness diff.** N/A (test seam); the design mirrors the SPEC's "highest-leverage surface … zero
  API spend … the loop is where the subtle bugs live" (`SPEC.md:115`).

### 5.7 `ToolCallAssembler` built now though v0 is non-streaming
- **Source.** The **universal** streaming pattern (survey `sampling-comparison.md:25-36`): Claude
  `input_json_delta` → `contentBlock.input += partial_json`; Codex `ToolCallInputDelta` →
  `ToolArgumentDiffConsumer`; Grok `InputJsonDelta` → `BlockState.args_acc`; OpenCode `tool-input-delta`.
  All accumulate a **raw string per content-block index** and parse once at block close.
- **Why build it in Task 5.** It is a pure, wire-agnostic helper with a crisp contract (index →
  buffer → parse-at-finish) that the future Anthropic streaming wire "drops straight in" (`assemble.rs:7-8`).
  Building + unit-testing it standalone now (five tests) de-risks the wire and documents the invariant
  while it's fresh, at near-zero cost.
- **Why not defer to Task 12.** It has no dependency on any wire and is the single most-reused,
  most-error-prone streaming primitive; landing it early with tests is cheaper than reconstructing it
  under wire pressure.
- **Harness diff.** All four accumulate identically; ours differs only in being a reusable typed
  helper (`BTreeMap<usize, Partial>`) rather than inline stream state.

### 5.8 `repair_pairing` lives in `locode-provider`, not `locode-protocol` or `locode-engine`
- **Source.** Grok exposes reusable `repair_dangling_tool_calls` (`grok conversation.rs:2784`) +
  `dedup_duplicate_tool_results` (`grok conversation.rs:2911`) + `has_dangling_tool_calls`
  (`grok conversation.rs:2854`), and runs them **before every request** (survey
  `sampling-comparison.md:83`). ADR-0004: pairing is a **wire-format** invariant, enforced as "a
  single function the provider layer calls unconditionally (before every send)."
- **Why here.** It is a *provider-layer* concern: the engine (which depends on `locode-provider`)
  calls it before each sample, and each future wire calls it before serializing (Task 12) — both can
  reach `locode-provider`. `locode-protocol` is types-only (no logic); putting behavior there would
  break that crate's role. The `repair.rs` module doc states exactly this rationale (`repair.rs:6-10`).
  During Task 6 planning the alternative (host it in `locode-protocol` so a wire could call it without
  depending on `engine`) was raised; the merged decision put it in `locode-provider`, which every
  wire and the engine already depend on, satisfying both consumers (todo Task 6 note, `tasks/todo.md:138`).
- **Why not in the engine.** A wire must run the pass before serializing even when driven outside the
  engine; hosting it in the engine would force `provider → engine` (a dependency cycle).
- **Structural adaptation & "keep the last" semantics** are covered in §4.3. Our dedup keeps the
  **last** result per id (Grok's rule, `grok conversation.rs:2920` "only the last occurrence is kept
  (the real result)"). Grok additionally distinguishes `DanglingToolCallReason{UserCancelled,
  HarnessHalted{class}}` for the synthetic text (`grok conversation.rs:2754-2765,2885`); ours uses a
  single constant `DANGLING_RESULT_TEXT` (`repair.rs:38`) because the headless core has no user-cancel
  path (ADR-0001) — the mid-batch-abort wording is the engine's concern, not this pass's.
- **Harness diff.** Grok = reusable explicit repair+dedup helpers (what we port); Codex records
  outputs in an ordered `drain_in_flight` and writes one `TurnAborted` marker instead of per-call
  synthesis; Claude synthesizes missing `tool_result`s on mid-stream abort inside the loop. We follow
  Grok's explicit-synthesis model — cleanest fit for a paired-by-id transcript and exactly what
  ADR-0004 cites.

### 5.9 No `system` field on `ConversationRequest`
- **Source.** Grok's `build_messages_request` builds `system_blocks` separately from `messages` by
  hoisting System items out of the flat `items` stream (`grok conversation.rs:2977-2983`); Grok's
  request itself carries only `items` (no dedicated `system`, `grok conversation.rs:516-548`).
- **Why.** ADR-0013 fixes a 4-role conversation where System/Developer are ordinary role-tagged
  messages in the one stream. Keeping `messages` as the single source and letting each wire hoist
  leading System messages into its top-level slot (Anthropic's `system`) avoids a redundant field the
  engine would have to keep in sync (`request.rs:8-10`).
- **Why not a `system: String`.** Two sources of truth for the same content; and Developer messages
  (a distinct role) don't fit a single `system` string.
- **Harness diff.** Anthropic's *wire* has a top-level `system`; Grok hoists into it at build time;
  we defer that hoist to the wire and keep the neutral request uniform.

---

## 6. Tests (as built)

**`lib.rs` inline `mod tests` — 11 tests** (`crates/locode-provider/src/lib.rs:25-231`):
1. `mock_emits_scripted_turns_in_order` — script `[tool_call, text]`; first has tool calls + `stop == ToolUse` + one tool_use; second is text-only + `stop == EndTurn`, `text() == "done"`. The realistic tool-then-final loop shape.
2. `mock_can_script_errors` — `with_results([Err(RateLimited{None})])`; asserts the popped error and `retryable() == true`.
3. `mock_panics_when_over_consumed` — `#[should_panic(expected = "script exhausted")]`; one-item script, two calls.
4. `api_schema_is_the_wire_id` — `MockProvider::new(vec![]).api_schema() == "mock"`.
5. `provider_error_classifies_retryable` — `Transport` & `Api{503}` retryable; `Api{400}`, `ContextOverflow`, `Quota`, `Auth` not.
6. `completion_preserves_thinking_blocks` — a `Completion` with `Thinking{signature:Some("sig-abc")}` first, `Text` second; `text() == "answer"` and the Thinking block + signature survive verbatim. Guards §5.2.
7. `assembler_stitches_fragmented_args` — three fragments each invalid alone (`{"comm` / `and":"echo` / ` hi"}`) → one `ToolUse{ input: {"command":"echo hi"} }`.
8. `assembler_empty_input_becomes_empty_object` — `begin` then no `push_json` → `input == {}`.
9. `assembler_preserves_index_order` — `begin(1,..); begin(0,..)` → ids come out `["c1","c2"]`.
10. `assembler_rejects_fragment_without_start` — `push_json(3, …)` → `MissingStart(3)`.
11. `assembler_reports_invalid_json` — `{not json` → `InvalidJson{index:0, ..}`.

**`repair.rs` inline `mod tests` — 4 tests** (`crates/locode-provider/src/repair.rs:157-261`):
- `dangling_tool_use_gets_synthetic_result` — tail assistant `tool_use` with no result → `synthesized == 1`, trailing `User` message with an `is_error` result for `c1`.
- `duplicate_results_keep_the_last` — two results for `c1` → `deduped == 1`, `synthesized == 0`, survivor's text is `"second"`.
- `valid_transcript_is_unchanged` — paired transcript → `is_noop()` and `messages == before` (byte-identical).
- `repair_is_idempotent` — first pass synthesizes 1, second pass is a no-op.

**Coverage note.** The todo lists "11 unit tests" for Task 5 (the `lib.rs` block); the `repair.rs`
four are counted under Task 6's accounting because `repair_pairing` landed with the engine work.
Together the crate ships 15 tests, all zero-network. No `tests/` integration dir — the crate has no
I/O; end-to-end coverage of the sampling seam is the engine's `MockProvider`-driven loop matrix
(Task 6). The mandatory triangle (`cargo fmt`/`clippy -D warnings`/`test`) passes.

---

## 7. Dependencies

No new external crates beyond the already-vendored stack (no "Ask first" trigger). Actual
`crates/locode-provider/Cargo.toml` deps:

| Dep | Why |
|---|---|
| `locode-protocol` | `Message`, `ContentBlock`, `ResultChunk`, `Role`, `Usage` (+ `AddAssign`), `ToolSpec`. The **only** workspace dep — enforces `provider ↛ tools`. |
| `async-trait` | `Provider::complete` is `async fn` in a trait object → `#[async_trait]` (same as `locode-tools::DynTool`). |
| `thiserror` | `#[derive(Error)]` for `ProviderError` and `AssembleError`. |
| `serde_json` | `ToolCallAssembler` parses accumulated arg buffers into `serde_json::Value` for `ContentBlock::ToolUse.input`. |
| `tokio` (dev) | `#[tokio::test]` for the async `MockProvider` tests. Not a lib runtime dep — the crate performs no async work of its own. |

`ToolSpec` was **moved** into `locode-protocol` (Task 5, `crates/locode-protocol/src/lib.rs:237`) so
`locode-tools` builds it and `locode-provider` consumes it without a `provider → tools` edge.

---

## 8. Open questions / concerns / future considerations (exhaustive, honest)

These are the seams the merged code deliberately leaves open, plus the genuine risks in what shipped.

1. **The per-model gateway config record is not built.** ADR-0007 names `{ base_url, api_backend,
   extra_headers, env_key/auth }` (Grok/Codex shape, `codex model-provider-info/src/lib.rs:89-135`)
   as the way to point a schema at OpenRouter/Bedrock/a proxy. Today `api_schema()` returns only the
   schema id — there is **no** endpoint, header, or auth plumbing anywhere in the crate. When the wire
   lands (Task 12) this record must appear; open question whether it lives in `locode-provider`, a new
   `locode-config`, or is passed in by `locode-exec`. Note the survey warning: overriding `base_url`
   often changes the auth header (`x-api-key` → `Authorization: Bearer`), so auth must ride the record,
   not a global constant (survey `sampling-comparison.md:62`).

2. **`SamplingArgs` per-wire extras + a passthrough bag.** The neutral core has four fields. Anthropic
   `top_k`, `stop_sequences`, thinking `budget_tokens`; OpenAI `frequency_penalty`, `presence_penalty`,
   `seed`, `logit_bias` have nowhere to go yet. Decision deferred to the wire, but there's an open
   design question: do wires read these from a typed per-wire superset (Grok's approach), or does
   `ConversationRequest` grow an `extra: Map<String, Value>` passthrough for advanced callers? The
   current answer is "typed per-wire, no passthrough," which is cleaner but less flexible.

3. **`reasoning_effort → budget` mapping is unspecified here.** `ReasoningEffort{Minimal,Low,Medium,High}`
   exists but nothing maps it. Grok maps `to_messages_api`: `None/Minimal → None`, `Low → "low"`,
   `Medium → "medium"`, `High → "high"`, `Xhigh → "max"` (`grok types.rs:812-819`) and `to_responses_api`
   1:1. Our enum drops `None` (absence = `Option::None`) and `Xhigh`. Open: is `{Minimal,Low,Medium,High}`
   the right neutral set, and does Anthropic's `budget_tokens` (a token count, not a level) need a
   numeric mapping table the enum can't express? (Anthropic thinking is *budget*, not *effort*.)

4. **`StopReason` variant completeness / mapping to `Status`.** Our set is Anthropic-leaning
   (`EndTurn, MaxTokens, ToolUse, StopSequence, Refusal, PauseTurn, Unknown`). Grok's normalized set is
   `{Stop, Length, ToolCalls, ContentFilter}` (`grok conversation.rs:606`); OpenAI adds
   `function_call`/`content_filter`/`tool_calls`. Open questions: (a) is `PauseTurn` (server-tool
   round-trip) meaningful without server tools? (b) The engine keys off ToolUse-block *presence*, not
   `stop` — so what, if anything, ever *reads* `stop`? Today nothing does; if it stays unread it's
   dead weight, if it's meant to drive a refusal signal (see #5) that path is unbuilt.

5. **`Refusal` / `ContentFilter` / empty-content turns produce no distinct signal.** A completion with
   `stop == Refusal` and empty content flows through as an ordinary Completed turn (the engine keys off
   tool-use presence). Grok emits a provider-refusal notice chunk (its turn loop); we surface nothing.
   Open: should a refusal map to a distinct `Report` signal, or is Completed-with-empty-text acceptable?

6. **`tool_choice` and parallel-tool control are absent.** `ConversationRequest` cannot say "you must
   call a tool" / "don't call tools" / "call exactly `f`" — Grok has `ConversationToolChoice{Auto, None,
   Required, Function(String)}` (`grok conversation.rs:583-596`), nor "disable parallel tool calls."
   Both matter for structured-output and single-tool-forcing flows. Deferred; will need a
   `ConversationRequest` field (a public-surface change → "Ask first" per Boundaries).

7. **Structured output / `response_format` / `json_schema` is unmodeled.** Grok's request carries
   `json_schema: Option<Value>` for strict-mode structured output (`grok conversation.rs:547`), and
   `ApiBackend::supports_json_schema` gates it (Chat/Responses yes, Messages no — `grok types.rs:1024-1029`).
   SPEC Open Q3 keeps v0 envelope-only. When structured output lands it touches both `ConversationRequest`
   (a schema field) and the report — a coordinated change.

8. **`CacheHint` placement policy is a stub.** `CacheHint{Off, Standard}` is a *reserved seam*; the actual
   `cache_control` breakpoint placement (Anthropic: one marker on the last message + ≤4 on system blocks)
   is the wire's (Task 12, `request.rs:70-73`). Open: is a 2-state hint enough, or do we need per-message
   or per-segment cache markers (Grok attaches `CacheControl` to individual `ContentBlock`s,
   `grok conversation.rs:3006`)? A binary hint may be too coarse for multi-breakpoint caching.

9. **Streaming vs non-streaming is a hard fork the crate only half-anticipates.** `ToolCallAssembler`
   is the streaming primitive, but there is no SSE event model (`content_block_start/delta/stop`,
   `message_delta` for usage/stop), no idle/per-chunk timeout (grok `IdleTimeout`, `grok error.rs:116`),
   and no "stream closed before completion" error (codex `Stream(String, Option<Duration>)`,
   `codex protocol/src/error.rs:82`). When streaming lands, `complete()` may need a streaming sibling or
   the trait may grow a `complete_streaming` — a `Provider` signature change (Ask-first). Also: Claude
   uniquely falls back to a **non-streaming** request when the stream fails (survey `sampling-comparison.md:36`)
   — our trait can't express that fallback today.

10. **Usage is summed, not reconciled; cost is absent.** `Completion.usage` is `locode_protocol::Usage`
    (input/output/cache-read/cache-creation, 4 `u64`s). The engine plain-sums across turns, which
    **over-counts `input_tokens`** because each request re-sends the full history (engine plan §5.9).
    Grok's normalized `TokenUsage` documents the subtlety — `prompt_tokens` is the FULL prompt
    (uncached + cache reads + writes), `cached_prompt_tokens` is only the hit subset, "do not subtract"
    (`grok conversation.rs:640-665`). We have no cost table (`total_cost_usd` is a TODO, ADR-0014) and no
    client-side `bytes/4` estimate (survey `sampling-comparison.md:79`). Open: last-input-plus-summed-output
    vs plain-sum; and whether cache-write (~1.25×) needs separate cost weighting.

11. **`ProviderError` may still be too coarse for the wire's recovery strategies.** Grok's real error
    enum distinguishes payload-too-large/413 (`is_payload_too_large`, drives image-strip recovery),
    encrypted-content/model-family mismatch (`is_encrypted_content_error`, terminal → new session),
    doom-loop (`DoomLoopDetected`, retryable on a separate budget), idle-timeout (non-retryable),
    max-tokens-truncation (`grok error.rs:105-133,180-213`). We collapse all of these into
    `Transport`/`Api{status}`/`Decode`. When the wire needs 413-strip or encrypted-content handling it
    will either add variants (exhaustive-match churn, but that's the point of §5.4) or carry richer detail
    in `Api.message` and re-classify by string — the latter is fragile. Open: which variants graduate to
    first-class?

12. **`RateLimited{ retry_after }` is defined but nothing populates or honors it.** The field exists so
    the wire can carry `Retry-After` (grok `retry_after_secs`, `grok error.rs:100,265`; codex
    `Stream(_, requested_delay)`, `codex responses_retry.rs:50-52`), but this crate neither parses it (no
    wire) nor waits on it (no retry loop). It's a correctly-shaped placeholder; the risk is it stays
    unfilled and the engine's bounded resample ignores server-requested delays until Task 12.

13. **Two-tier retry split leaves a seam, not an implementation.** Task 5 fixes only the taxonomy +
    `retryable()`. The **transport tier** (backoff+jitter, `Retry-After`, WS→HTTPS fallback, 401 refresh)
    is the wire's (`codex responses_retry.rs:22-79`); the **loop-level resample** is the engine's. Neither
    exists in this crate. The concern: the classification boundary (what's retryable) is decided here in
    isolation from the two consumers that will act on it — if the wire needs a *different* notion of
    retryable than the engine (e.g. wire retries idle-timeout, engine doesn't), a single boolean may not
    suffice and we'd need `retryable_transport()` vs `retryable_resample()`.

14. **`api_schema()` returns `&str` tied to `&self`, forcing a stored id on real wires.** The mock uses a
    literal (`#[allow(clippy::unnecessary_literal_bound)]`, `mock.rs:46`). Minor, but real wires must own
    their id string; an associated const or `&'static str` might have been cleaner. Not worth changing.

15. **`repair_pairing`'s "gather all answered ids transcript-wide" is stronger than Grok's adjacent-run
    scan — possibly too strong.** Grok only counts results in the **immediately-following run** as
    answering a call (`grok conversation.rs:2807-2811`); ours counts *any* `ToolResult` with a matching
    id anywhere (`repair.rs:99-106`). This tolerates results split across messages but would *fail to
    synthesize* if a stray result for an id sat far away in the transcript. For our engine-built
    transcripts this never happens, but for arbitrary caller-supplied history it's a subtle semantic
    difference worth noting. Also: dedup uses `HashMap`/`HashSet` (non-deterministic iteration) but writes
    back by sorted `(mi, bi)` winners, so output order is deterministic — verified by the "unchanged"
    test, but worth keeping in mind if the algorithm changes.

16. **`repair_pairing` home could still move.** It lives in `locode-provider` (§5.8) so both engine and
    every wire reach it. If a future wire crate wants to call it *without* depending on all of
    `locode-provider` (e.g. a thin serializer crate), the "types + this one pass" might want to be a
    smaller shared crate. Not a problem today; flagged because the placement was a genuine fork in
    planning (protocol vs provider vs engine).

### Speech-to-text / identifier confirmations
This retrospective worked from written source and ADRs; no spoken identifiers were reconstructed.
One naming fact to confirm rather than a guess: the trait method is `api_schema()` (renamed from an
earlier `name()`), and the report/`Event::Init` field it stamps is `provider` (which names the wire
*schema*, not a gateway) — consistent across `provider.rs`, ADR-0009, and the envelope golden
(todo Task 6 note, `tasks/todo.md:139`).
