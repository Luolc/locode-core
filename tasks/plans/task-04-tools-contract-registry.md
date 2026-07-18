# Task 4 — `locode-tools`: the typed `Tool` contract, registry, and dispatch door

Retrospective, source-grounded plan. **Written after the fact**: Task 4 is
implemented and merged (commit history through Phase 1; `tasks/todo.md` Task 4 ✅).
This is the design doc we skipped writing before the code — it records **what is
actually built and why**, grounded in the shipped source and the harness study.
It changes no code.

Source of truth for the decisions: `SPEC.md` (§Code Style, the tool contract),
ADR-0003 (typed tool contract), ADR-0004 (error taxonomy + pairing), ADR-0008
(one dispatch door + path jail), and `tasks/todo.md` Task 4 (incl. the
"decided during implementation" design notes). Every non-obvious decision is
grounded in the two harnesses whose real tool runtimes we studied, with
`file:line` citations.

Submodule roots (abbreviated in citations):
- `grok` = `~/dev/coding-cli-survey/submodules/grok-build/crates/codegen/xai-grok-tools/src`
- `codex` = `~/dev/coding-cli-survey/submodules/codex/codex-rs`

Shipped files this plan documents (read in full):
`crates/locode-tools/src/{tool.rs, ctx.rs, error.rs, registry.rs, lib.rs}`.

---

## 1. Purpose & scope

Build **the single most important type in the system** (SPEC §Code Style,
todo Task 4): the typed `Tool` contract, its `ToolKind` classification tag, the
soft/fatal error taxonomy, the small per-call `ToolCtx`, the type-erasure seam
(`DynTool` + the `TypedTool<T>` adapter), the `Registry`, and the one
`dispatch` door every tool call funnels through. This crate is the *framework*:
it is **host-agnostic and holds no concrete tools** — no filesystem, no shell,
no `rg`. Those arrive as the grok pack over `locode-host` (Tasks 7–11). Here we
fix the shapes the whole loop and every future pack depend on.

The crate has one job: make "author a tool against concrete Rust types, and the
model-facing JSON Schema + the report record + the paired `tool_result` follow
for free" true, and make **every side effect go through one door** so that
policy/sandbox/timeouts can be added in exactly one place later (ADR-0008).

### In scope (v0, as built)
- The `Tool` trait: associated `Args: DeserializeOwned + JsonSchema + Send`,
  `Output: Serialize + ToolOutput + Send`, `kind()`, `description()`, a
  **default-derived** `parameters_schema()`, and `async fn run`.
- `ToolKind` — a closed `snake_case` enum (`Shell/Read/Write/Edit/Glob/Grep`)
  plus a `#[serde(other)] Other` catch-all, for **cross-pack A/B alignment only**
  (not the wire name, not the Rust type name).
- `ToolOutput::to_prompt_text()` — the model-facing face of a tool's output.
- `ToolError { Respond, Fatal }` — the two recovery paths (ADR-0004).
- `ToolCtx { cwd, call_id, workspace_root, cancel }` — the deliberately small
  per-call context (ADR-0003 rejects a god-object context).
- The object-safe `DynTool` trait, the `TypedTool<T>` erasure adapter (a
  wrapper, **not** a blanket impl — §5.5), and `ToolRunResult { output,
  prompt_text }`.
- `Registry`: `register<T: Tool>` (typed), `register_dyn(Box<dyn DynTool>)`
  (the MCP/dynamic seam), `contains`, `names`, `specs() -> Vec<ToolSpec>`, and
  `dispatch(name, raw_args, ctx) -> Dispatched { tool_result, record, fatal }`.
- Duplicate-name registration panics at startup; unknown tool + bad args are
  **soft** (`Respond`-shaped `is_error` results), never fatal.

### Out of scope / deferred (reserved seams, not built here)
- **Concrete tools + the host.** No `run_terminal_command`/`read_file`/`write`/
  `search_replace`/`grep`/glob here; they are the grok pack (Tasks 9–11) over
  `locode-host` (Task 7). The trait is the seam; the impls live elsewhere.
- **Policy / sandbox / timeouts / path-jail enforcement.** ADR-0008 puts these
  *behind* the dispatch door, but the door in v0 is always-allow: `ToolCtx`
  merely *carries* `workspace_root` and `cancel`; nothing in this crate resolves
  paths, enforces the jail, times out a subprocess, or caps output. Those live in
  `locode-host` (Task 7) and are *invoked by tool bodies*, funneled through this
  same `dispatch`. Reserved seam: the one place to add a decision gate later.
- **Output truncation.** `truncate_for_model` is a shared post-process that lands
  with `locode-host` (Task 7) and is applied by the engine before the model
  re-enters (ADR-0008; todo Task 6 "Deferred"). Not applied in `dispatch`.
- **Real MCP integration.** `register_dyn` + `ToolKind::Other` are the *seam*;
  actual server discovery, dynamic schema fetch, and the `mcp__*` naming
  convention are deferred (todo "Deferred" line). A `FakeMcpTool` test proves the
  seam holds.
- **Parallel dispatch / per-file write locks.** The engine dispatches serially in
  v0 (ADR-0005); `dispatch` takes `&self` (shared, `Send + Sync`) so parallel is
  *possible* later, but no concurrency control lives here yet (§8).
- **`ToolNamespace` axis.** Grok carries *two* classification axes — `kind` **and**
  `namespace` (`GrokBuild/Codex/OpenCode/MCP`). We ship only `kind`; the
  per-run harness is a single scalar on the `Report`, so the namespace axis is
  not needed yet (§5.2, §8).
- **`effective_tool_name` / meta-tools.** Grok's `ToolRunResult` carries an
  `effective_tool_name` for meta-tools that dispatch to another tool
  (`use_tool` → `linear__save_issue`); we drop it (§5.4, §8).

---

## 2. Module layout (`crates/locode-tools/src/`, as built)

