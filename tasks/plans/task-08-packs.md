# Task 8 — `locode-packs`: harness-pack framework + `grok` pack wiring

> **Resolved (user-confirmed):** the grok pack registers grok's **real** tool names
> (e.g. `run_terminal_cmd`) and **omits a standalone `write`** tool; faithfully mimic Grok
> Build. See `tasks/plans/README.md`.
>
> **Built with one change from §3 below:** `Pack::system_prompt(&PackContext) -> String`
> became **`Pack::preamble(&PackContext) -> Vec<Message>`** (role-tagged `System`/`Developer`),
> so each pack expresses its own role split and the wire places each role. grok's real
> System-vs-Developer/User split is deferred to Task 13 (source: grok has no `Developer`
> role — its base prompt is a `System` item, env is injected as `User` system-reminders).
> Task 8's grok `register` is empty (real tools land Tasks 9-11); the framework is proven
> via a test-local fake pack.

Detailed implementation plan (pre-implementation). Source of truth: `SPEC.md`,
`docs/decisions/ADR-0012-harness-packs.md` (supersedes ADR-0006), `tasks/plan.md`,
`tasks/todo.md` (Task 8). Aligns to the already-landed `locode-tools` and
`locode-protocol` types. Cites the studied harness source by `file:line`; grok source
paths are under
`~/dev/coding-cli-survey/submodules/grok-build/crates/codegen/xai-grok-tools/`.

---

## 1. Purpose & scope

### What Task 8 delivers
The **pack layer** (ADR-0012): the seam that turns a bag of `Tool` implementations plus
a system prompt into a single, name-selectable **harness pack**, and the `--harness
<name> → pack` resolver. Concretely:

- A `Pack` abstraction: `name` + `register(&mut Registry)` (assigns each tool its
  harness's **real wire name**) + `system_prompt(&PackContext)`.
- A resolver `resolve(name) -> Result<&'static dyn Pack, UnknownHarness>` and an
  `available()` list, so `locode-exec` (Task 14) can map `--harness` to a pack and print
  a clear error for an unknown name.
- The `grok` pack **module scaffold** (`grok/mod.rs`) implementing `Pack` — wired but,
  at Task-8 time, holding a **placeholder tool** only (see §4/§8). The real grok tools
  land in Tasks 9–11 and its real prompt in Task 13; each real tool already carries a
  `ToolKind` tag via `Tool::kind()`, so cross-pack A/B alignment needs **no extra pack
  machinery**.

### Why the pack layer exists (ADR-0012)
locode is a **faithful experiment bed**: run one task under a genuine reproduction of
each harness and compare. A pack is therefore a *complete, faithful* toolset — real
implementations, names, schemas, behavior — selected whole via `--harness`, **not** a
re-skin of one canonical tool (ADR-0012 §Decision; supersedes ADR-0006's dialect
re-skin). Fidelity beats DRY. Grok Build is the reference for how a toolset + prompt is
unified into a selectable harness (survey `05-comparative/patterns-matrix.md:19` scores
"Multi-dialect tool packs" ★★★ — **Grok-only among the four**;
`05-comparative/tool-surface.md` "Grok | Full specialized set + dialect packs for
compat").

### In scope for Task 8
Framework types (`Pack`, `PackContext`, `UnknownHarness`), the resolver, the grok module
scaffold + placeholder registration, and unit tests for the framework and resolution.
Depends only on **Task 4** (`locode-tools`) — not on Task 7 (host); the two are
explicitly parallelizable (`tasks/plan.md:112-114`).

### Deferred (explicitly NOT Task 8)
- **Real grok tools** — `run_terminal_cmd`, `read_file`, `search_replace`, `grep`,
  `list_dir` (Tasks 9–11); each is a host-backed `Tool` slice that fills grok's
  `register`.
- **Grok's real system prompt** — minijinja-rendered, headless-branched identity
  (Task 13); Task 8 ships a placeholder `system_prompt` returning a short static string.
- **Other packs** — `codex`, `claude`, `opencode`, and the best-of `locode` pack are
  the **next milestone** (Task 15). Task 8 leaves the resolver trivially extensible
  (one `match` arm + one `static` per pack) but wires only `grok`.
- **`minijinja` dependency** — belongs to Task 13, not Task 8 (keep the prompt a stub).

---

## 2. Module layout

```
crates/locode-packs/
├── Cargo.toml                 → add `thiserror` (workspace); dev-deps for tests
└── src/
    ├── lib.rs                 → crate doc; re-exports; the `resolve`/`available` resolver + UnknownHarness
    ├── pack.rs                → the `Pack` trait, `PackContext`, default `build_registry`
    └── grok/
        └── mod.rs             → `GrokPack` (impl Pack): register() + system_prompt() scaffold
```

Rationale: one module **per harness** (SPEC Project Structure: "one module per harness";
ADR-0012 "a module per harness"). `pack.rs` holds the harness-neutral abstraction;
`grok/mod.rs` is the first concrete pack. Tasks 9–11 add sibling files
`grok/{terminal,read,search_replace,grep,list_dir}.rs` and `grok/mod.rs` grows its
`register` body; Task 13 adds `grok/prompt.rs` + templates. Future packs are new
sibling dirs (`codex/`, `claude/`, …) — no change to `pack.rs` or the resolver shape
beyond added arms.

The existing `crates/locode-packs/src/lib.rs` is a 3-line scaffold doc-comment; it is
replaced by the real `lib.rs` below.

---

## 3. Key types & signatures (Rust sketch)

All sketches align to landed types: `Registry`, `Registry::{register, register_dyn,
dispatch, specs, contains, names}`, `ToolSpec`, `Tool` (no `name()`), `ToolKind`
(`crates/locode-tools/src/{registry,tool}.rs`); `Message`, `Role`
(`crates/locode-protocol/src/lib.rs:33-57`).

### 3.1 `pack.rs` — the abstraction

```rust
//! The harness-pack abstraction (ADR-0012): a named toolset + system prompt that
//! assigns each tool its harness's REAL wire name at registration time.

use locode_tools::Registry;

/// Dynamic, per-run context a pack's system prompt is rendered against.
///
/// Minimal in v0 (Task 8): the fields grok's real prompt needs (Task 13) —
/// cwd/OS/shell/date + the headless identity branch. Deliberately small, like
/// `ToolCtx` (ADR-0003 rejects god-object contexts). NOT the tool-call context.
#[derive(Debug, Clone)]
pub struct PackContext {
    /// Absolute working directory shown to the model.
    pub cwd: std::path::PathBuf,
    /// Target OS label (e.g. "macos").
    pub os: String,
    /// Login shell (e.g. "/bin/zsh").
    pub shell: String,
    /// Current date, preformatted (prompt is a pure fn of context).
    pub date: String,
    /// Headless run → autonomous identity branch (vs interactive). See Task 13.
    pub headless: bool,
}

/// A faithful reproduction of one harness: its real toolset + its real system
/// prompt, selected whole via `--harness` (ADR-0012). One pack is active per run.
///
/// The pack — not a per-tool field — is the unit of harness identity. Contrast
/// Grok Build, which tags every tool with a `ToolNamespace` because it co-locates
/// ALL harnesses' tools in one registry (see this plan §5.2).
pub trait Pack: Send + Sync {
    /// The `--harness` selector and the report-envelope `harness` value.
    fn name(&self) -> &'static str;

    /// Register this pack's tools into `reg`, each under its harness's REAL wire
    /// name (`Tool` has no name of its own — the name is assigned here;
    /// `crates/locode-tools/src/tool.rs:70-77`). Infallible: a duplicate name is a
    /// wiring bug and panics inside `Registry::register`
    /// (`crates/locode-tools/src/registry.rs:156-160`).
    fn register(&self, reg: &mut Registry);

    /// The pack's system prompt (each pack owns its own — ADR-0012). Task 8 ships a
    /// placeholder; Task 13 renders grok's real prompt from `ctx` via minijinja.
    fn system_prompt(&self, ctx: &PackContext) -> String;

    /// Convenience: a fresh `Registry` with exactly this pack's tools.
    fn build_registry(&self) -> Registry {
        let mut reg = Registry::new();
        self.register(&mut reg);
        reg
    }
}
```

Note: the trait is object-safe (used as `&'static dyn Pack`); `build_registry` is a
provided method (not a separate free fn) so every pack gets it for free.

### 3.2 `lib.rs` — the resolver

```rust
//! locode-packs — faithful per-harness toolsets (ADR-0012), one module per harness.

mod grok;
mod pack;

pub use grok::GrokPack;
pub use pack::{Pack, PackContext};

/// Process-wide singleton packs (unit structs → zero-sized; no allocation, no
/// lifetime juggling). One `static` per harness.
static GROK: GrokPack = GrokPack;

/// Every wired harness, in a stable order (for `--help` and error messages).
/// Grows by one entry per pack (Task 15).
const PACKS: &[&'static (dyn Pack + 'static)] = &[&GROK];

/// The registered `--harness` names, stable order.
#[must_use]
pub fn available() -> Vec<&'static str> {
    PACKS.iter().map(|p| p.name()).collect()
}

/// Resolve a `--harness <name>` to its pack.
///
/// # Errors
/// [`UnknownHarness`] when no pack matches — carries the requested name and the
/// available list so `locode-exec` can print a clear, actionable error.
pub fn resolve(name: &str) -> Result<&'static dyn Pack, UnknownHarness> {
    PACKS
        .iter()
        .copied()
        .find(|p| p.name() == name)
        .ok_or_else(|| UnknownHarness {
            requested: name.to_owned(),
            available: available(),
        })
}

/// An unknown `--harness` selector (a caller/config error — soft, not a panic).
#[derive(Debug, thiserror::Error)]
#[error("unknown harness `{requested}`; available: {}", .available.join(", "))]
pub struct UnknownHarness {
    /// The name the caller asked for.
    pub requested: String,
    /// The names that ARE registered.
    pub available: Vec<&'static str>,
}
```

### 3.3 `grok/mod.rs` — the pack scaffold

```rust
//! The `grok` pack — a faithful port of Grok Build's `xai-grok-tools` toolset,
//! trimmed to headless-minimal (ADR-0012 §Scope). Tools land in Tasks 9-11; the
//! real prompt in Task 13. Task 8 wires the pack with a placeholder tool.

use locode_tools::Registry;
use crate::pack::{Pack, PackContext};

/// The grok harness pack (unit struct → a `&'static` singleton in `lib.rs`).
#[derive(Debug, Default, Clone, Copy)]
pub struct GrokPack;

impl Pack for GrokPack {
    fn name(&self) -> &'static str {
        "grok"
    }

    fn register(&self, reg: &mut Registry) {
        // Tasks 9-11 register grok's REAL tools here under their REAL wire names,
        // each carrying its ToolKind tag via `Tool::kind()`:
        //   reg.register("run_terminal_cmd", GrokRunTerminalCmd::new(host));  // Task 9
        //   reg.register("read_file",        GrokReadFile::new(host));        // Task 9
        //   reg.register("search_replace",   GrokSearchReplace::new(host));   // Task 10
        //   reg.register("grep",             GrokGrep::new(host));            // Task 11
        //   reg.register("list_dir",         GrokListDir::new(host));         // Task 11
        //
        // Task 8 (framework only, no host dep yet): a single placeholder so
        // routing/spec tests exercise the seam. Removed when Task 9 lands.
        reg.register("run_terminal_cmd", placeholder::GrokPlaceholder);
    }

    fn system_prompt(&self, _ctx: &PackContext) -> String {
        // Placeholder — Task 13 renders grok's real headless-branched prompt.
        "You are Grok, an autonomous coding agent.".to_owned()
    }
}
```

Wire names above are grok's **real** ids, verified in source:
`bash/mod.rs:1581` → `run_terminal_cmd`, `read_file/mod.rs:568` → `read_file`,
`search_replace/mod.rs:787` → `search_replace`, `grep/mod.rs:264` → `grep`,
`list_dir/mod.rs:458` → `list_dir`.

---

## 4. Behavior & edge cases

- **Selection.** `resolve("grok")` → `&GROK`. `locode-exec` (Task 14) parses `--harness`
  (default `grok`), calls `resolve`, then hands the engine `pack.build_registry()` and
  `pack.system_prompt(ctx)`.
- **Unknown harness → clear soft error.** `resolve("gpt")` →
  `Err(UnknownHarness{ requested:"gpt", available:["grok"] })`, whose `Display` is
  `unknown harness `gpt`; available: grok`. This is caller input, so it is an error
  value, **never a panic** — surfaced by `locode-exec` on stderr with a non-zero exit
  (ADR-0009). Contrast Grok, which returns a soft `RequirementError{ category:
  "tool_not_found" }` for an unknown tool id (`registry/types.rs:788-800`) — same
  soft-error philosophy, our unit is the harness not the tool.
- **Duplicate wire name → startup panic.** If a pack's `register` assigns the same name
  twice, `Registry::register` panics (`registry.rs:156-160`, message `duplicate tool
  registration for name ...`). This is a **wiring bug in authored pack code**, not user
  input, so panic-at-startup is correct (matches the Task-4 design note). Only *within*
  a pack can this happen — one pack per run means no cross-pack key collision
  (`registry.rs` module doc, lines 12-15).
- **Empty/partial grok registry during Tasks 8→9.** Until Task 9 lands, grok registers
  only the placeholder; `build_registry().specs()` returns one spec. Framework tests
  assert against a **test-local fake pack** (deterministic), while the grok-specific
  "expected real specs" assertion is introduced/tightened in Tasks 9–11 as each real
  tool lands (see §8 open question 1).
- **Spec surface.** A pack's tool specs come straight from `Registry::specs()`
  (`registry.rs:176-185`) — `{name, description, parameters}` per tool — which the
  provider maps onto the Anthropic wire (Task 12). Task 8 adds no new spec type;
  `ToolSpec` already exists.
- **System prompt shape.** `system_prompt` returns a `String` (the `Role::System`
  message body per ADR-0013 — `System` message *is* the base prompt,
  `protocol/lib.rs:22-24,49-50`). The engine wraps it into `Message{ role: System, .. }`;
  Task 8 does not itself build `Message`s (keeps the pack layer decoupled from how the
  engine seeds the preamble).

---

## 5. Design decisions (with harness `file:line`, why, why-not, differences)

### 5.1 A `Pack` **trait** owning register + prompt — modeled on Grok's `ToolPack` fn, elevated
Grok's out-of-tree extension seam is `pub type ToolPack = fn(&mut ToolRegistryBuilder)`
plus `register_tool_pack(pack)` (`registry/types.rs:41-51`): a *function* that
contributes registrations to a builder. That is exactly "a pack is a thing that
registers tools." We **elevate it to a trait** that also owns the pack's `name` and
`system_prompt`, because ADR-0012 defines a pack as tools **+ prompt + registration**,
and we want the prompt to travel with the toolset (a harness's tools and its prompt are
a matched pair — the prompt refers to the tools by name).
- **Why not a bare `fn(&mut Registry)` like Grok?** It carries no identity and no
  prompt; we'd need side tables mapping name→fn and name→prompt. A trait keeps the three
  facets (name, tools, prompt) cohesive in one `impl` per module.
- **Why not an `enum Pack { Grok, Codex, … }`?** Closed enums are fine for a fixed set,
  but a trait + `&'static dyn` keeps each pack's code fully inside its own module (no
  central `match` over behavior, only over construction) and mirrors Grok's
  per-implementation trait style (`Tool` + `ToolMetadata`, survey
  `03-grok-build/tool-abstraction.md`). Resolution still uses a tiny table (`PACKS`),
  which is the only central point that grows per pack.

### 5.2 Pack identity is the **module/struct**, not a per-tool namespace tag — the core divergence from Grok
Grok tags **every tool** with a `ToolNamespace` enum
(`{GrokBuild, GrokBuildConcise, GrokBuildHashline, Codex, OpenCode, MCP}`,
`types/tool.rs:33-46`) and builds one fully-qualified id `"<namespace>:<id>"` at
registration (`registry/types.rs:578-582`), e.g. `GrokBuild:read_file`,
`Codex:read_file`, `OpenCode:read` all live in **one** `ToolRegistryBuilder`
(`registry/types.rs:657-746` registers *all* namespaces' tools into a single builder).
- **Why Grok does this:** Grok Build is a long-lived, multi-tenant **tool server** that
  serves several client harnesses (the grok-build app, a Codex-compat surface, an
  OpenCode-compat surface) from **one process**; a session picks its toolset by sending
  a `ToolServerConfig { tools: Vec<ToolConfig> }` list of fully-qualified ids
  (`registry/types.rs:207-214`, selected in `finalize` at `registry/types.rs:1077`).
  The namespace tag is what lets multiple implementations of the same logical tool
  coexist in one registry and be disambiguated per-session.
- **Why we DON'T:** locode selects **one whole pack per run** via `--harness`
  (ADR-0012). Each pack builds its **own fresh `Registry`** (`build_registry`) holding
  only its tools under their real names — there is never more than one implementation of
  `read_file` in a registry, so nothing needs a namespace tag to disambiguate
  (`locode-tools/src/registry.rs` module doc, 12-15). Pack identity lives in the module
  path + `GrokPack::name()`, not in a per-tool field. This is strictly simpler and loses
  nothing the experiment bed needs.
- **`ToolKind` is the one tag we keep** — and only as a **cross-pack A/B alignment**
  classifier, exactly as Grok uses its semantic `ToolKind`
  (`types/tool.rs:70-105`, `#[serde(other)] Other`, `as_key()` at 113-115). Ours is the
  landed `ToolKind` (`locode-tools/src/tool.rs:34-68`); it already flows through
  `dispatch` into each report record — the pack layer adds nothing for it.

### 5.3 No name/param/description override machinery (ADR-0012 drops the re-skin)
Grok's `ToolConfig` carries `name_override`, `params_name_overrides`,
`description_override` (`registry/types.rs:53-92`, resolved at
`registry/types.rs:179-183`, 1095-1122) so one client can rename `read_file`→`view` etc.
This is precisely ADR-0006's dialect re-skin — **explicitly rejected by ADR-0012**
("collapses every harness onto one shared behavior"). Our packs hardcode each tool's
real wire name at `Registry::register("read_file", …)`; there is no override layer, no
`ToolConfig`, no `resolve_client_name`.
- **Why:** fidelity beats DRY (ADR-0012 §Context). A rename layer would let one impl
  masquerade as several harnesses — the exact contamination the experiment bed must
  avoid.

### 5.4 Duplicate-name handling: **panic** (authored code) vs Grok's soft config error
Grok validates a *client-supplied* config and returns a soft
`RequirementError{ category: "duplicate_client_name" }` when two enabled tools resolve to
the same client name (`registry/types.rs:830-851`) — because the offending input is
untrusted client data. locode's duplicate is inside a pack's own `register` (authored
Rust), so `Registry::register` **panics at startup** (`registry.rs:156-160`).
- **Why the difference:** soft-vs-fatal tracks **who authored the fault** (ADR-0004).
  Client config → recoverable soft error; our own wiring bug → fail fast, loud, at
  startup, before any model spend.
- Grok additionally forbids *mixing* standard vs hashline file tools in one config
  (`registry/types.rs:857-887`, `category: "file_toolset_conflict"`). We need **no such
  rule**: one pack per run can't mix two file-tool families. Another simplification the
  single-pack model buys.

### 5.5 The pack owns its prompt; no kind→name indirection needed
Grok keeps the prompt **outside** the toolset crate (the shell layer composes prompt +
toolset) and threads a `TemplateRenderer` built from a `kind → client_name` map
(`registry/types.rs:941-964`) so prompt text can reference `${{ tools.by_kind.read }}`
and stay correct even when a client renames the tool.
- **Why Grok needs the indirection:** its per-client renames (5.3) mean the prompt can't
  hardcode a tool name.
- **Why we don't:** ADR-0012 gives each pack its own prompt and we never rename tools,
  so grok's prompt (Task 13) can hardcode `run_terminal_cmd`, `read_file`, … directly.
  If a future pack wants kind→name lookup, `Registry` already exposes `names()`/`specs()`
  to build one — no new machinery in Task 8. Keeping `system_prompt` a method on `Pack`
  (not a separate crate) matches "each pack owns its system prompt" (ADR-0012 §Decision).

### 5.6 `&'static dyn Pack` singletons, not `Box<dyn Pack>`
Packs are unit structs (zero-sized), so a `static GROK: GrokPack = GrokPack;` +
`&'static dyn Pack` table costs no allocation and no lifetimes to thread through the
engine. Grok's packs are likewise stateless registration functions
(`registry/types.rs:41`). Resolution returns a shared `&'static dyn Pack`.

---

## 6. Tests (unit, in `#[cfg(test)]`)

Framework tests use a **test-local fake pack** so they are deterministic regardless of
how grok's real registry grows (Tasks 9–11):

1. **`resolve_grok_returns_grok_pack`** — `resolve("grok").unwrap().name() == "grok"`.
2. **`unknown_harness_errors_clearly`** — `resolve("gpt")` is `Err`; the `Display`
   contains both `gpt` and `grok` (asserts the actionable message the todo requires:
   "an unknown `--harness` errors clearly").
3. **`available_lists_grok`** — `available() == ["grok"]` (freezes the wired set;
   updated when Task 15 adds packs).
4. **`fake_pack_builds_expected_specs`** — a test-local `FakePack` registering two typed
   tools (an echo-style `Tool` like the ones in `locode-tools` tests) under wire names
   `alpha`/`beta`; assert `build_registry().specs()` yields exactly those names +
   derived schemas + `contains("alpha")`. Proves the pack→`Registry`→`ToolSpec` path
   without depending on grok's evolving toolset.
5. **`fake_pack_routes_to_impl`** — `#[tokio::test]`: `dispatch("alpha", args, &ctx)` on
   the built registry hits the registered impl and returns the expected
   `Dispatched.record.ok` + output (proves "a client call routes to the pack impl").
6. **`duplicate_registration_panics`** — `#[should_panic(expected = "duplicate tool
   registration")]`: a `FakePack` that registers `alpha` twice; `build_registry` panics
   (proves the startup-panic invariant at the pack layer).
7. **`grok_registers_placeholder`** — `GrokPack.build_registry().contains("run_terminal_cmd")`
   and `system_prompt(&ctx)` is non-empty. A **thin smoke test** of the grok scaffold;
   Tasks 9–11 replace it with real per-tool spec/behavior assertions (§8 Q1).

Dev-deps needed for tests: `tokio` (macros, rt), `schemars`, `serde`/`serde_json`,
`tokio-util` (for `CancellationToken` in a `ToolCtx`) — same set the `locode-tools`
tests already use.

---

## 7. Dependencies to add (with justification)

`crates/locode-packs/Cargo.toml` currently lists only `locode-protocol`, `locode-tools`,
`locode-host` (path deps). Task 8 additions:

- **`thiserror` (workspace dep)** — for `#[derive(Error)]` on `UnknownHarness`. Already a
  workspace dependency (used in `locode-tools`, ADR-0003/Task-4 note) and part of the
  approved tooling baseline (SPEC Tech Stack: "Errors | `thiserror`"). Reusing an
  existing workspace dep, not a new third-party crate — no "ask first" trigger, but
  noted per the working agreement.
- **dev-deps**: `tokio` (`macros`, `rt`), `schemars`, `serde`, `serde_json`,
  `tokio-util` — test-only, mirroring `locode-tools`'s dev-deps for the fake typed
  tools. No production weight.

**Not added in Task 8 (deliberate):**
- `minijinja` — prompt rendering is Task 13; keep `system_prompt` a stub here so the
  prompt engine and its templates land as their own slice.
- No dependency on `locode-host` is *exercised* yet (the placeholder tool needs no
  host); the path dep stays declared for Tasks 9–11.

No change to `schema_version`, no public `Tool`/`Provider` signature change, no crate
boundary change → none of the "Ask first" boundaries (SPEC §Boundaries) are tripped.

---

## 8. Open questions

1. **Task-8-vs-9 placeholder.** Task 8 lands before the real grok tools (dep is Task 4,
   not Task 7). Recommendation: register a single in-crate **placeholder** tool named
   `run_terminal_cmd` so routing/spec tests exercise the seam in isolation, and assert
   grok's *real* specs incrementally in Tasks 9–11. Alternative: make grok's `register`
   a no-op (empty registry) in Task 8 and defer *all* grok-specific assertions to Task 9
   — cleaner separation but leaves Task 8's "a client call routes to the grok impl"
   acceptance criterion (`todo.md:162`) provable only via the fake pack. **Preferred:
   placeholder**, since it satisfies the todo's grok-routing criterion directly and is a
   two-line delete when Task 9 lands. Confirm.

2. **Real grok tool names (P1 naming — ADR-0012 says names are P1).** The SPEC/todo/plan
   spell the terminal tool **`run_terminal_command`** (`SPEC.md:130`, `todo.md:168`,
   `plan.md:59`), but grok's **real** id is **`run_terminal_cmd`** (`bash/mod.rs:1581`).
   Since ADR-0012 makes behavior P0 and exact names P1 ("faithful"), the plan uses the
   **real** `run_terminal_cmd`. Confirm we want the faithful name (recommended) and,
   if so, whether to correct the SPEC/todo wording in a follow-up. The other four are
   already faithful: `read_file`, `search_replace`, `grep`, `list_dir`.

3. **Grok has no standalone `write` tool.** SPEC §Success Criteria and Task 10 list a
   grok `write` tool, but `xai-grok-tools/src/implementations/grok_build/` has **no
   `write` module** — grok creates files via `search_replace` (create semantics) /
   `bash`; a dedicated `write` is an OpenCode tool (`opencode::OpenCodeWriteTool`,
   `registry/types.rs:703`). Not a Task-8 blocker, but flag now so Task 10 either (a)
   ports grok's real create-via-`search_replace` path, or (b) knowingly adds a `write`
   as a locode-ism. Needs a decision before Task 10.

4. **`PackContext` field set.** Proposed minimal `{cwd, os, shell, date, headless}` is a
   guess at what grok's real prompt (Task 13) consumes. If Task 13's ported template
   needs more (e.g. git branch, model name), the struct grows then. Flagging that Task 8
   fixes the *seam*, not the final field set.

5. **Where the engine turns a pack into a preamble.** Task 8 returns `String` from
   `system_prompt`; whether the engine (Task 6) or a `locode` facade helper (Task 14)
   wraps it into a `Role::System` `Message` (and appends a `Developer` message for
   cwd/OS context per ADR-0013) is a Task-6/14 boundary detail, not decided here.

---

## Identifier guesses to confirm (per AGENTS.md)
- Interpreted the terminal tool as grok's real **`run_terminal_cmd`**, not the
  SPEC/todo's `run_terminal_command` (see §8 Q2).
- Assumed the grok `write` tool in SPEC/Task 10 has no faithful grok source and will be
  resolved in Task 10 (see §8 Q3).
