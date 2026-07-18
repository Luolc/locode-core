# ADR-0003: Typed `Tool` contract with derived schemas and dual output

## Status
Accepted

## Date
2026-07-17

## Context
The model's tool spec and the code that executes the tool must not drift. Codex co-locates `spec()` with `handle()`; Grok derives schemas from `Args: JsonSchema` (schemars). Separately, a tool result serves two different readers: the host/JSON report wants **structured data**, the model wants **rendered text**. Grok's `ToolRunResult { output, prompt_text }` captures this.

## Decision
Define one `Tool` trait with associated `Args` (`DeserializeOwned + JsonSchema`) and `Output` (`Serialize + ToolOutput`). The wire JSON Schema is **derived** from `Args` via `schema_for!`, never hand-written. `Output` exposes two faces: the structured value (into the report's `tool_calls[]`) and `to_prompt_text()` (into model history). Canonical identity is a `ToolKind` enum, distinct from the client-facing wire name (assigned by the **pack at registration** — ADR-0012 supersedes the ADR-0006 "dialect" model; as built, `Tool` has no `name()` and `Registry::register(name, tool)` sets it). Type-erase to `Box<dyn DynTool>` at the registry boundary (decode JSON → `run` → re-serialize). Keep `ToolCtx` small: `{ cwd, call_id, workspace_root, cancel }`.

## Alternatives Considered
### Hand-written JSON schemas
- Rejected: spec and handler drift; the single most common source of tool bugs.

### Single result value (structured only, stringify for the model)
- Rejected: the model often needs a different rendering than the report (e.g. `read` returns `{path, lines, truncated}` structurally but the file body as text). Collapsing them loses information in one direction or the other.

### Fat tool object carrying UI/render methods (Claude Code style)
- Rejected: couples presentation into the contract; `locode-core` is headless. Tools return data + text only.

## Consequences
- Adding a tool = define `Args`/`Output` types + `run`; the schema follows for free.
- The report envelope and the model context are independent renderings of one call — the key property for a JSON-output agent.
- A god-object context (Claude's ~40-field `ToolUseContext`) is explicitly avoided.
