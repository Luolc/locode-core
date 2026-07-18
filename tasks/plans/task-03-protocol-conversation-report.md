# Task 3 — `locode-protocol`: 4-role conversation model + report envelope

**Retrospective plan.** This task is **already implemented and merged** (`tasks/todo.md`
Task 3 ✅; commit `df93f54`). This document is the pre-implementation plan we skipped,
written after the fact to record **what is actually built and why**, grounded in the
shipped source and the four studied harnesses. It does not propose changes.

Source of truth: `SPEC.md`, ADR-0013 (conversation protocol), ADR-0009 (headless I/O
contract). Every non-obvious decision is grounded in the studied harnesses with `file:line`
citations. Grok Build's normalized conversation model is the primary reference for how to
unify Anthropic + OpenAI; the empirical wire capture (`claude-code-system-surfaces` note)
is the primary reference for the role model.

As-built code: `crates/locode-protocol/src/lib.rs` (lines 18–244 for this task; 246–312 are
Task 3b). Tests: inline `#[cfg(test)]` (`lib.rs:314–525`) + `tests/envelope_golden.rs` +
`tests/report_snapshot.json`.

Submodule roots (abbreviated in citations):
- `grok` = `~/dev/coding-cli-survey/submodules/grok-build`
- `codex` = `~/dev/coding-cli-survey/submodules/codex`
- survey = `~/dev/coding-cli-survey/survey`
- note = the `claude-code-system-surfaces` memory (proxy capture + Claude Code source)

---

## 1. Purpose & scope

Provide the **pure, provider-neutral, no-I/O types** every other crate shares: (a) the
**conversation model** — a 4-role, Anthropic-shaped, interleaved content-block history that
the loop accumulates and hands to a `Provider` (ADR-0013); (b) the **report envelope** — the
single JSON artifact `locode-exec` prints (ADR-0009), with a frozen `schema_version: 1`; and
(c) the shared **`ToolSpec`**, hoisted here so both `locode-tools` (which builds it) and
`locode-provider` (which consumes it) can depend on it without violating the dep graph.

This crate sits at the root of the dependency DAG (`protocol ← everything`, SPEC §Project
Structure): it must have **no** dependency on `tools`, `provider`, `host`, or `engine`, and
must not perform any wire (de)serialization — each `Provider` impl owns its own mapping onto
a vendor wire (ADR-0013 "each `Provider` wire owns its own (de)serialization"). The only
external deps are `serde` + `serde_json` (§7).

### In scope (v0, as built)
- `Conversation { messages: Vec<Message> }` — one uniform stream, no separate `system` field.
- `Message { role: Role, content: Vec<ContentBlock> }`.
- `Role { System, Developer, User, Assistant }` — semantics, not wire names (ADR-0013).
- `#[non_exhaustive] ContentBlock` with the interleaved Anthropic block shape:
  `Text`, `Image{source}`, `Thinking{text, signature}`, `ToolUse{id,name,input}`,
  `ToolResult{tool_use_id, content, is_error}`.
- `ResultChunk { Text, Image }` (the restricted set a tool result may carry).
- `ImageSource { Base64{media_type,data}, Url{url} }`.
- `Report` envelope (`schema_version:1`, `status`, `harness`, `api_schema`, `final_message`,
  `structured_output`, `turns`, `tool_calls[]`, `usage`, `session_id`, `error`).
- `Status { Completed, MaxTurns, ModelError, Error }` → the exact ADR-0009 strings.
- `ToolCallRecord` (the report-side view of one tool call: `output`, not `prompt_text`).
- `Usage` (4 token counters) + `impl AddAssign` so the engine sums per-turn usage.
- `ToolSpec { name, description, parameters }`.
- Golden freeze of the envelope shape + a lossless serde round-trip covering all four roles
  and tool_use/tool_result pairing.

### Out of scope / deferred (reserved by design, not built here)
- **Wire (de)serialization.** No Anthropic/OpenAI request/response mapping — that is each
  `Provider`'s job (system hoisting, Developer rendering, tool-result exploding, arg
  stringifying all live in the wire; ADR-0013 mapping tables). This crate stays vendor-free.
- **Per-block cache markers / `CacheHint`.** Grok's `ContentBlock::Text`/`ToolResult` carry an
  optional `cache_control` field (`grok …/messages.rs:100–125`); ADR-0013 notes "content blocks
  may carry an optional cache marker". We **deliberately did not** bake `cache_control` into the
  block types — cache breakpoint placement is a wire concern, decided via a `CacheHint` on
  `ConversationRequest` in the Anthropic wire (Task 12; ADR-0007). See §8.
- **`Document` block.** Reserved by ADR-0013 ("reserved: Document {..}"); `#[non_exhaustive]`
  leaves the door open. Not added in v0.
