# ADR-0013: Conversation protocol — 4-role, Anthropic-shaped content-block model

## Status
Accepted

## Date
2026-07-17

## Context
`locode-protocol` needs a conversation model that (a) is rich enough to express
roles, multi-block content, tool-call/result pairing, reasoning, and multimodal
content, and (b) is **provider-neutral** so one internal model maps to both the
Anthropic Messages API and the OpenAI Chat/Responses APIs. The design doc's
original "minimal history model" (`System(String)` / `User(String)` /
`Assistant{text, tool_calls}` / `Tool{…}`) is too flat — it can't carry image or
thinking blocks and hides the block structure both vendors actually use. This
supersedes that flattened model and refines the message shape of the
`ConversationRequest` introduced in [ADR-0007](ADR-0007-provider-trait.md).

**Empirical grounding** (a transparent reverse-proxy capture of Claude Code → the
real Anthropic API, plus the Claude Code source — see the `claude-code-system-surfaces`
research note): a real request carries **three distinct "system" surfaces** — a
static **top-level `system`** (cached), a **mid-conversation `role:"system"` message**
(enabled by the `mid-conversation-system` beta) carrying *client-injected* capability
context (skills, subagents, reminders), and **`<system-reminder>` text blocks inside
`user` messages**. Tool **schemas** travel separately via the native `tools` param and
are *server-rendered*. The lesson: Anthropic overloads the word "system" for two
semantically different things — the static constitution and dynamic client-injected
instructions. OpenAI already separates these as `system` (legacy/static) vs
**`developer`** (app-author instructions; hierarchy platform > developer > user).

## Decision
Adopt a **four-role** model whose roles carry semantics, not wire names:

| Role | Meaning | Set by | Cardinality |
|------|---------|--------|-------------|
| **System** | Immutable base identity + safety/policy (the "constitution"). | harness/platform, once | usually one, front-loaded |
| **Developer** | App-author instructions + **dynamically injected context** (tool/skill availability, environment reminders, mid-conversation nudges). | the harness, repeatedly | many, anywhere |
| **User** | The human's turns; **also carries `tool_result`** (Anthropic convention). | end user / the loop | many |
| **Assistant** | Model turns: `text`, `thinking`, `tool_use`. | the model | many |

Splitting **System** (static) from **Developer** (client-injected) resolves
Anthropic's naming collision and matches the vendors' converging semantics — we
borrow OpenAI's word (`developer`) for the injected one.

### One uniform message stream, Anthropic-shaped blocks
```rust
struct Conversation { messages: Vec<Message> }        // no separate `system` field
struct Message { role: Role, content: Vec<ContentBlock> }
enum Role { System, Developer, User, Assistant }

#[non_exhaustive]
enum ContentBlock {
    Text(String),
    Image(ImageSource),                                    // multimodal
    Thinking { text: String, signature: Option<String> },  // assistant reasoning
    ToolUse    { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: Vec<ResultChunk>, is_error: bool },
    // reserved: Document { .. }
}
enum ResultChunk { Text(String), Image(ImageSource) }
enum ImageSource { Base64 { media_type: String, data: String }, Url(String) }
// content blocks may carry an optional cache marker (Anthropic sets cache_control per-block)
```
Types are Rust-native and provider-neutral; **each `Provider` wire owns its own
(de)serialization** (no vendor coupling in the core). The
`tool_use.id ↔ tool_result.tool_use_id` link is the pairing invariant from
[ADR-0004](ADR-0004-error-taxonomy-and-pairing.md).

### Mapping — locode → Anthropic (Messages API)
| locode role | Anthropic placement |
|-------------|---------------------|
| **System** | the **top-level `system`** param (blocks, with `cache_control`); leading System messages are hoisted out of the stream into it |
| **Developer** | a mid-conversation **`role:"system"` message** (beta `mid-conversation-system`) — *or* fallback: a `role:"user"` message wrapped in `<system-reminder>…</system-reminder>` (wire flag; default = portable fallback) |
| **User** | `role:"user"`; content blocks incl. `tool_result` |
| **Assistant** | `role:"assistant"`; blocks `text` / `thinking` / `tool_use` |

### Mapping — locode → OpenAI (Chat Completions / Responses)
| locode role | OpenAI placement |
|-------------|------------------|
| **System** | `role:"system"` message (downgrade to `developer` on models that only know that) |
| **Developer** | **`role:"developer"` message** (exact semantic match) |
| **User** | `role:"user"` message |
| **Assistant** | `role:"assistant"` + `tool_calls[]` (`tool_use` → `tool_calls`; `input` JSON → **stringified** `arguments`) |
| a User message's `tool_result` blocks | **exploded** into separate `role:"tool"` messages (`tool_call_id`) |

Responses (inbound) map trivially both ways: a model only emits **Assistant** content.

### The naming rule, stated so we never trip on it
> **An Anthropic `role:"system"` message is our `Developer`, not our `System`.**
> Our `System` becomes Anthropic's *top-level* `system`. "System" points at different
> things by direction — hence renaming the injected one to `Developer`.

