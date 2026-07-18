# Design discussion: the report / structured-output envelope

> **Status: discussion doc** (not an ADR yet). Purpose: study how the four surveyed
> agents emit headless structured output, compare against locode's current `Report`
> envelope (ADR-0009 + the Task 3 types), and converge on the v0 shape + the seams we
> reserve. When we settle, this hardens into an ADR that refines ADR-0009.
>
> Primary reference: **Claude Code's `--output-format text|json|stream-json` + `--verbose`**
> (the most mature of the four). Grounded in source, not memory — see the per-agent tables.

## 1. Where we are today

`locode-protocol::Report` (ADR-0009, implemented in Task 3) is a **single buffered JSON
object** on stdout:

```
schema_version=1 · status · harness · provider · final_message? · structured_output?
· turns · tool_calls[] · usage · session_id · error?
```
- `status ∈ {completed, max_turns, model_error, error}`
- `tool_calls[] = ToolCallRecord{ id, name, kind, args, ok, output }`
- `usage = { input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens }`
- stdout discipline is compiler-enforced in `locode-exec` (`#![deny(clippy::print_stdout)]`).

**Gaps this doc weighs:** no timing, no model id, no cost, no reasoning-token line, a coarse
error taxonomy, no streaming event mode, and no `--output-format` selection.

## 2. Prior art (from source)

### 2.1 Output-mode flags — all four converge on a *text / single-JSON / streaming-JSONL* triad

| Agent | Flag | Modes | Single final JSON object? | Streaming JSONL? |
|---|---|---|---|---|
| **Claude Code** | `--output-format` | `text` (default) · `json` · `stream-json` | ✅ `json` | ✅ `stream-json` (**requires `--verbose`**; `--include-partial-messages` for deltas) |
| **Codex** | `--json` (+ `--output-last-message FILE`, `--output-schema FILE`) | text (default) · JSONL | ❌ (no single-object mode) | ✅ |
| **Grok Build** | `--output-format` | `plain` · `json` · `streaming-json` | ✅ `json` | ✅ `streaming-json` |
| **OpenCode** | `--format` | `default` · `json` | ❌ | ✅ (json is streaming-only) |

Only **Claude and Grok** offer a *single buffered JSON object* (which is what our `Report`
is). Codex and OpenCode's JSON is streaming-only. Claude uniquely gates `stream-json` behind
`--verbose`.

### 2.2 The final result object (single summary)

| | fields |
|---|---|
| **Claude** (`SDKResultMessage`) | `type:"result"`, `subtype`, `result` (final text), `is_error`, `num_turns`, `duration_ms`, `duration_api_ms`, `stop_reason`, `total_cost_usd`, `usage`, `modelUsage`, `permission_denials`, `structured_output?`, `session_id`, `uuid`. **Error variant** drops `result`, adds `errors: string[]`, `subtype ∈ {error_during_execution, error_max_turns, error_max_budget_usd, error_max_structured_output_retries}` |
| **Grok** (`build_json_result`) | `text`, `stopReason`, `sessionId`, `requestId`, `thought?`, `structuredOutput?`/`structuredOutputError?`, `num_turns`, `usage`, `total_cost_usd`, `total_cost_usd_ticks`, `modelUsage`, `usage_is_incomplete?` |
| **Codex** | **none** — terminal signal is a `turn.completed{usage}` event; `--output-last-message` writes the raw final text to a file |
| **OpenCode** | **none** — loop stops on session-idle; the answer is accumulated `text` parts, no summary line |

**Claude has the richest terminal summary; Grok mirrors it. Codex and OpenCode have no rich
final object at all.** Our `Report` is in the Claude/Grok camp (correct for a headless engine
whose output is consumed programmatically).

### 2.3 The streaming event schema (when JSONL is on)