- **`Image` / `Thinking` full wiring.** The variants exist (multimodal + reasoning replay), but
  only `Text`/`ToolUse`/`ToolResult` are exercised end-to-end in v0 (ADR-0013 Conventions; SPEC
  §Assumptions). `Image`/`Thinking` are structurally present and serde-tested by construction
  but not driven through a live loop yet (§8).
- **`structured_output` (`--json-schema`).** The field exists on `Report` (always `None` in v0)
  so the envelope shape is frozen with the slot present; the interception logic is deferred
  (ADR-0009; SPEC Open Q3; ADR-0014). See §8.
- **`total_cost_usd`.** Tokens-only for now (ADR-0014 "stay tokens-only … `total_cost_usd` is a
  TODO"); no cost field on `Report`/`Usage`. See §8.
- **Transcript repair/dedup helpers.** ADR-0004 pairing repair lives in `locode-provider`
  (`repair_pairing`), not here — decided during Task 6 (todo.md Task 6 design notes). This crate
  only defines the *invariant* (`tool_use.id ↔ tool_result.tool_use_id`); enforcement is elsewhere.
- **Durable session persistence (JSONL on disk).** Types serialize with serde, but reading/
  writing session files is deferred (SPEC §Assumptions 6).

---

## 2. Module layout (as built)

Everything lives in a **single file**, `crates/locode-protocol/src/lib.rs`, sectioned by
banner comments. The crate is small enough that splitting into modules would add ceremony
without clarity; the file is organized top-to-bottom as:

```
lib.rs
├── crate docs (//!)                       lib.rs:1–13   ADR links, the "two concerns" framing
├── use serde…; use serde_json::Value      lib.rs:15–16
├── ==== Conversation model ====           lib.rs:18–136 Conversation, Message, Role,
│                                                          ContentBlock, ResultChunk, ImageSource
├── ==== Report envelope ====              lib.rs:138–224 Report, Status, ToolCallRecord,
│                                                          Usage (+ AddAssign)
├── ==== Tool spec ====                    lib.rs:226–244 ToolSpec
├── ==== Streaming events ====             lib.rs:246–312 Event, reconstruct_conversation (Task 3b)
└── #[cfg(test)] mod tests                 lib.rs:314–525
```

Tests: unit-scope inline (`lib.rs:314–525`); the envelope golden is an integration test under
`tests/` (`envelope_golden.rs` + committed `report_snapshot.json`) so the frozen shape is a
file artifact a reviewer sees in the diff, not buried in a `json!` literal.

Cargo deps (§7): `serde` (derive) + `serde_json`. No async, no tokio, no schemars here.

---

## 3. Key types & signatures (the actual shipped types)

Quoted verbatim from `crates/locode-protocol/src/lib.rs`. Attributes shown where they carry
a decision.

### Conversation model (`lib.rs:20–136`)

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub messages: Vec<Message>,          // no separate `system` field (ADR-0013)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,      // ordered, interleaved blocks
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role { System, Developer, User, Assistant }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ContentBlock {
    Text       { text: String },
    Image      { source: ImageSource },
    Thinking   { text: String, signature: Option<String> },
    ToolUse    { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: Vec<ResultChunk>, is_error: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResultChunk { Text { text: String }, Image { source: ImageSource } }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Url    { url: String },
}
```

### Report envelope (`lib.rs:144–224`)

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,               // frozen at 1
    pub status: Status,
    pub harness: String,                   // pack, e.g. "grok"
    pub api_schema: String,                // wire schema, e.g. "anthropic" (renamed from `provider`)
    pub final_message: Option<String>,
    pub structured_output: Option<Value>,  // --json-schema slot (always None in v0)
    pub turns: u32,
    pub tool_calls: Vec<ToolCallRecord>,
    pub usage: Usage,
    pub session_id: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status { Completed, MaxTurns, ModelError, Error }
//                completed  max_turns model_error error   (ADR-0009 strings)

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,        // matches the conversation's tool_use id
    pub name: String,      // client-facing wire name the model called
    pub kind: String,      // canonical ToolKind tag (e.g. "shell") for cross-pack A/B
    pub args: Value,
    pub ok: bool,
    pub output: Value,     // the report view, NOT prompt_text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

impl std::ops::AddAssign for Usage {          // engine sums across turns
    fn add_assign(&mut self, rhs: Self) { /* field-wise += on all four */ }
}
```

### Tool spec (`lib.rs:236–244`)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,           // model-facing wire name (the pack's name)
    pub description: String,
    pub parameters: Value,      // JSON Schema derived from the arg type (schemars, in tools)
}
```

---

## 4. Behavior, serde shape, and golden freeze

The crate has almost no *behavior* — its contract is the **on-wire serde shape**. The shapes
that matter:

- **Content blocks are Anthropic-tagged.** `#[serde(tag = "type", rename_all = "snake_case")]`
  produces `{ "type": "text", "text": "…" }`, `{ "type": "tool_use", "id":…, "name":…,
  "input":… }`, `{ "type": "tool_result", "tool_use_id":…, "content":[…], "is_error":… }`.
  Test `content_block_uses_anthropic_style_type_tags` (`lib.rs:387–393`) pins the `text` case.
  This is our *own* persistence/reconstruction shape — **not** a vendor wire; a `Provider` may
  re-tag or explode blocks on the way out (e.g. OpenAI `tool_result` → separate `tool` messages,
  ADR-0013 mapping).
- **Roles snake_case.** `Role` serializes to `"system" | "developer" | "user" | "assistant"`.
- **Status strings are frozen to ADR-0009.** `status_serializes_to_adr_0009_strings`
  (`lib.rs:396–406`) asserts `Completed→"completed"`, `MaxTurns→"max_turns"`,
  `ModelError→"model_error"`, `Error→"error"`. These are the exit-code-bearing strings a caller
  parses; drifting them is a breaking change.
- **Tool pairing is by id.** `tool_use.id == tool_result.tool_use_id` is *the* correctness
  invariant (ADR-0004). The round-trip test extracts both and asserts equality
  (`lib.rs:376–383`). The type system does not enforce pairing — the loop + `repair_pairing`
  (provider) do — but the field names make the link explicit.
- **`Usage` accumulates.** `AddAssign` is the only non-serde behavior; the engine does
  `acc.usage += completion.usage` each turn (Task 6 §5.9). Field-wise sum; no cost, no
  averaging. `Default` gives an all-zero starting accumulator.
- **The envelope shape is golden-frozen.** `tests/envelope_golden.rs` builds a fixed `Report`,
  serializes it, and asserts byte-structural equality with the committed
  `tests/report_snapshot.json` (a `serde_json::Value` compare, so key *presence/shape* is what
  is pinned, order-independent). A second test asserts `schema_version == 1` literally
  (`envelope_golden.rs:47–49`). **Any structural change to the envelope must deliberately edit
  `report_snapshot.json`** — that diff is the "Ask first" tripwire from SPEC §Boundaries.

The snapshot (`report_snapshot.json`) shows the exact top-level key set and the nested shapes
of `tool_calls[]` (`{id,name,kind,args,ok,output}`) and `usage`
(`{input_tokens,output_tokens,cache_read_tokens,cache_creation_tokens}`), with
`final_message`/`structured_output`/`error` shown as their `null`-able forms.

---

## 5. Design decisions (each: harness `file:line` · why · why-not · harness diff)

### 5.1 Four roles: `System` vs `Developer` (split the overloaded "system")
- **Source.** The empirical wire capture (note; ADR-0013 "Empirical grounding") found a real
  Claude Code request carries **three** distinct "system" surfaces: a static top-level
  `system[]` (cached), a **mid-conversation `role:"system"` message** (beta
  `mid-conversation-system`, carrying client-injected capability context — subagents/skills/
  slash-commands/ToolSearch deltas), and `<system-reminder>` text blocks inside `user` messages.
  OpenAI already separates the two intents as `system` (static) vs **`developer`** (app-author);
  Codex's `ResponseItem::Message { role: String, … }` (`codex …/models.rs:813–817`) keeps role as
  a free string precisely because it juggles `system`/`developer`/`user`/`assistant`. Grok's
  `ConversationItem` (`grok …/conversation.rs:28–59`) has a dedicated `System(SystemItem)` plus a
  `SyntheticReason::SystemReminder` marker (`conversation.rs:82`) for the reminder surface.
- **Why.** ADR-0013: Anthropic overloads "system" for two semantically different things — the
  immutable constitution and dynamically injected instructions. Modeling both as one role forces
  reminder-blocks-only and loses the "who authored this" semantics. We borrow OpenAI's word
  (`Developer`) for the injected one, so **each role carries meaning, not a wire name**. `System`
  maps to Anthropic's *top-level* `system` (leading System messages are hoisted); `Developer`
  maps to Anthropic's *mid-conversation* `role:"system"` beta message (or a portable
  `<system-reminder>` fallback) — the rule ADR-0013 states so we never trip: "an Anthropic
  `role:"system"` message is our `Developer`, not our `System`."