```
lib.rs        Crate docs; module wiring; the public re-export surface.
tool.rs       `ToolOutput`, `ToolKind` (+ `as_str`), the `Tool` trait.
ctx.rs        `ToolCtx` (the small per-call context) + `ToolCtx::new`.
error.rs      `ToolError { Respond, Fatal }` (thiserror).
registry.rs   `ToolRunResult`, the `DynTool` trait, the `TypedTool<T>` adapter,
              `Dispatched`, the `Registry`, `dispatch`, and the free
              `ok_result`/`error_result`/`record` helpers. Tests are inline in
              `lib.rs`'s `#[cfg(test)] mod tests`.
```

Public surface (`lib.rs:18-22`):
```rust
pub use ctx::ToolCtx;
pub use error::ToolError;
pub use locode_protocol::ToolSpec;                       // re-export, not re-defined
pub use registry::{Dispatched, DynTool, Registry, ToolRunResult};
pub use tool::{Tool, ToolKind, ToolOutput};
```

Note two seam facts encoded by the module split:
- `ToolSpec` is **re-exported from `locode-protocol`, not defined here**
  (`lib.rs:20`). It lives in protocol because both `locode-tools` (builds it via
  `specs()`) and `locode-provider` (consumes it in `ConversationRequest`) need
  it, and the dep graph forbids `provider → tools` (`locode-protocol/src/lib.rs:236`
  doc comment; todo Task 5 note). `Registry::specs()` is the only producer.
- All tests are inline in `lib.rs` (`#[cfg(test)] mod tests`, `lib.rs:24-257`),
  which lets them exercise the *private* `TypedTool` adapter path via the public
  `register` while also implementing `DynTool` directly (the MCP seam).

Cargo deps (see §7): `locode-protocol`, `serde`, `serde_json`, `async-trait`,
`schemars` (v1), `thiserror` (v2), `tokio-util`; dev-dep `tokio` (`macros`,`rt`).

---

## 3. Key types & signatures (actual, quoted from source)

### 3.1 `ToolOutput` — the model-facing face (`tool.rs:19-22`)
```rust
pub trait ToolOutput {
    /// Render this output as the text the model reads back in the transcript.
    fn to_prompt_text(&self) -> String;
}
```
The dual-face rule (ADR-0003): the structured `Tool::Output` value goes into the
report's `tool_calls[]`; `to_prompt_text()` renders the text the model reads in
history. They are *independent renderings of one call* — e.g. a read tool reports
`{path, lines, truncated}` structurally but shows the file body as text.

### 3.2 `ToolKind` — the cross-pack tag (`tool.rs:34-68`)
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Shell, Read, Write, Edit, Glob, Grep,
    #[serde(other)]
    Other,
}
impl ToolKind {
    pub fn as_str(self) -> &'static str { /* "shell"|"read"|…|"other" */ }
}
```
`as_str` is the stable key that lands in the report record; a test asserts it
matches the serde form for every variant (`lib.rs:243-256`).

### 3.3 The `Tool` trait (`tool.rs:77-109`)
```rust
#[async_trait]
pub trait Tool: Send + Sync {
    type Args: DeserializeOwned + JsonSchema + Send;
    type Output: Serialize + ToolOutput + Send;

    fn kind(&self) -> ToolKind;
    fn description(&self) -> &str;

    #[must_use]
    fn parameters_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(Self::Args))
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
    }

    async fn run(&self, ctx: &ToolCtx, args: Self::Args) -> Result<Self::Output, ToolError>;
}
```
Note there is **no `name()`** — the wire name is assigned at registration
(§5.3). `parameters_schema` is *defaulted* (derived from `Args`) and "should
rarely be overridden" (doc, `tool.rs:92`); the `unwrap_or_else` degrades to an
empty object rather than `unwrap` (clippy `unwrap_used` is denied workspace-wide).

### 3.4 `ToolCtx` — the small per-call context (`ctx.rs:14-25`)
```rust
#[derive(Debug, Clone)]
pub struct ToolCtx {
    pub cwd: PathBuf,
    pub call_id: String,          // the tool_use id → the pairing link (ADR-0004)
    pub workspace_root: PathBuf,  // the path-jail root (ADR-0008); host resolves under this
    pub cancel: CancellationToken,
}
```
`ToolCtx::new(cwd, call_id, workspace_root, cancel)` builds one per call. The
loop sets `call_id` to the `tool_use` id so the produced `tool_result` pairs
back (`ctx.rs:9-13`).

### 3.5 `ToolError` (`error.rs:12-21`)
```rust
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("{0}")] Respond(String),  // soft → is_error tool_result; loop iterates
    #[error("{0}")] Fatal(String),    // hard → abort the turn, non-zero exit
}
```

### 3.6 `ToolRunResult` — the two faces after erasure (`registry.rs:31-37`)
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRunResult {
    pub output: Value,        // → report's tool_calls[]
    pub prompt_text: String,  // → transcript
}
```

### 3.7 `DynTool` — the object-safe erased tool (`registry.rs:47-60`)
```rust
#[async_trait]
pub trait DynTool: Send + Sync {
    fn kind(&self) -> ToolKind;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    async fn call(&self, ctx: &ToolCtx, raw_args: Value) -> Result<ToolRunResult, ToolError>;
}
```
The registry stores `Box<dyn DynTool>`. Note the type-erasing shift: `Tool` has
associated types `Args`/`Output`; `DynTool` speaks only `Value` in and
`ToolRunResult` out, which is what makes it object-safe.

### 3.8 The `TypedTool<T>` adapter (`registry.rs:62-96`) — a wrapper, not a blanket impl
```rust
struct TypedTool<T: Tool>(T);

#[async_trait]
impl<T: Tool> DynTool for TypedTool<T> {
    fn kind(&self) -> ToolKind { self.0.kind() }
    fn description(&self) -> &str { self.0.description() }
    fn parameters_schema(&self) -> Value { self.0.parameters_schema() }

    async fn call(&self, ctx: &ToolCtx, raw_args: Value) -> Result<ToolRunResult, ToolError> {
        let args: T::Args = serde_json::from_value(raw_args)
            .map_err(|e| ToolError::Respond(format!("invalid arguments: {e}")))?;
        let output = self.0.run(ctx, args).await?;
        let prompt_text = output.to_prompt_text();
        let output = serde_json::to_value(&output)
            .map_err(|e| ToolError::Fatal(format!("failed to serialize tool output: {e}")))?;
        Ok(ToolRunResult { output, prompt_text })
    }
}
```