- **Codex — the cleanest, most formally typed.** A serde-tagged `ThreadEvent` enum, exactly **8 events**: `thread.started`, `turn.started`, `turn.completed{usage}`, `turn.failed{error}`, `item.started` / `item.updated` / `item.completed` (each `{item}`), `error{message}`. Items are `ThreadItem{ id: "item_N", ...details }` where `ThreadItemDetails` ∈ `agent_message` / `reasoning` / `command_execution{command,aggregated_output,exit_code,status}` / `file_change` / `mcp_tool_call` / `web_search` / `todo_list` / `error`.
- **Claude — richest, ~30 variants (Zod).** `system`/`init` (cwd, session_id, tools[], model, agents[], skills[], …), plus `system` subtypes (`compact_boundary`, `status`, `api_retry`, …), `assistant`, `user`, `stream_event` (deltas), `result`, `tool_progress`, `tool_use_summary`, `rate_limit_event`, control-protocol frames.
- **Grok** — inline `{"type": …}` literals: `text`, `thought`, `end{stopReason,sessionId,…usage}`, `error{message}`, `max_turns_reached`, plus `auto_compact_*` lifecycle.
- **OpenCode** — `emit(type,data)` lines `{type,timestamp,sessionID,…}`: `tool_use`, `step_start`, `step_finish`, `text`, `reasoning`, `error`. (Drops `message.updated`, so message-level totals are absent in JSON mode.)

**Takeaway:** if/when we add streaming, **Codex's typed 8-event `ThreadEvent`/`ThreadItem`
enum is the model to copy** — small, serde-tagged, dotted names, no loose string literals.

### 2.4 stdout / stderr discipline

| Agent | Enforcement |
|---|---|
| **Codex** | **strictest** — compiler-enforced `#![deny(clippy::print_stdout)]`; only 2 allow sites (the JSONL line + final message); everything else `eprintln!` |
| **Claude** | runtime **NDJSON stdout-guard** — monkey-patches `stdout.write`, JSON-parses each line, reroutes any non-JSON to stderr with a `[stdout-guard]` marker; escapes U+2028/2029 |
| **Grok / OpenCode** | call-site convention only. *Both route JSON-mode errors to **stdout** as an error event*, not stderr |

We already match Codex (compiler-enforced). Claude's runtime guard is an optional belt-and-suspenders.

### 2.5 Usage / cost

| Agent | Token fields | Cost |
|---|---|---|
| **Claude** | `usage` (snake, raw Anthropic: `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`, `cache_creation.{ephemeral_1h,ephemeral_5m}`, `server_tool_use`, `service_tier`) + `modelUsage` map (camel: `inputTokens`, `costUSD`, `contextWindow`, …) | ✅ `total_cost_usd` |
| **Codex** | `input_tokens`, `cached_input_tokens`, `cache_write_input_tokens`, `output_tokens`, `reasoning_output_tokens` | ❌ **none** (tokens only) |
| **Grok** | `input_tokens` (uncached), `cache_read_input_tokens`, `output_tokens`, `reasoning_tokens`, `total_tokens` | ✅ `total_cost_usd` + `total_cost_usd_ticks` (i64 exact, 1e10/USD) |
| **OpenCode** | nested `tokens{ input, output, reasoning, cache{ read, write } }` | ✅ `cost` (computed client-side from models.dev pricing) |

Notes: **cost is split** — Claude/Grok/OpenCode emit it, **Codex refuses** (tokens only).
Casing is deliberately mixed even within one tool (Claude & Grok both pair a snake_case
`usage` with a camelCase `modelUsage`, "frozen for external-tool compat"). Reasoning/thinking
tokens are tracked *separately* by Codex/Grok/OpenCode.

### 2.6 Error representation — two families

- **Flag + subtype** (Claude): `is_error` + a `subtype` enum + `errors: string[]`.
- **Typed event/union** (Codex `turn.failed`/`error`; OpenCode `Assistant.error` NamedError union — `ProviderAuthError`, `ContextOverflowError`, `APIError{statusCode,isRetryable,…}`, …; Grok inline `{"type":"error"}`).
- **Universal:** *soft tool errors stay in-band* — a normal `tool_result` with an error flag, model keeps going (matches our ADR-0004 soft/fatal split). Only fatal/terminal conditions reach the result object.

## 3. What this implies for locode