- **Why not** 2-role (system top-level + user/assistant): no first-class mid-conversation
  injected context (rejected, ADR-0013). **Why not** 3-role reusing "system" mid-stream:
  perpetuates the naming collision (rejected). **Why not** the design-doc flat model
  (`System(String)`/`User(String)`/`Assistant{text,tool_calls}`/`Tool{…}`): can't carry image or
  thinking blocks, hides block structure (rejected — this ADR supersedes it).
- **Harness diff.** Grok = one `System` item + a reminder marker; Codex = `role: String` on a
  `Message` item (stringly-typed, all four values); Claude Code = three physical wire surfaces.
  We collapse Claude's three surfaces into two *roles* (System, Developer) and leave the
  `<system-reminder>`-vs-beta-message rendering choice to the wire (a flag, ADR-0013 Consequences).

### 5.2 One uniform message stream, no separate `system` field
- **Source.** Grok's history is a flat `Vec<ConversationItem>` where `System` is just another
  item (`grok …/conversation.rs:28–31`); Codex's is `Vec<ResponseItem>` likewise. Neither keeps
  `system` as a side field on the request object at the history level.
- **Why.** ADR-0013: "There is no separate `system` field — a `Role::System` message *is* the
  base prompt; the Anthropic wire hoists leading System messages into its top-level `system`
  param." A single `Vec<Message>` is the simplest thing that round-trips and reconstructs
  (Task 3b `reconstruct_conversation` just concatenates), and it keeps mid-stream `Developer`
  injection first-class rather than special-cased.