### 3.9 `Dispatched` — the outcome of one call (`registry.rs:104-112`)
```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Dispatched {
    pub tool_result: ContentBlock,       // ALWAYS present, paired to call_id
    pub record: ToolCallRecord,          // the report-side view
    pub fatal: Option<String>,           // Some iff ToolError::Fatal: append, then abort
}
```

### 3.10 `Registry` (`registry.rs:116-216`)
```rust
#[derive(Default)]
pub struct Registry { tools: HashMap<String, Box<dyn DynTool>> }

impl Registry {
    pub fn new() -> Self;
    pub fn register<T: Tool + 'static>(&mut self, name: impl Into<String>, tool: T);   // → register_dyn(TypedTool(tool))
    pub fn register_dyn(&mut self, name: impl Into<String>, tool: Box<dyn DynTool>);   // MCP/dynamic door
    pub fn contains(&self, name: &str) -> bool;
    pub fn names(&self) -> impl Iterator<Item = &str>;
    pub fn specs(&self) -> Vec<ToolSpec>;                                              // provider-neutral, unordered
    pub async fn dispatch(&self, name: &str, raw_args: Value, ctx: &ToolCtx) -> Dispatched;
}
```
Both `register*` panic on a duplicate name (`registry.rs:144-148`). `dispatch`
takes `&self` (so the registry is shareable across a batch) and is `async`.

---

## 4. Behavior: erasure, dispatch algorithm, and every edge case

### 4.1 Registration (`registry.rs:133-149`)
`register<T: Tool>(name, tool)` wraps `tool` in `TypedTool(tool)`, boxes it, and
delegates to `register_dyn(name, Box::new(TypedTool(tool)))`. `register_dyn`
`name.into()`s the key, **asserts** it is not already present (panic message
`"duplicate tool registration for name \`{name}\`"`), then inserts. The panic is
deliberate: a duplicate wire name is a **startup wiring bug**, not runtime input
(§5.6). Everything downstream keys off the `HashMap<String, Box<dyn DynTool>>`.

### 4.2 The erasure boundary (`TypedTool::call`, `registry.rs:83-95`)
One `DynTool::call` does the whole typed↔untyped round-trip, in this order:
1. **Decode** `raw_args: Value` → `T::Args` via `serde_json::from_value`. On
   failure → `ToolError::Respond("invalid arguments: …")` — **soft** (ADR-0004):
   the model reads the decode error and retries. This is the single most common
   real-world failure and it must never crash the loop.
2. **Run** `self.0.run(ctx, args).await` → `T::Output` (or propagate the tool's
   own `ToolError`).
3. **Render** the prompt text *before* consuming the value:
   `output.to_prompt_text()`.
4. **Serialize** the structured `output` → `Value`. A serialize failure here is
   `ToolError::Fatal` — a `Serialize` type that cannot serialize is a programming
   error in the tool, not something the model can fix by retrying. (This is the
   one place a `Fatal` originates *inside* the framework rather than a tool body.)
5. Return `ToolRunResult { output, prompt_text }`.

The ordering (render text, *then* serialize to `Value`) matters: both faces are
produced from the one `Output` value without cloning it.

### 4.3 The dispatch door (`registry.rs:182-215`)
`dispatch(name, raw_args, ctx)` is the single funnel (ADR-0008). It snapshots
`ctx.call_id` as the pairing `id`, then:

1. **Unknown tool** (`registry.rs:185-192`): if `self.tools.get(name)` is `None`,
   return a `Dispatched` with an `is_error` `tool_result` (`"unknown tool: {name}"`),
   a `record` with `kind = ToolKind::Other`, `ok = false`, `output = Value::Null`,
   and `fatal = None`. **Soft** — the model can pick a real tool and retry.
2. **Known tool**: read `kind = tool.kind()` up front (so it is available even on
   error paths), then `match tool.call(ctx, raw_args.clone()).await`:
   - `Ok(ToolRunResult { output, prompt_text })` → `tool_result = ok_result(id,
     prompt_text)`, `record(id, name, kind, args, ok=true, output)`, `fatal=None`.
   - `Err(ToolError::Respond(msg))` → `error_result(id, &msg)`,
     `record(… ok=false, output=Null)`, `fatal=None`. Loop continues.
   - `Err(ToolError::Fatal(msg))` → `error_result(id, &msg)`,
     `record(… ok=false, output=Null)`, **`fatal = Some(msg)`**. The result is
     *still paired* — the loop appends it, keeping the transcript valid, then aborts.

`raw_args` is cloned once (`raw_args.clone()` at `registry.rs:195`) so the
original can go into the `record.args` regardless of success/failure — the report
faithfully records *what the model asked for* even when the call failed.

### 4.4 The three block constructors (`registry.rs:219-255`)
- `ok_result(id, text)` → `ContentBlock::ToolResult { tool_use_id, content:
  vec![ResultChunk::Text{text}], is_error: false }`.
- `error_result(id, message)` → same shape with `is_error: true`.
- `record(id, name, kind, args, ok, output)` → `ToolCallRecord { id, name,
  kind: kind.as_str().to_owned(), args, ok, output }`.

Every path produces **both** a `ContentBlock` (history) and a `ToolCallRecord`
(report) — the two views never diverge in existence, only in content.

### 4.5 Edge-case ledger

