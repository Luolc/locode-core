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