- **Why not** a `Conversation { system: Vec<Block>, messages: Vec<Message> }` split: it would
  privilege the static prompt and make mid-conversation system/developer injection awkward, and
  it does not match how either harness stores history.
- **Harness diff.** Anthropic's *wire request* does have a top-level `system` param — but that is
  a wire projection the `Provider` produces by hoisting, not a shape of our history. We store
  uniformly and project at the wire (ADR-0013 mapping tables).

### 5.3 Anthropic-shaped, interleaved content blocks
- **Source.** Grok's `ContentBlock` (`grok …/messages.rs:100–125`) is exactly this shape —
  `Text`, `Image{source}`, `ToolUse{id,name,input}`, `ToolResult{tool_use_id,content}`,
  `Thinking{thinking,signature}` — `#[serde(tag="type", rename_all="snake_case")]`. Codex models
  content as `Vec<ContentItem>` on a `Message` item (`codex …/models.rs:816`) plus separate
  `Reasoning` items; the interleaving is preserved as an ordered item list.
- **Why.** ADR-0013 + SPEC: the maintainer's target model is Claude (SPEC §Assumptions 2), and
  Anthropic's block model is the richest common denominator — it natively expresses multimodal
  (`Image`), reasoning (`Thinking`), and tool-call/result pairing as *ordered blocks within one
  message*, which is what preserves the `[thinking, text, tool_use]` order the model emitted.
  Mapping *from* this shape to OpenAI is mechanical (explode tool_results into `tool` messages,
  stringify `input` → `arguments`; ADR-0013 OpenAI table); mapping the other direction would be
  lossier. So we model on Anthropic and project to OpenAI, not vice-versa.