| Condition | Handling | `fatal`? | Cite |
|---|---|---|---|
| Args decode fails | `Respond("invalid arguments: …")` → `is_error` result | none | `registry.rs:85-86`; test `lib.rs:150-162` |
| Unknown tool name | soft `is_error`, `record.kind = "other"` | none | `registry.rs:185-192`; test `lib.rs:164-176` |
| Tool returns `Respond` | `is_error` result, `record.ok=false` | none | `registry.rs:204-208` |
| Tool returns `Fatal` | `is_error` result **still paired**, then abort | `Some(msg)` | `registry.rs:209-213`; test `lib.rs:178-196` |
| Output serialize fails | framework raises `Fatal` | `Some(msg)` | `registry.rs:89-90` |
| Duplicate wire name | **panic at registration** (startup) | — | `registry.rs:144-148`; test `lib.rs:198-204` |
| `register_dyn` MCP-style tool | stored + dispatched identically to typed | per its result | test `lib.rs:229-241` |
| Successful call | `ok_result` + `record.ok=true` + structured `output` | none | test `lib.rs:122-148` |

The invariant the whole taxonomy protects: **`dispatch` always returns a paired
`tool_result`** — success, soft error, unknown tool, *and* fatal. The `fatal`
flag is out-of-band signalling to the loop ("append this, then stop"), never an
excuse to skip the pairing. This is exactly the ADR-0004 posture: transcript
validity is independent of how a call ended.

---

## 5. Design decisions (each: harness `file:line` · why · why-not · harness diff)

### 5.1 Derive the schema from `Args`, never hand-write it
- **Source.** Grok derives every tool's arg schema from the typed `Args` at
  registration: `input_schema: generate_schema::<T::Args>()`
  (`grok/registry/types.rs:595`), with `T::Args: … + schemars::JsonSchema`
  (`:545, :567`). Codex, by contrast, carries a *hand-typed* `JsonSchema` struct
  for function tools — `ToolSpec::Function(ResponsesApiTool { … parameters:
  JsonSchema … })` (`codex/tools/src/tool_spec.rs:17-26`) — and its own
  `codex-rs/protocol` builds those by hand.
- **Why.** ADR-0003's headline: "the model's tool spec and the code that executes
  the tool must not drift … the single most common source of tool bugs." Our
  default `parameters_schema()` calls `schemars::schema_for!(Self::Args)`
  (`tool.rs:98`), so adding a tool = define `Args`/`Output` + `run`, and the
  schema follows for free (ADR-0003 Consequences). A test freezes this:
  `schema_is_derived_from_args` asserts the derived schema describes
  `message: String` and marks it required (`lib.rs:111-120`).
- **Why not hand-written (Codex style).** Rejected in ADR-0003: the spec and
  handler drift the moment someone edits one and not the other. Codex accepts the
  hand-written form because it targets a specific wire's `parameters` shape; we
  are provider-neutral (`ToolSpec.parameters` is a generic `Value`, mapped
  per-wire later), so deriving costs us nothing and buys drift-freedom.
- **Harness diff.** Grok = schemars-derived (we match it); Codex = hand-typed
  `JsonSchema`; our default *is* Grok's approach, with an override hook
  (`parameters_schema` is a provided method) for the rare dynamic case.

### 5.2 `ToolKind` = a cross-pack tag, closed + `Other`, no wire meaning
- **Source.** Grok's real `ToolKind` (`grok/types/tool.rs:70-105`) is a large
  fixed enum (`Read, Edit, Delete, ListDir, Write, Move, Search, Lsp, Execute,
  Plan, WebSearch, …` — 30-plus variants) terminated by `#[serde(other)] Other`
  (`:103-104`), serialized `snake_case`. Its doc says `Other` is both the default
  *and* the `#[serde(other)]` sink "so a consumer pinned to an older schema
  deserializes a newer `kind` to `Other` instead of erroring" (`:50-54`).
- **Why.** ADR-0003 makes canonical identity a `ToolKind` enum "distinct from the
  client-facing wire name." Its only purpose is **A/B alignment across harness
  packs** (todo Task 4 design note; `tool.rs:24-33`): so a report can compare
  "grok's `read_file`" against "codex's `read`" as the same *kind*. It lands in
  the report as `ToolCallRecord.kind` (a `String`, `protocol/src/lib.rs:193`). We
  start with the six kinds our v0 grok pack needs (`Shell/Read/Write/Edit/Glob/
  Grep`) and grow the canonical set as packs need it — exactly Grok's "fixed set
  plus `Other`" shape, at a fraction of the size.
- **Why not the Rust type name or the wire name.** Type names are incidental
  (`GrokReadFile`) and invisible to the model; wire names are pack-specific
  (`read_file` vs `read`) so they can't align *across* packs. `ToolKind` is the
  third, stable axis (`registry.rs:5-15` "three distinct names").
- **Why `#[serde(other)]`.** Forward-compat on deserialize (a newer `kind`
  degrades to `Other` instead of failing the parse) *and* a home for tools with
  no cross-pack analog — MCP tools and harness-unique specials (`tool.rs:49-51`).
- **Harness diff.** Grok additionally carries a **second** axis, `ToolNamespace`
  (`GrokBuild/GrokBuildConcise/Codex/OpenCode/MCP`, `grok/types/tool.rs:33-46`),
  qualifying every id as `"GrokBuild:read_file"`. We deliberately ship only the
  `kind` axis: only one pack is active per run, and the harness is a single
  scalar on the `Report`, so a per-tool namespace tag is redundant in v0 (§8
  revisits whether A/B ever needs it).