### 3.1 Modes — adopt the triad as a seam, ship one mode in v0
Reserve **`--output-format {json, text, stream-json}`** (align names with Claude/Grok):
- **`json`** (v0 default for `locode-exec`): the single buffered `Report` on stdout. This is our ADR-0009 contract and matches Claude/Grok's `json`.
- **`text`**: just `final_message` to stdout (human pipe). Trivial, add in v0 or Task 14.
- **`stream-json`**: JSONL event stream. **Post-v0**, modeled on Codex's typed `ThreadEvent`.

This keeps v0 = "one JSON object" while reserving the streaming seam explicitly, rather than pretending events don't exist.

### 3.2 Report envelope — proposed refinements (for review against ADR-0009)
Keep the current fields; consider these additions, all cheaply available in `locode-exec`:

| Add | Rationale | Prior art |
|---|---|---|
| `model` | which model actually ran — essential for A/B analysis alongside `harness`/`provider` | Claude `system/init.model` |
| `duration_ms` (wall) | cheap via `Instant`; standard in result objects | Claude `duration_ms`; Grok timing |
| `usage.reasoning_tokens` | thinking is on (ADR-0013); track it distinctly | Codex `reasoning_output_tokens`, Grok `reasoning_tokens` |
| richer `status`/error subtype? | our `{completed,max_turns,model_error,error}` is coarser than Claude's error subtypes | Claude `error_max_turns`, `error_during_execution`, … |

Deliberately **defer**: `total_cost_usd` (needs a pricing table; **Codex omits cost entirely** — reasonable to skip until we want it), `permission_denials` (no permissions in v0), `modelUsage` map (single-model runs for now), `duration_api_ms` (needs per-call timing).

**Decision needed on `status` vs error-subtype:** keep the flat 4-value `status` (simple), or split into `status` + an `error.kind` subtype enum (Claude-style: `max_turns` / `model_error` / `tool_fatal` / `auth` / `config` / `overflow`)? The latter is more debuggable; the former is what we have.

### 3.3 Usage field naming
We use neutral names (`input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_creation_tokens`) and map from each provider's raw usage inside the wire impl. This avoids Claude/Grok's frozen-mixed-casing problem. Recommend **keep neutral names**; add `reasoning_tokens`.

### 3.4 stdout discipline
We already match Codex (compiler-enforced). Optionally add Claude's runtime NDJSON guard in `locode-exec` later (parse-each-line, reroute non-JSON to stderr) as defense-in-depth for `stream-json` mode. Not needed for single-`json` mode.

### 3.5 The streaming event model (reserved, post-v0)
When we build `stream-json`, copy Codex's shape: a serde-tagged event enum, e.g.
`session.started` · `turn.started` · `turn.completed{usage}` · `message{role, blocks}` ·
`tool.started{call}` · `tool.completed{record}` · `result{report}` · `error{message}`.
The final `result` event carries the same `Report` — so the single-JSON and streaming modes
share one summary type. This is the key design constraint: **one `Report`, emitted either
alone (`json`) or as the terminal `result` event (`stream-json`).**

## 4. Recommendation (v0)

1. **Ship `--output-format json` (default) + `text`.** Reserve `stream-json` as a documented seam.
2. **Add `model` and `duration_ms` to `Report`; add `usage.reasoning_tokens`.** (Bump nothing — still `schema_version: 1` since it's pre-release; freeze after first tag.)
3. **Defer** cost, `modelUsage`, `permission_denials`, and the streaming events to post-v0.
4. **Decide** the `status` vs `status + error.kind` question (§3.2) — the one real fork.
5. When streaming lands, use **Codex's typed event enum**, with the terminal `result` event carrying the same `Report`.

## 5. Open questions

1. **Error taxonomy:** flat `status` (current) vs `status + error.kind` subtype enum? (Claude splits; Codex/OpenCode use typed unions.)
2. **Cost:** ever compute `total_cost_usd` (needs a pricing table), or stay tokens-only like Codex?
3. **Transcript:** should `json` mode optionally embed the full message transcript, or is that strictly the `stream-json`/`--events-jsonl` job? (Our envelope is a *summary* — final_message + tool_calls + usage — not a transcript.)
4. **`--verbose` gate:** Claude requires `--verbose` for `stream-json`. Adopt that ergonomic, or make `stream-json` self-sufficient?
5. **Schema versioning:** freeze `schema_version` at the first tagged release; until then, additive changes stay at `1`. Agree?