- **Why not** a text+tool_calls flat assistant shape (the design doc's original): drops thinking
  and image blocks and hides order (rejected, §5.1).
- **Harness diff.** Grok ≈ identical block enum (we omit its per-block `cache_control`, §5.7);
  Codex uses items + a separate reasoning item rather than an in-message `Thinking` block.

### 5.4 `#[non_exhaustive]` on `ContentBlock` (and later `Event`)
- **Source.** Grok's `StopReason` keeps an `Unknown(String)` untagged catch-all as the last
  variant "so a new server-side value can never fail the terminal parse"
  (`grok …/messages.rs:216–226`); the same defensive posture applies to block kinds that vendors
  keep adding (documents, video, redacted-thinking, server-tool blocks).
- **Why.** ADR-0013 reserves `Document{..}` and calls out that only `Text`/`ToolUse`/`ToolResult`
  are wired in v0. `#[non_exhaustive]` lets us add `Document`, per-token delta support, or new
  multimodal kinds **without a breaking change** to downstream crates, and forces external
  matchers to keep a wildcard arm. It is the additive-evolution seam for the one type most likely
  to grow.
- **Why not** a plain exhaustive enum: adding any block kind later would be a semver-breaking
  change rippling through every `match` in `tools`/`provider`/`engine`.
- **Why not** also `#[non_exhaustive]` on `ResultChunk`/`ImageSource`/`Role`/`Status`: those are
  closed by design. `Role` is a fixed 4 (ADR-0013 fixes exactly four). `Status` is the fixed
  ADR-0009 terminal set (ADR-0014 keeps a *single flat* enum and grows it deliberately with the
  golden test as the gate). `ResultChunk`/`ImageSource` mirror the small closed vendor sets. Note
  the asymmetry with `locode-provider`'s `StopReason`, which *is* `#[non_exhaustive]` +
  `Unknown(String)` (todo.md Task 5) — because stop reasons are read *from* an open server enum,
  whereas `Status` is *authored by us*.

### 5.5 `Thinking { text, signature: Option<String> }` — preserve the replay signature
- **Source.** Grok's `ContentBlock::Thinking { thinking: String, signature: String }`
  (`grok …/messages.rs:120–124`) and its `ReasoningContent { text, encrypted, id }`
  (`grok …/conversation.rs:371–382`) both keep the opaque token needed to *replay* reasoning.
  Codex's `ResponseItem::Reasoning { …, encrypted_content: Option<String>, … }`
  (`codex …/models.rs:840–848`) and its always-on request `include:
  ["reasoning.encrypted_content"]` (survey `02-codex/provider-api.md:29`) do the same on the
  Responses side. Claude Code streams `thinking_delta` + `signature_delta` blocks that "must stay
  contiguous with the following `tool_use`/`tool_result`" (survey `01-claude-code/provider-api.md:43`).
- **Why.** Extended thinking is only *replayable* if you hand the provider back the exact block
  **plus its opaque signature** on the next request; dropping the signature breaks thinking
  continuity and busts the prompt cache (survey `01-claude-code/provider-api.md:43`; ADR-0013).
  So `Thinking` is a first-class block carrying `signature`. It is `Option<String>` because not
  every provider/turn supplies one (chat-completions plaintext reasoning has none; Grok's
  `ReasoningContent.encrypted` is likewise optional), and our own internal/mock construction may
  omit it. The engine appends `Completion.content` (incl. `Thinking`) **verbatim** so the
  signature survives to the next sample (Task 6 §5.5).
- **Why not** a `signature: String` (non-optional, matching Grok's block): would force a
  placeholder for providers that don't emit one and for mock/test construction. `Option` models
  "may be absent" honestly, matching Codex/Grok's `Option<…>` reasoning fields.
- **Why not** carry the encrypted-reasoning `id` too (Grok's `ReasoningContent.id`,
  `conversation.rs:380`; needed for Responses replay): v0 is Anthropic-first (SPEC §Assumptions 2),
  where signature is the replay token; the Responses-API `id` is a wire detail deferred to a
  future OpenAI wire. See §8.
- **Harness diff.** Grok/Anthropic = `signature` string; Codex/Responses = `encrypted_content`
  (+ item `id`); Grok's normalized `ReasoningContent` unifies both as `text`+`encrypted`+`id`. We
  keep the Anthropic-shaped `text`+`signature?` now and would add an `encrypted`/`id` path (or a
  neutral opaque field) when the OpenAI wire lands.

### 5.6 Report envelope frozen at `schema_version: 1`, flat `Status`
- **Source.** Codex enforces "stdout is sacred" with `#![deny(clippy::print_stdout)]` and exactly
  one machine artifact (ADR-0009 Context). Claude Code's headless result uses a *flat* error
  model — `is_error: bool` + a `subtype` string (`error_during_execution`/`error_max_turns`/
  `error_max_budget_usd`/`error_max_structured_output_retries`) (ADR-0014 "Error taxonomy").
- **Why.** ADR-0009: `locode-exec` emits exactly one JSON document; `harness` + `api_schema` make
  A/B runs self-describing; `schema_version` protects consumers from format drift, frozen at `1`
  early so the golden test is the change-control gate. ADR-0014 chose a **single flat `Status`
  enum** over Claude's two-level `is_error`+`subtype` nesting because a flat enum is the clearest
  terminal signal and grows by adding values (with the golden test forcing a deliberate edit). The
  four values map 1:1 onto the engine's `Terminal` outcomes (Task 6 §5.3).
- **Why not** Claude's nested `is_error`+`subtype`: two-level nesting for what is a single closed
  terminal set; harder to `match`, no clearer (rejected, ADR-0014).
- **Why not** an unversioned or semver-string envelope: a plain integer `schema_version` is the
  minimal drift guard; a whole semver is overkill for a shape a golden test already pins.
- **Harness diff.** Claude = `is_error`+`subtype`; Codex = typed error taxonomy internally but a
  single artifact on stdout; we = flat `Status` + `error: Option<String>` in one envelope.

### 5.7 Cache markers are NOT baked into block types (deferred to `CacheHint`)
- **Source.** Grok bakes `cache_control: Option<CacheControl>` into `ContentBlock::Text` and
  `::ToolResult` (`grok …/messages.rs:103–104,117–119`; `CacheControl` at `:92`). Claude Code
  places **exactly one** message-level `cache_control` marker (on the last message) plus ≤3–4 on
  system blocks, tuned to server KV-page eviction — with a source comment warning "Do not add any
  more blocks for caching or you will get a 400" (survey `01-claude-code/provider-api.md:34–37`).
- **Why.** ADR-0013 allows per-block markers "in principle", but cache-breakpoint *placement* is a
  delicate, Anthropic-specific optimization (exactly-one-on-last-message; ≤4 on system; the 400
  cliff) that has no meaning for OpenAI. Baking `cache_control` into the neutral block type would
  (a) leak a vendor concern into the provider-neutral core and (b) invite callers to set markers
  the wire then has to re-derive anyway. Task 3 acceptance (todo.md) states this explicitly:
  "Per-block cache placement deferred to the Anthropic wire via `CacheHint` — ADR-0007/Task 12 —
  not baked into the block types." The wire computes breakpoints from a `CacheHint` on
  `ConversationRequest`.
- **Why not** carry an `Option<CacheControl>` like Grok: couples the core to a vendor and to a
  placement policy the wire owns; the field would be dead weight for the OpenAI mapping.
- **Harness diff.** Grok = per-block field on the neutral type; Claude = wire-side placement; we
  follow Claude's "wire owns placement" split, exposed as `CacheHint` at the request boundary.

### 5.8 `ToolSpec` lives in `locode-protocol` (both tools + provider need it)
- **Source / constraint.** `locode-tools` *builds* a `ToolSpec` from a `Registry` (schemars-
  derived `parameters`); `locode-provider` *consumes* it via `ConversationRequest.tools`
  (todo.md Task 5). Codex's `ToolSpec` "serializes directly to Responses 'Tool' JSON via
  `#[serde(tag="type")]`" (survey `02-codex/provider-api.md:27`) — i.e. it is a shared neutral
  type the wire maps.
- **Why.** The SPEC dep graph forbids `provider → tools` (`provider → protocol` only). The one
  crate both `tools` and every `provider` wire share is `locode-protocol`, so `ToolSpec` is
  hoisted here (lib.rs doc-comment `:230–235` says exactly this). It is a name + description +
  args JSON Schema — the wire-agnostic representation each `Provider` maps onto its own tool
  format (Anthropic `{name,description,input_schema}` vs OpenAI `{type:"function",function:{…}}`).
- **Why not** define it in `locode-tools`: `provider` can't depend on `tools` (cycle/dep-graph
  violation). **Why not** in `locode-provider`: `tools` would then depend on `provider`, also
  wrong-direction.
- **Harness diff.** Both Grok and Codex keep a neutral tool-spec type that each wire serializes;
  we do the same, homed at the DAG root.

### 5.9 The `provider` → `api_schema` rename
- **Source.** Grok distinguishes an `ApiBackend` (wire schema) from an un-enumerated `base_url`
  (gateway); Codex splits `WireApi` (the protocol shape) from `ModelProviderInfo` (endpoint/auth)
  (survey `02-codex/provider-api.md:12`, todo.md Task 5 design notes). The provider trait's method
  is `api_schema() -> &str` (todo.md Task 5), not `name()`.
- **Why.** ADR-0009 (as amended) states the field "names the request/response *protocol shape* —
  the provider's `api_schema()` — not a gateway/endpoint, which is configuration." Calling the
  field `provider` conflated *which wire dialect* (anthropic/openai/mock) with *which
  gateway/endpoint* (OpenRouter/Bedrock/proxy `base_url`) — the latter is config, not identity.
  The rename (`provider → api_schema`) landed across `Report`, `Event::Init`, ADR-0009, and the
  golden snapshot in the Task 6 timeframe (todo.md Task 6 design notes; commit history). The task
  prompt flags this as the recent rename to be aware of.
- **Why not** keep `provider`: it invited callers to read a gateway into a field that means "wire
  dialect", defeating the self-describing-A/B goal (two runs through the same anthropic schema via
  different gateways should stamp the same `api_schema`).