### 5.3 The tool has no `name()` — the wire name is assigned at registration
- **Source.** Grok builds the model-facing name *at register time*, from the
  namespace + the tool's own id: `name = format!("{}:{}", tool.tool_namespace(),
  Tool::id(&tool))` (`grok/registry/types.rs:578-582`), and its client-facing
  name is resolvable/overridable per config (`resolve_client_name`,
  `:832`; `params_name_overrides`, `:60`). The tool struct carries an *id*, but
  the *registry key* is composed by the registry.
- **Why.** A tool has **three names, do not conflate them** (`registry.rs:5-15`,
  todo Task 4 design note): the Rust type name (compile-time only), the **wire
  name** (what the model calls = the registry key, assigned by the pack), and the
  `ToolKind` tag. Because the pack owns wire-name assignment (Task 8), putting a
  `name()` on `Tool` would (a) duplicate/contradict the registry key, and (b)
  stop a pack from registering the same tool type under a different name. So
  `register(name, tool)` takes the name as a parameter (`registry.rs:133`) and
  `Tool` stays name-less.
- **Why not a `name()` method.** It would make the type the authority on its wire
  name; but the *pack* is the authority (ADR-0012), and one pack is active per
  run so there is no cross-pack key collision to arbitrate — only duplicates
  *within* a pack, which panic (§5.6).
- **Harness diff.** Grok composes `namespace:id` and supports client-name
  overrides; Codex keys tools by the function name in the spec. We take the name
  as an explicit registration argument — the simplest form that keeps the pack in
  control.

### 5.4 Dual output: structured `output` + `prompt_text`
- **Source.** Grok's `ToolRunResult` is "the **single return type** from
  `ToolRunner::run()`" carrying `output` ("never mutated by layers; for JSON
  serialization, protocol translation") and `prompt_text` ("rendered with system
  reminders appended; for model prompt") — `grok/types/output.rs:128-145`; the
  bridge splits the same two fields to "build the model prompt (from
  `prompt_text`)" (`grok/bridge.rs:28-44`).
- **Why.** ADR-0003: "a tool result serves two different readers: the host/JSON
  report wants **structured data**, the model wants **rendered text**." Collapsing
  them loses information in one direction (a read tool's `{path, lines, truncated}`
  vs the file body). Our `TypedTool::call` produces both from one `Output`
  (`registry.rs:87-90`), and `dispatch` routes `prompt_text` → the `tool_result`
  and `output` → the `ToolCallRecord` (`registry.rs:196-202`). Test
  `echo_round_trips_output_and_prompt_text` asserts the two faces independently
  (`lib.rs:122-148`).
- **Why not a single value stringified for the model.** Rejected in ADR-0003:
  either the report loses the text rendering or the model loses the structure.
- **Harness diff.** Grok's `ToolRunResult` additionally carries
  `effective_tool_name: Option<String>` for meta-tools that dispatch onward
  (`grok/types/output.rs:141-144`); we drop it (no meta-tools in v0, §8). Codex
  keeps output typed via a separate `tool_output`/payload path
  (`codex/tools/src/tool_output.rs`); the two-face split is the shared idea.

### 5.5 `TypedTool<T>` adapter, **not** a blanket `impl<T: Tool> DynTool for T`
- **Source.** Grok erases typed tools into boxed closures stored in a `ToolEntry`:
  `output_converter: Box::new(|value| { let typed: T::Output =
  serde_json::from_value(value)?; Ok(typed.into()) })` and `parse_input:
  Box::new(|json| … serde_json::from_value::<T::Args>(json)? …)`
  (`grok/registry/types.rs:597-615`). Erasure happens *at registration*, and the
  stored entry speaks only JSON — structurally the same move as our `TypedTool`.
- **Why a wrapper.** Doc, `registry.rs:62-67`: "A wrapper (rather than a blanket
  `impl<T: Tool> DynTool for T`) is deliberate: the blanket form would forbid any
  manual `impl DynTool for McpTool` under Rust's coherence rules, closing the MCP
  seam." A blanket impl over *all* `T: Tool` plus a hand impl for a specific
  `McpTool` would be two overlapping impls of `DynTool`; Rust's coherence
  (orphan/overlap) rejects that. The wrapper keeps `TypedTool<T>: DynTool` and
  leaves `impl DynTool for McpTool` open. `register` uses the wrapper
  (`registry.rs:134`); MCP tools use `register_dyn` directly.
- **Why not the blanket impl.** It reads cleaner but permanently forecloses
  hand-written `DynTool`s — precisely the dynamic-schema tools (MCP) we must keep
  room for.
- **Harness diff.** Grok erases via per-tool boxed closures inside `ToolEntry`
  (no `DynTool` trait object — the closures *are* the vtable); we use one
  object-safe trait (`DynTool`) with a generic adapter. Both reach the same
  "stored thing speaks JSON only" end state; ours keeps a nameable trait so a
  non-derived tool can implement it directly.

### 5.6 Duplicate name → **panic** (startup wiring bug, not runtime input)
- **Source.** Grok treats duplicate *client* names as a **validation error**
  surfaced at build time: a `duplicate_client_name` `RequirementError` ("already
  used by {prev_id}. Use name_override …") collected before the registry is used
  (`grok/registry/types.rs:829-851`). Its low-level `register_with_params` just
  `self.tools.insert(name, …)` (`:587`) — last-writer-wins at the map level, with
  the collision caught by the separate validation pass.
- **Why panic.** todo Task 4 design note: "one pack is active per run, so no
  cross-pack key collision — only duplicates within a pack panic." A duplicate
  wire name can only come from *pack wiring code* (Task 8), which runs at startup
  before any model input — so `assert!` + panic (`registry.rs:144-148`) fails
  fast and loud at exactly the moment a human can fix it, and it can never be
  triggered by the model. Test `duplicate_registration_panics` freezes this
  (`lib.rs:198-204`).
- **Why not a silent overwrite (Grok's raw `insert`) or a `Result`.** Silent
  overwrite hides a real bug (two tools fighting for one name). A `Result` would
  push a can't-happen-at-runtime error into every caller's signature; panic keeps
  `register` ergonomic and the failure impossible to ignore.
- **Harness diff.** Grok = collect-and-report validation error (it has a config
  layer and user-supplied `name_override`s to reconcile); we = immediate panic
  (our names are hardcoded pack wiring, so there is nothing to reconcile).

### 5.7 Soft-by-default errors: unknown tool + bad args are `Respond`, not `Fatal`
- **Source.** Codex's `FunctionCallError { RespondToModel(String), Fatal(String) }`
  (`codex/tools/src/function_call_error.rs:5-9`) is the exact two-way split we
  copy into `ToolError` (`error.rs:12-21`). ADR-0004 cites it directly.
- **Why.** ADR-0004: "**Default everything to `Respond`** — bad args, unknown
  tool, not-found, command failure, timeout … all become a `tool_result{is_error:
  true}` the model can recover from. Reserve `Fatal` for 'the transcript is
  unrecoverable.'" So `dispatch` makes an unknown tool soft (`registry.rs:186-191`)
  and `TypedTool::call` makes a decode failure soft (`registry.rs:85-86`); tests
  `unknown_tool_is_soft` and `bad_args_are_soft` freeze it (`lib.rs:150-176`).
  This keeps the loop *productive*: a mis-called tool returns prose telling the
  model how to fix it, rather than crashing the run.
- **Why not "any error aborts."** Rejected in ADR-0004: it throws away the model's
  ability to self-correct and makes the agent brittle. `Fatal` is rare by design.
- **Harness diff.** Same two-variant split as Codex. Grok expresses recoverability
  differently (its `ToolError` + higher-level `ToolLoop` variants), but the "most
  failures are just data the model reads" posture is shared across all studied
  harnesses (ADR-0004 Context).

### 5.8 `dispatch` always returns a paired `tool_result`, with an out-of-band `fatal` flag
- **Source.** ADR-0004's second invariant: providers "reject the entire request if
  a `tool_use` has no `tool_result`." All studied harnesses spend real code
  guarding this; Grok exposes reusable repair/dedup helpers (cited in the Task 6
  plan, `grok …/conversation.rs`).
- **Why.** `Dispatched` carries `tool_result` (always), `record` (always), and
  `fatal: Option<String>` (`registry.rs:104-112`). Even the `Fatal` arm builds an
  `error_result` *before* setting `fatal = Some(msg)` (`registry.rs:209-213`), so
  the loop can append the paired result and *then* abort — the transcript is valid
  regardless of outcome. Test `fatal_sets_flag_and_still_pairs` asserts the paired
  result exists on the fatal path (`lib.rs:178-196`). This is why `fatal` is a
  side-channel flag, not a `Result<Dispatched, _>`: a fatal call still has a
  result to append.
- **Why not `dispatch -> Result<Dispatched, Fatal>`.** That shape tempts the
  caller to drop the paired result on the error path, breaking pairing — the exact
  bug ADR-0004 exists to prevent. Returning `Dispatched` unconditionally makes the
  right thing (append, then check `fatal`) the *only* thing.
- **Harness diff.** Grok/Claude synthesize missing results via explicit
  repair passes at the loop level; we make the *door itself* structurally unable
  to return an unpaired result. The loop's own mid-batch synthesis (Task 6) sits
  on top.

### 5.9 `ToolCtx` stays small (`{cwd, call_id, workspace_root, cancel}`) + `CancellationToken`
- **Source.** Both harnesses thread `tokio_util::sync::CancellationToken` for
  cooperative cancellation: Grok stores `scheduler_cancel:
  Option<tokio_util::sync::CancellationToken>` and mints tokens per run
  (`grok/registry/types.rs:450, :1176`); Codex uses `tokio_util::sync::
  CancellationToken` throughout core (`codex/core/src/{exec,client}.rs`). ADR-0003
  explicitly contrasts Claude Code's ~40-field `ToolUseContext` god-object.
- **Why.** ADR-0003 "Keep `ToolCtx` small: `{ cwd, call_id, workspace_root,
  cancel }`." Just what a tool call needs: where to run (`cwd`), what it answers
  (`call_id` → the pairing link), the jail root the host resolves under
  (`workspace_root`), and a cooperative `cancel` a long-running tool should
  observe (kill its subprocess, stop reading) — `ctx.rs:7-25`. Choosing the
  *standard* `tokio_util` token (not a bespoke channel) means the future host,
  engine, and any parallel executor already speak the same cancellation type both
  harnesses use.
- **Why not a fat context.** ADR-0003 Consequences: "A god-object context
  (Claude's ~40-field `ToolUseContext`) is explicitly avoided" — it couples every
  tool to a huge surface and makes the contract untestable.
- **Note (as built).** In v0 the token is *plumbed but never fired* (todo Task 6
  "live cancellation reserved"); the `Fatal` path, not a live cancel, exercises
  mid-batch abort. `workspace_root` is likewise carried but not yet *enforced* —
  the host (Task 7) will resolve/jail against it.

---

## 6. Tests (as built, inline in `lib.rs:24-257`)

Two in-test tools model the two success/failure shapes, mirroring the engine's
later `Echo`/`Boom`: `Echo` (`Args = EchoArgs{message}`, `Output = EchoOut`,
`ToolOutput` returns the echoed string, `kind = Shell`) and `Boom`
(`run` always `Err(ToolError::Fatal)`). A `FakeMcpTool` implements `DynTool`
*directly* (no compile-time `Args`) for the MCP seam.

| # | Test (`lib.rs`) | Proves |
|---|---|---|
| 1 | `schema_is_derived_from_args` (`:111`) | `parameters_schema()` reflects `Args` — `message` typed `string`, listed `required` (§5.1). |
| 2 | `echo_round_trips_output_and_prompt_text` (`:122`) | Both faces: `tool_result` carries `prompt_text` paired to `call_id`, `record` carries structured `output`, `kind="shell"`, `ok=true`, `fatal=None` (§5.4). |
| 3 | `bad_args_are_soft` (`:150`) | Missing `message` → `is_error` result, `record.ok=false`, `fatal=None` — not a panic/fatal (§5.7). |
| 4 | `unknown_tool_is_soft` (`:164`) | Unknown name → `is_error`, `record.kind="other"`, `fatal=None` (§5.7). |
| 5 | `fatal_sets_flag_and_still_pairs` (`:178`) | `Fatal` → `tool_result` **still paired** to id + `is_error`, and `fatal=Some("unrecoverable")` (§5.8). |
| 6 | `duplicate_registration_panics` (`:198`) | Second `register("echo", …)` panics `"duplicate tool registration"` (§5.6). |
| 7 | `register_dyn_supports_mcp_like_tools` (`:229`) | A hand-written `DynTool` registers via `register_dyn`, dispatches, `kind="other"`, round-trips its raw args (§5.5). |
| 8 | `tool_kind_key_matches_serde` (`:243`) | `ToolKind::as_str()` equals the serde form for every variant (report-key stability). |

This is exactly the todo Task 4 verification list (`todo.md:88-89`): schema
derived; bad-args soft; echo round-trips; duplicate panics; unknown soft; fatal
flags + pairs; MCP `register_dyn` works. Coverage note: the *framework*-raised
`Fatal` on output-serialize failure (`registry.rs:89-90`) is exercised only
indirectly (all in-test `Output`s serialize); a dedicated test could pin it (§8).

---

## 7. Dependencies (as built)

No dependency was novel to the workspace — each was already vendored or is the
canonical crate for its job. The versions are pinned in the workspace root
`Cargo.toml` and referenced with `workspace = true` (`locode-tools/Cargo.toml:11-21`).

| Dep | Version (`Cargo.toml`) | Why · precedent |
|---|---|---|
| `locode-protocol` | path | `ContentBlock`, `ResultChunk`, `ToolCallRecord`, `ToolSpec` — the shapes `dispatch`/`specs` produce; `ToolSpec` re-exported from here (dep graph forbids `provider → tools`). |
| `serde` / `serde_json` | workspace | `Value` in/out of the erasure boundary; `Serialize`/`Deserialize` on `ToolKind`; report (de)serialization. |
| `async-trait` (`0.1`) | workspace | `Tool::run` and `DynTool::call` are `async fn` in a trait → needs `#[async_trait]` for object safety on `DynTool`. Both harnesses' tool traits are async (Grok's `Reminder`/`Tool`, `grok/types/tool.rs:124`; Codex's executor). |
| `schemars` (`1`) | workspace | `schema_for!(Self::Args)` derives the arg JSON Schema (§5.1). **Grok uses schemars for the identical purpose** — `T::Args: … schemars::JsonSchema`, `generate_schema::<T::Args>()` (`grok/registry/types.rs:545, :595`). SPEC Tech-Stack names schemars. |
| `thiserror` (`2`) | workspace | Derives `ToolError`'s `Display` (`error.rs:12`). **Codex derives its `FunctionCallError` with `thiserror`** too (`codex/tools/src/function_call_error.rs:1-4`). SPEC Tech-Stack names thiserror for errors. |
| `tokio-util` (`0.7`) | workspace | `CancellationToken` for `ToolCtx.cancel` (§5.9) — the exact type Grok (`grok/registry/types.rs:450`) and Codex (`codex/core/src/exec.rs`) both use. |
| `tokio` (dev, `macros`+`rt`) | workspace | `#[tokio::test]` for the async dispatch tests only. |