### Conventions
- **System is front-loaded** (Anthropic `system` is a single top-level param; the wire concatenates leading System blocks into it). Developer is the mid-stream role.
- **`tool_result` lives in a `User` message** in our model; the OpenAI wire explodes it into `tool` messages.
- **Thinking / Image / Document** variants exist now (`#[non_exhaustive]`) but only `Text` / `ToolUse` / `ToolResult` are fully wired in v0.

## Alternatives Considered
- **Flattened minimal model** (design doc): rejected — cannot express multimodal or thinking blocks; hides block structure.
- **Classic 2-role (system top-level only, User/Assistant)**: rejected — no first-class representation of mid-conversation client-injected context; forces reminder-blocks-only.
- **3-role reusing "system" mid-stream**: rejected — perpetuates Anthropic's naming collision; `Developer` is clearer and matches OpenAI.

## Consequences
- One internal model serializes faithfully to Anthropic and mechanically to OpenAI; the vendor-specific quirks (system hoisting, Developer rendering, tool-result exploding, arg stringifying) live entirely inside each wire impl.
- The core stays provider-neutral (ADR-0007) while being modeled on Anthropic's block shape (the maintainer's preference).
- `Developer` fidelity is a wire flag (beta message vs portable `<system-reminder>` block) — decided in the Anthropic wire task, not the protocol crate.
- Updates Task 3's acceptance to this model.

## Amendment (2026-07-18): `ContentBlock::RedactedThinking`

The Task-12 live smoke against the real Anthropic wire returned a
`redacted_thinking` block (provider-encrypted reasoning) alongside signed
thinking. The API requires it to be **replayed verbatim** in the assistant turn,
exactly like signed thinking — dropping it invalidates a thinking + tool-use
replay — so it must survive the round trip through the neutral model.
`ContentBlock` (already `#[non_exhaustive]`) gains:

```rust
/// Assistant reasoning the provider encrypted (Anthropic `redacted_thinking`).
RedactedThinking { data: String },
```

Carried opaquely end-to-end: the wire parses it into the block, the engine
appends it with the rest of the assistant content, and the wire re-emits it
untouched on the next request. No other crate interprets `data`.

## Amendment (2026-07-19): unified `Reasoning` block (supersedes the `RedactedThinking` amendment)

`ContentBlock::Thinking` and `ContentBlock::RedactedThinking` are replaced by one
block, designed in the Task-18 plan review (user's design):

```rust
Reasoning { format: ReasoningFormat, text: String,
            signature: Option<String>, payload: Option<Value> }
enum ReasoningFormat { Anthropic, AnthropicRedacted, OpenAiResponses, TextOnly }
```

`format` selects the replay contract (values echo the `api_schema` strings):
`anthropic` = full `text` + `signature` validator; `anthropic_redacted` =
encrypted `payload`; `openai_responses` = summary in `text`, the WHOLE Responses
reasoning item opaquely in `payload` (replayed verbatim — gateways add fields
nobody models); `text_only` = capture-only, never replayed. Each wire's build
replays only its own format(s) and drops foreign formats (a session never
crosses wires). Rationale: one reasoning shape in every trace/report for eval
tooling, with semantics explicit instead of inferred from field shapes.

## Amendment (2026-07-23): `Developer` is native-mapped only; injected framing is `User` (see [ADR-0023](ADR-0023-fidelity-boundary-and-agents-md-loading.md))

The original decision let `Developer` carry "environment reminders, mid-conversation
nudges" and gave it a **portable fallback** rendering: a `role:"user"` message
wrapped in `<system-reminder>` (default on the Anthropic wire, table row 74;
`crates/locode-provider/src/anthropic/build.rs:133-146`). Designing `AGENTS.md`
injection exposed that this fallback is **not reverse-lossless**: forward
(`Developer` → user `<system-reminder>`) is fine, but given a `role:"user"` payload
message nothing reliably recovers whether it *was* a `Developer` — vs. genuine user
text or a plain reminder. Reconstructing the role needs hand-maintained
tag/format detection that differs per pack and breaks the instant a user's own text
contains the sentinel. `Developer` earns its place precisely by mapping **1:1 and
losslessly** onto a native provider role (OpenAI `role:"developer"`; Anthropic beta
mid-conversation `role:"system"`), and the fallback quietly breaks that property.

Therefore, narrowing the role:

- **`Developer` is reserved for content that has a genuine native role to ride** —
  the beta system message, or OpenAI `developer` — where `Developer ⇄ payload`
  round-trips bijectively. It is **no longer** the vehicle for reminders or
  injected framing.
- **Injected framing — project instructions (`AGENTS.md`), reminders, nudges — is
  authored as `User`** content blocks carrying `<system-reminder>…</system-reminder>`
  from the start. A value whose only faithful rendering on a wire is the
  user-`<system-reminder>` fallback must be `User`, so there is no role to recover
  and the conversation ⇄ payload conversion is losslessly bidirectional by
  construction.
- `DeveloperRendering::SystemReminder` stays in the Anthropic wire for a caller who
  deliberately emits a `Developer` message without the beta, but it carries this
  **reverse-lossy caveat** and is not used for reminders. Whether to retire it (make
  a non-beta `Developer` an error) is ADR-0023 Open Question 6.

The role table (line 38) and the Anthropic mapping (line 74, "*or* fallback …") are
read subject to this amendment: the fallback is a deliberate, caveated escape
hatch, not the home for reminders.