- **Harness diff.** Codex = `WireApi` vs `ModelProviderInfo` (schema vs endpoint); Grok =
  `ApiBackend` vs `base_url`; our envelope now names only the schema, leaving gateway to config.

### 5.10 `ToolCallRecord` (report view) distinct from `ContentBlock::ToolUse` (history view)
- **Source.** Every harness keeps a tool call's *model-facing* text separate from its *structured*
  result: SPEC §Code Style "a tool result has two faces (`output` for the JSON report,
  `to_prompt_text()` for model history)"; `locode-tools` returns both a history `tool_result` and
  a report record (`Dispatched{tool_result,record}`, todo.md Task 4).
- **Why.** The report is a *summary for programs* (`output: Value`, `kind`, `ok`), while the
  conversation's `ToolResult` carries *model-facing* content chunks (`prompt_text`). Same call,
  two projections. `ToolCallRecord.kind` carries the canonical `ToolKind` tag (e.g. `"shell"`) so
  A/B runs across packs align comparable tools even when wire names differ — the whole point of
  the harness-pack A/B (ADR-0012). The `id` field ties the report record back to the
  conversation's `tool_use` id.
- **Why not** reuse `ContentBlock::ToolUse`/`ToolResult` in the report: they carry model-facing
  content and no `kind`/`ok`/structured `output`; the report needs the structured face and the
  cross-pack tag, not the prompt text.

---

## 6. Tests (as built)

All shipped and green (todo.md Task 3 ✅). Inline unit tests + one integration golden.