Per the todo Task 4 note, these are "ADR-0003 alignment + Codex/Grok precedent."
None trips the "Ask first: adding a dependency" boundary in a novel way — they
are the crates SPEC's Tech-Stack already commits to, at workspace-pinned
versions.

---

## 8. Open questions, concerns & future considerations (exhaustive & honest)

Ordered roughly by when they'll bite.

1. **Parallel dispatch + per-file write locks.** `dispatch(&self, …)` is already
   `Send + Sync`-friendly, so the engine *could* run a batch on a
   `FuturesUnordered` later — but there is **no concurrency control in this
   crate**. When parallel lands (ADR-0005 reserved seam), *where* does the write
   lock live? Grok keys a per-file mutex on a single path arg
   (`grok …/tool_calls.rs`) and that keying **misses** multi-file/`apply_patch`
   ops; Codex uses a coarse read/write lock split. The lock is inherently
   *pack-and-tool-specific* (it needs to know which arg is "the path"), so it may
   belong in the pack or host, not in `locode-tools`. Unresolved: should `Tool`
   grow a `fn write_targets(&self, &Args) -> Vec<PathBuf>` (or a `parallel_safe()`
   marker) so the executor can build the lock set generically? That would touch
   the trait signature (an "Ask first" boundary).

2. **`ToolKind` vocabulary growth + whether to add the `ToolNamespace` axis.** Six
   kinds cover v0's grok pack; the moment a second pack (codex/claude/opencode)
   lands with tools that have no analog (`apply_patch`, `todo`, `web_search`,
   `lsp`), the canonical set must grow — and every growth is an *additive*
   `ToolKind` variant plus an `as_str` arm (mechanical, but it re-derives the
   report shape). Grok needed 30+ (`grok/types/tool.rs:70-105`). Two sub-questions:
   (a) do we grow toward Grok's vocabulary or keep a minimal A/B-only set? (b) does
   honest cross-pack A/B ever need Grok's **second** axis, `ToolNamespace`? Today
   the harness is a single `Report` scalar so we don't — but if one run ever mixes
   packs (e.g. a base pack + MCP tools), a per-tool namespace tag would matter.

3. **Real MCP integration mechanics + dynamic schemas.** The *seam*
   (`register_dyn` + `DynTool` + `ToolKind::Other`) is proven by `FakeMcpTool`,
   but real MCP needs: server discovery/handshake, fetching each tool's
   `input_schema` at runtime (Codex does exactly this — builds the spec from the
   server's `input_schema`, `codex/tools/src/mcp_tool.rs:7-31`,
   `dynamic_tool.rs:11`), the `mcp__<server>__<tool>` naming convention, per-server
   auth/lifecycle, and error mapping (an MCP transport error → `Respond` vs
   `Fatal`?). None of that lives here; `register_dyn` is only the front door. Open:
   does the dynamic schema need validation/normalization before it hits the wire
   (Codex injects a missing `"properties"` for OpenAI, `mcp_tool.rs:9-21`)?

4. **Per-tool policy / timeout / sandbox behind the door.** ADR-0008's whole point
   is "add policy in one place." But `dispatch` in v0 is *pure routing* — it does
   **not** consult any policy, enforce the `workspace_root` jail, apply a timeout,
   or sandbox. Those are promised to `locode-host` (Task 7) and are invoked *by
   tool bodies*, which means the "one door" today enforces nothing. Concern: is
   the decision gate really at `dispatch`, or does it de-facto scatter into each
   tool's host calls? A cleaner design might pass a `Policy`/`Host` handle through
   `ToolCtx` and check it *in* `dispatch` before `tool.call`. Deferred, but the
   "one place" promise is only as good as where the gate actually lands.