**Round-trips / serde shape (`lib.rs:319–406`):**
1. `conversation_round_trips_all_roles_and_tool_pairing` (`:319–384`) — a `Conversation` with
   all four roles + a `ToolUse`/`ToolResult` pair serializes to JSON and back **losslessly**
   (`assert_eq!(conversation, back)`), then asserts `tool_use.id == tool_result.tool_use_id` (the
   ADR-0004 pairing link). This is the SPEC-required round-trip verification (native serde, not a
   wire format).
2. `content_block_uses_anthropic_style_type_tags` (`:387–393`) — pins `Text` →
   `{"type":"text","text":"hi"}` (the `#[serde(tag="type")]` shape).
3. `status_serializes_to_adr_0009_strings` (`:396–406`) — all four `Status` values →
   `"completed"/"max_turns"/"model_error"/"error"`. Guards §5.6.

**Envelope golden (`tests/envelope_golden.rs`):**
4. `report_matches_committed_snapshot` (`:35–44`) — a fixed `Report` serializes to a
   `serde_json::Value` equal to the committed `report_snapshot.json`. Freezes the envelope shape;
   drift fails the build (the SPEC "golden test" + the change-control gate).
5. `schema_version_is_frozen_at_1` (`:47–49`) — literal `== 1`.

The Task 3b tests (`events_reconstruct_full_conversation`, `event_uses_snake_case_type_tags`) live
in the same module — covered in the Task 3b plan.

**What is NOT directly tested (by design):** `Image`/`Thinking` variants have no dedicated
round-trip beyond being constructible + serde-derived (they *are* exercised structurally in the
Task 3b reconstruction test only for `Text`/`ToolUse`/`ToolResult`); `Usage::AddAssign` is
exercised in the engine's `usage_summed` test (Task 6), not here. See §8.

---

## 7. Dependencies

No new **external** crates beyond what the workspace already vendors; no "Ask first" trigger.
`crates/locode-protocol/Cargo.toml`:

| Dep | Why |
|---|---|
| `serde` (derive) | `Serialize`/`Deserialize` on every type. |
| `serde_json` | `Value` for `ToolUse.input`, `ToolCallRecord.args/output`, `ToolSpec.parameters`, `Report.structured_output`, and `Event::Init.tools` (Task 3b). |

No `tokio`, no `async-trait`, no `schemars` (schema derivation is `locode-tools`' job; this crate
only *holds* the derived `Value`). No dependency on any sibling `locode-*` crate — this is the DAG
root (SPEC §Project Structure: `protocol ← everything`).

---

## 8. Open questions / concerns / future considerations (exhaustive & honest)

1. **`Image` / `Thinking` only partially exercised in v0.** Both variants exist and serde-derive,
   but no live loop drives multimodal input or extended thinking end-to-end, and there is no
   dedicated round-trip test for either (only `Text`/`ToolUse`/`ToolResult` appear in the
   reconstruction/round-trip tests). Risk: a subtle serde bug in `Thinking{signature}` or
   `ImageSource::Base64` would not surface until Task 12 (wire) or a multimodal pack. Should we add
   round-trip tests for `Thinking` (with and without `signature`) and both `ImageSource` arms now,
   given how load-bearing `signature` preservation is (§5.5)?

2. **`Thinking` carries `signature` but not the Responses-API `id`/`encrypted` split.** We chose
   the Anthropic-shaped `text`+`signature?` (§5.5). Grok's normalized `ReasoningContent` keeps
   `text`+`encrypted`+`id` (`grok …/conversation.rs:371–382`) and Codex keeps `encrypted_content`
   +item-`id` (`codex …/models.rs:840–848`) because the Responses API needs the item `id` to
   replay reasoning. When the OpenAI/Responses wire lands, does `Thinking` grow an `encrypted`/`id`
   field (widening the block), or does the wire carry an out-of-band reasoning map keyed by
   position? `#[non_exhaustive]` lets us add a field, but changing a field is still a shape change.

3. **`structured_output` slot present but inert.** The field is frozen into the envelope (always
   `None`) so `--json-schema` (ADR-0009; SPEC Open Q3; ADR-0014) can fill it later without a
   schema-version bump. Open: when it lands, is a schema-constrained answer *always* a JSON
   `Value`, or do we also need to record the schema used / a validation-failure signal (Claude's
   `error_max_structured_output_retries` subtype, ADR-0014)? Does adding a retry-exhaustion status
   value force a `schema_version` bump or is it an additive `Status` variant?

4. **No `total_cost_usd` / cost accounting.** Tokens-only (ADR-0014). Adding cost needs a pricing
   table (per model × per token-class) and a decision on *where* it lives — on `Usage`, on
   `Report`, or computed by `locode-exec` at print time from `Usage` + a price map. Is cost even a
   core-library concern, or does it belong to the future `locode-app`?