5. **Where output truncation applies.** `truncate_for_model` (ADR-0008: "a shared
   post-process applied before the model re-enters, not per-tool ad hoc") is
   **not** in `dispatch`. Today `prompt_text` flows verbatim into the
   `tool_result`. When it lands (Task 7), does it wrap `dispatch` (so the door
   owns it, honoring ADR-0008's "shared post-process"), or does the engine apply
   it between `dispatch` and append? The `Dispatched.tool_result` is already
   built by the time the engine sees it, so truncating there means *rebuilding*
   the block — an argument for doing it inside/around the door instead.

6. **`Fatal`-on-serialize is untested + arguably too harsh.** `TypedTool::call`
   raises `Fatal` if `serde_json::to_value(&output)` fails (`registry.rs:89-90`).
   That aborts the whole run for what is a single tool's bug. Is a non-serializable
   `Output` really "the transcript is unrecoverable," or should it be a `Respond`
   ("that tool is broken, try another")? It is also the one framework-internal
   `Fatal` with no dedicated test (§6). Low-risk (a `Serialize` type that fails is
   rare), but worth a decision + a test.

7. **`ToolOutput::to_prompt_text(&self) -> String` allocates unconditionally.** For
   a large read/grep result the model-facing text is materialized as an owned
   `String` even before truncation (§5). Fine at v0 scale; a future concern for
   huge outputs (a `Cow<str>` or a writer-based rendering would avoid the copy).
   Interacts with truncation (#5): truncating *before* rendering would be cheaper
   than rendering-then-truncating.

8. **`specs()` is provider-neutral (`ToolSpec`) — is that the right altitude?**
   `specs()` returns `Vec<ToolSpec>{name, description, parameters}`
   (`registry.rs:164-173`) and each wire maps it to its own tool format later
   (Anthropic `input_schema` vs OpenAI `function`). This is deliberately *not*
   wire-shaped (keeps `tools` provider-agnostic). But it also means the schema
   `schemars` emits (draft 2020-12, with `$defs`/`$ref`, `$schema`, `title`) is
   handed to the wire *as-is*; some providers reject `$schema`/`$ref` or want
   inlined defs. Open: does normalization (strip `$schema`, inline `$defs`,
   enforce `additionalProperties:false`) belong in `specs()` (once, neutral) or in
   each wire (per-provider)? Today neither does it. Also unordered
   (`HashMap` iteration) — deterministic ordering may matter for prompt-cache
   stability.

9. **schemars 1 version risk + draft compatibility.** We pin `schemars = "1"`
   (`Cargo.toml:16`); Grok's vendored copy is also v1-era. schemars 1 emits JSON
   Schema **draft 2020-12**. Two live risks: (a) providers that expect draft-07
   `input_schema` (Anthropic historically tolerant, OpenAI stricter) — feeds #8;
   (b) a schemars major bump could change the emitted shape and silently break the
   golden expectations a future wire test freezes. Worth a wire-level schema
   conformance test once Task 12 lands.

10. **Tool-choice / hosted / server-side tools.** Nothing models "force tool X",
    "auto", or provider-*hosted* tools (Anthropic web-search/computer-use,
    OpenAI's built-ins) that have **no local `run`**. Those aren't `Tool`s at all —
    they're wire directives. Where do they live? Likely `ConversationRequest`/the
    wire, not the `Registry` — but a hosted tool still needs to appear in `specs()`
    and its result still needs a `ToolCallRecord`. Unresolved seam.

11. **`effective_tool_name` / meta-tools dropped.** Grok's `ToolRunResult` carries
    `effective_tool_name` for a meta-tool (`use_tool`, `search_tool`) that
    dispatches to another tool (`grok/types/output.rs:141-144`). We omit it. If we
    ever port Grok's `use_tool`/MCP-router pattern, the report couldn't record
    *which* underlying tool actually ran. Additive to `ToolRunResult`/`Dispatched`
    when needed.

12. **Registration ergonomics vs. a builder.** `register`/`register_dyn` mutate a
    `&mut Registry` and panic on collision — fine for hardcoded pack wiring, but a
    pack that assembles tools from config/MCP at runtime has no non-panicking path
    to "register if free." A `try_register -> Result` (or a validate-then-build
    builder, which is what Grok's collect-errors pass is, `grok/registry/types.rs:
    829-851`) may be wanted once tool sets are data-driven.

13. **`dispatch` clones `raw_args` every call.** `raw_args.clone()`
    (`registry.rs:195`) keeps the original for `record.args`. For large tool
    inputs this is a full JSON deep-clone per call. Acceptable now; a concern under
    parallel batches with big args. Could be avoided by recording args by reference
    or moving the clone into only the paths that need both copies.

### Speech-to-text / identifier confirmations
This plan was written from the shipped source and the written ADRs; **no
user-supplied identifiers were reconstructed**. One name I resolved from context
without asking: the survey's "Grok's real `ToolKind` … same shape (a fixed set
plus `Other`)" referenced in `tool.rs:30-33` maps to
`grok/types/tool.rs:70-105`, and "Codex `RespondToModel`/`Fatal`" maps to
`codex/tools/src/function_call_error.rs:5-9`. Flagging in case either target was
meant differently.