5. **Usage summation over-counts input across turns.** `AddAssign` field-wise sums, and the engine
   sums every turn's `Usage` (Task 6 §5.9); because each request re-sends the full history,
   `input_tokens` accumulates the growing context (turn 2 re-counts turn 1's context). The envelope
   documents this as a summary, not a precise per-request ledger. Alternatives: take input/cache
   from the *final* completion only (reflects full context once) and sum output; or add a
   `per_turn: Vec<Usage>` breakdown. Is the over-count acceptable for A/B (both packs over-count
   the same way, so relative comparison holds), or do we want precise accounting before the first
   A/B (Task 16)? This is really a Task 6 concern but the `Usage` shape here constrains the fix.

6. **Per-message cache placement / `CacheHint`.** We deliberately kept `cache_control` out of the
   block types (§5.7), deferring to a `CacheHint` on `ConversationRequest` (Task 12). Open: is a
   single request-level `CacheHint` expressive enough for Anthropic's "≤4 system breakpoints +
   exactly-one-on-last-message" placement (survey `01-claude-code/provider-api.md:34–37`), or does
   the wire need finer per-message hints? If finer, does that pressure ever push a marker back onto
   the block type (re-litigating §5.7)? Grok chose per-block; we bet the wire can place from a hint
   — unproven until Task 12.

7. **Is the `Developer`-role mapping right?** We map `Developer` → Anthropic mid-conversation
   `role:"system"` (beta) *or* a `<system-reminder>` user block (ADR-0013 leaves the choice a wire
   flag). But the empirical capture shows Claude Code uses **both** surfaces for *different* kinds
   of injected context: bare `role:"system"` for capability deltas vs `<system-reminder>` inside
   `user` for per-turn ephemeral context (note surfaces 2 and 3). Our single `Developer` role
   collapses two physically-distinct surfaces. Is one role enough, or will we need to distinguish
   "durable capability context" from "ephemeral per-turn reminder" (e.g. a block flag or a second
   role) when we port Claude's pack? Currently the wire flag decides globally, not per-message.

8. **Envelope evolution / versioning policy.** `schema_version: 1` is frozen and the golden test
   gates changes, but we have **no written policy** for *when* a change bumps the version vs. is a
   backward-compatible additive field. Adding an `Option<T>` field (serializes to `null`) is
   arguably non-breaking for tolerant consumers; removing/renaming/retyping is breaking. Do we
   commit to "additive `Option` fields keep v1; any removal/rename/retype → v2", and does the
   golden snapshot need a matching "v1 frozen forever" copy so we can test both versions? The
   `provider → api_schema` rename (§5.9) was exactly a rename that *did not* bump the version
   (it happened before any external consumer existed) — that precedent should be written down.

9. **Does `reconstruct_conversation` (and the `Event` types) belong here?** Task 3b put the stream
   protocol + reconstruction in `locode-protocol`. There's a live question whether stream
   reconstruction is a *protocol* concern or a future `locode-transcript`/session-durability
   concern (SPEC §Assumptions 6 defers JSONL persistence). Keeping it here means `protocol` grows a
   function with light logic (not just types); a future durable-session crate might want to own
   replay/fork/compaction of transcripts. Flagged for the Task 3b plan; noted here because the two
   tasks share the file.

10. **`Status` is closed but the loop may grow terminal states.** ADR-0014 says "grow its values
    as the loop introduces terminal states." Codex has *no* hard turn cap and treats compaction as
    the runaway guard (no `MaxTurns`); if we later add compaction, budget ceilings
    (`error_max_budget_usd`), or refusal-as-terminal, each is a new `Status` value + a golden edit.
    Because `Status` is **not** `#[non_exhaustive]` (§5.4), adding a value is a (minor) breaking
    change for external exhaustive matchers. Intentional (we want the golden test + a compile break
    to force consumers to handle new terminals) — but worth confirming we're comfortable with that
    friction as terminals grow.

11. **`kind: String` on `ToolCallRecord` vs a typed `ToolKind`.** The report stores the canonical
    tool kind as a bare `String` (e.g. `"shell"`), while `locode-tools` has a typed `ToolKind`
    enum (todo.md Task 4). Stringifying at the envelope boundary keeps `protocol` from depending on
    `tools` (dep-graph), but it means a typo or drift between the enum's serialization and a
    hand-built record is not caught by the type system. Acceptable (the envelope is a wire
    artifact), but the mapping `ToolKind → String` must stay stable for A/B alignment across
    versions — another thing the versioning policy (Q8) should cover.

---

### Speech-to-text / identifier confirmations
This plan was written from the shipped source + written ADRs; **no user identifiers were
reconstructed**. The one term worth confirming from the task prompt: the field rename is
**`provider → api_schema`** (on `Report` and `Event::Init`), which matches the shipped code
(`lib.rs:152–155`, `:266–267`) — recorded in §5.9.
