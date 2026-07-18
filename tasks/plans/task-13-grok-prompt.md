# Task 13 — grok pack system prompt (minijinja, headless-branched identity)

> Scope: Task 13 in `tasks/todo.md:233`. Delivers the grok pack's system prompt as a
> minijinja-rendered template ported from Grok Build's real prompt, with the identity
> line branched on headless. Files: `crates/locode-packs/src/grok/prompt.rs`, a template
> asset, and tests. Depends on Task 8 (pack framework) which owns tool-name resolution.
>
> Reference harness: **Grok Build** — `xai-grok-agent` (prompt engine) + `xai-grok-tools`
> (`TemplateRenderer`). All `file:line` citations below are into
> `~/dev/coding-cli-survey/submodules/grok-build/crates/codegen/…` unless noted.

---

## 1. Purpose & scope (+ deferred)

**Purpose.** Give the `grok` pack the *third* thing a `Pack` owns (alongside its tool set
and its registration) — its **system prompt** (ADR-0012:46 "Each pack owns its system
prompt"). v0 ports Grok Build's *real, trimmed* base prompt (`templates/prompt.md`, 46
lines, ≈670 words — the shortest of the four studied harnesses and the *only one* that
branches identity on interactive-vs-headless, per the survey design note
`survey/06-design-lessons/minimal-headless-rust-agent.md:417`). The port renders through
minijinja with Grok's **custom `${{ }}` / `${% %}` delimiters**, resolving the grok pack's
real tool names (`read_file`, `search_replace`, …) and branching the identity line on
`is_non_interactive`.

**In scope**
- A template asset (ported + trimmed from `templates/prompt.md`) stored in the crate.
- `render_grok_system_prompt(ctx: &GrokPromptContext) -> String` — the System-message text.
- A `GrokPromptContext` struct (env + identity + resolved tool names) that feeds the render.
- The minijinja `Environment` with custom syntax + the `by_kind` tool map (mirrors
  `TemplateRenderer`, `xai-grok-tools/src/types/template_renderer.rs`).
- Snapshot test of the rendered prompt + a headless-branch toggle test.

**Deferred (reserved seams, not v0)** — everything Grok's prompt engine has that headless
doesn't need (ADR-0012:59 "trimmed to headless-minimal"):
- The **apply-patch** and **subagent** template variants (`apply_patch_prompt.md`,
  `subagent_prompt.md`) — arrive with the `codex` pack / any future subagent (Task 15).
- **XOR template obfuscation** (`prompt/template.rs:17` `decrypt` + `Zeroizing`) — a Grok
  anti-`strings` measure with *no* value for an experiment bed; we ship plaintext.
- `AGENTS.md` / project-memory injection (`prompt/agents_md.rs`), personas, skills, memory
  section, plugin/user-guide docs — design §8 marks these "optional later"
  (`minimal-headless-rust-agent.md:430`).
- `PromptMode::{Extend,Full}`, `TemplateOverride`, custom-body concatenation
  (`prompt/context.rs:260`) — one fixed base template in v0.
- Per-model-family prompt selection (OpenCode's `SystemPrompt.provider(model)`) — design §8
  point 2 says "nice-to-have once you run multiple providers; not v0".

---

## 2. Module layout

```
crates/locode-packs/
├── Cargo.toml                 # + minijinja (custom_syntax feature); + insta (dev) for snapshots
└── src/
    ├── lib.rs
    ├── pack.rs                # Task 8: Pack trait; a Pack yields tools + system prompt
    └── grok/
        ├── mod.rs             # Task 8: grok pack; calls prompt::render_grok_system_prompt(...)
        ├── prompt.rs          # THIS TASK: GrokPromptContext, render fn, minijinja env, tests
        └── templates/
            └── system_prompt.md   # ported+trimmed from grok's templates/prompt.md
```

- The template is embedded with `include_str!("templates/system_prompt.md")` — no
  filesystem read at runtime, no decryption (contrast Grok's `include_bytes!` of the
  *encrypted* bytes at `prompt/template.rs:75`). Plaintext-in-repo is deliberate: this repo
  is a study bed, and a plaintext template is what the snapshot test asserts against.
- Rendering lives entirely in `prompt.rs`. Unlike Grok, which splits the renderer
  (`xai-grok-tools::TemplateRenderer`) from the context (`xai-grok-agent::PromptContext`)
  across two crates because the renderer is shared with tool *descriptions*
  (`template_renderer.rs:185` `render_schema_descriptions`), we keep it in one file: v0
  does not template tool descriptions (schemas are schemars-derived, ADR-0003), so there is
  no second consumer to factor out yet.

---

## 3. Key types & signatures — concrete Rust sketches

### 3.1 The template context

```rust
//! crates/locode-packs/src/grok/prompt.rs
use serde::Serialize;
use std::collections::BTreeMap;

/// The data the grok system-prompt template sees at render time.
///
/// Mirrors the split Grok uses: agent-specific placeholders (`os_name`,
/// `is_non_interactive`, …) merged with a `tools.by_kind.<kind>` map that
/// resolves `${{ tools.by_kind.read }}` to the pack's real tool name.
/// (Grok: `PromptContext::placeholders()` at prompt/context.rs:237 + `ToolsContext`
/// at template_renderer.rs:37.)
#[derive(Debug, Clone, Serialize)]
pub struct GrokPromptContext {
    /// `You are <label> released by xAI` — grok's identity label. Grok defaults this
    /// to `"Grok"` (context.rs:153 `DEFAULT_SYSTEM_PROMPT_LABEL`). We keep the faithful
    /// label so the pack reproduces grok's real identity (ADR-0012: fidelity > our brand).
    pub system_prompt_label: String,

    /// THE headless branch. `true` → "an autonomous agent that completes software
    /// engineering tasks."; `false` → "an interactive CLI tool that helps users…"
    /// (grok `templates/prompt.md:1`). Design §8 signal #1 (minimal-headless…:419).
    /// For `locode-exec` this is ALWAYS `true` — the core is headless-only (ADR-0001).
    pub is_non_interactive: bool,

    // ── <user_info> env block (dynamic; injected at build time) ──
    pub os_name: String,          // e.g. "macos"   (std::env::consts::OS)
    pub shell_path: String,       // e.g. "/bin/zsh" ($SHELL)
    pub working_directory: String,// the model-facing cwd (workspace_root)
    pub current_date: String,     // "YYYY-MM-DD" local

    /// ToolKind (serialized snake_case) → the pack's real client-facing tool name.
    /// Serializes to `{ "by_kind": { "read": "read_file", "edit": "search_replace", … } }`
    /// so the template writes `${{ tools.by_kind.read }}`. (template_renderer.rs:37-40.)
    pub tools: ToolsContext,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsContext {
    pub by_kind: BTreeMap<String, String>, // BTreeMap → deterministic (snapshot-stable)
}
```

**How the pack builds it (Task 8 wiring, shown for context).** The pack owns the
`ToolKind → real name` map — it is the single source of truth already used to route
dispatch. The prompt reuses it:

```rust
// grok/mod.rs (Task 8) builds by_kind from the registered tools' kind()+wire-name:
//   Shell → "run_terminal_command", Read → "read_file", Write → "write",
//   Edit  → "search_replace",       Grep → "grep",      Glob → "list_dir"
```

> **Porting note — our `ToolKind` keys differ from grok's.** Our enum serializes
> `shell/read/write/edit/glob/grep/other` (`locode-tools/src/tool.rs:59-65`), whereas grok's
> template writes `execute/read/edit/search/list` (`template_renderer` test map,
> `prompt/template.rs:98-115`: Execute→run_terminal_command, Search→grep, List→list_dir).
> When porting `prompt.md` we **rewrite the placeholders to our keys**:
> `${{ tools.by_kind.execute }}` → `${{ tools.by_kind.shell }}`,
> `${{ tools.by_kind.search }}` → `${{ tools.by_kind.grep }}`,
> `${{ tools.by_kind.list }}`   → `${{ tools.by_kind.glob }}`.
> This is a deliberate, cited divergence — flag it in the PR.

### 3.2 The render function + minijinja env

```rust
use minijinja::{Environment, syntax::SyntaxConfig};

const TEMPLATE: &str = include_str!("templates/system_prompt.md");

/// Build a minijinja environment with grok's CUSTOM delimiters, so literal `{{ }}`
/// in prose passes through unrendered. (Grok: `make_desc_env` at
/// xai-grok-tools/src/types/description.rs:41-44.)
fn grok_env() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_syntax(
        SyntaxConfig::builder()
            .block_delimiters("${%", "%}")
            .variable_delimiters("${{", "}}")
            .comment_delimiters("${#", "#}")
            .build()
            .expect("static grok delimiter config is valid"), // const inputs → infallible
    );
    env
}

/// Render the grok pack's System-message text.
///
/// Deterministic (BTreeMap ordering + no ambient time — `current_date` is an input).
/// Returns `Result` because a malformed template is a programmer error we surface loudly
/// in the one place it can occur (the snapshot test), not `unwrap` in a hot path.
pub fn render_grok_system_prompt(
    ctx: &GrokPromptContext,
) -> Result<String, minijinja::Error> {
    grok_env().render_str(TEMPLATE, ctx)
}
```

- `render_str` compiles+renders in one shot (matches Grok's `env.render_str` at
  `template_renderer.rs:79`). No `add_template`/`get_template` dance needed for one template.
- **No fast-path shortcut** (Grok's `if !template.contains("${{")…return` at
  `template_renderer.rs:71`): our single template always contains markers, so the guard is
  dead weight. Note it as an optimization Grok needs because it renders *thousands* of
  mostly-static tool descriptions through the same call — we don't.

### 3.3 Where the env block lives (System vs Developer) — a composition decision for Task 8

The render fn above produces **one string**. Grok's shipped `templates/prompt.md` is
striking: it carries identity + `<action_safety>` + `<tool_calling>` + `<output_efficiency>`
+ `<formatting>` but **no `<user_info>` block** — the cwd/OS/shell/date are injected
*elsewhere* (Grok's `prompt/workspace_user.rs`, as a separate turn), which is why
`GrokPromptContext`'s env fields feed a block the *base* prompt.md doesn't itself render
(its subagent template does, `context.rs:804-819`).

Recommendation for our port, aligned to ADR-0013's 4-role model:
- **System message** = static identity + rules (cacheable "constitution",
  `locode-protocol` `Role::System`). This is what `render_grok_system_prompt` returns.
- **`<user_info>` env block** (cwd/OS/shell/date) = a **Developer message**
  (`Role::Developer` = "dynamically injected operational context",
  `locode-protocol/src/lib.rs:42-45`). Keeping the volatile env out of System preserves the
  Anthropic cache boundary (design §8 point 2, "keep base short and inject variable parts"
  + the Claude static/dynamic cache boundary noted at `minimal-headless-rust-agent.md:417`).

So Task 13 ships **two** small renderers (both trivial, both here):
`render_grok_system_prompt(ctx)` (identity+rules) and `render_grok_user_info(ctx) -> String`
(the `<user_info>` block for the Developer message). Both land in the `Event::Init.preamble`
(System + Developer, ADR-0014, `locode-protocol/src/lib.rs:243`). *If review prefers a single
System blob*, fold `<user_info>` into the template behind `${%- if working_directory %}` and
drop the Developer message — noted as Open Question Q1 below.

---

## 4. Behavior / algorithms + edge cases

**The ported template (`system_prompt.md`), sketch** — faithful to `prompt.md` with our
tool keys and the headless default. The identity line is verbatim grok (`prompt.md:1`):

```jinja
You are ${{ system_prompt_label }} released by xAI. You are
${%- if is_non_interactive %} an autonomous agent that completes software engineering tasks.
${%- else %} an interactive CLI tool that helps users with software engineering tasks.
${%- endif %} Your main goal is to complete the user's request, denoted within the
<user_query> tag.

<tool_calling>
- Use specialized tools instead of bash commands when possible. For file operations, prefer
  dedicated file tools${%- if tools.by_kind.read %} (e.g., `${{ tools.by_kind.read }}` for
  reading instead of cat/head/tail${%- if tools.by_kind.edit %}, `${{ tools.by_kind.edit }}`
  for editing and creating files instead of sed/awk${%- endif %})${%- endif %}. Reserve bash
  for actual system commands. NEVER use bash echo to communicate with the user — output all
  communication directly in your response text.
${%- if tools.by_kind.grep %}
- Prefer `${{ tools.by_kind.grep }}` / `${{ tools.by_kind.glob }}` to locate code before
  large shell scans; read a file before you edit it.
${%- endif %}
</tool_calling>

<output_efficiency> …verbatim from prompt.md:30-34… </output_efficiency>
<formatting> …verbatim from prompt.md:37-39… </formatting>
```

Then a separate `render_grok_user_info` produces (mirrors subagent template's
`<user_info>`, verified by `context.rs:804-819`):

```
<user_info>
OS: {os_name}
Shell: {shell_path}
Workspace Path: {working_directory}
Current Date: {current_date}
</user_info>
```

Content mapped to design §8's "minimal v0 prompt" checklist
(`minimal-headless-rust-agent.md:424-427`): (1) identity branched on headless ✓; (2) cwd/OS/
shell/date + tools by their pack names ✓; (3) prefer read/grep/glob before shell, prefer edit
over `echo >`, read before edit ✓; (4) "stop when done — reply with text, no tool calls" — add
one line (design §8 point 4; not in grok's trimmed prompt, but design asks for it).

**The guard invariant (port it).** Every `${{ tools.by_kind.X }}` MUST sit inside a
`${%- if tools.by_kind.X %}` — otherwise a pack missing tool X renders an empty string into
prose. Grok enforces this with a static analyzer test (`assert_guards`,
`prompt/template.rs:710-771`). We port a *scaled-down* version (Test 4 below): far fewer
kinds, but the same failure mode. In practice the v0 grok pack always registers all six
tools, so guards never fire — but the test protects against a future trimmed pack.

**Edge cases**
- **Literal braces in prose.** `{{ foo }}` must survive verbatim (grok relies on this for
  tool descriptions containing JSON examples). The custom `${{ }}` delimiters guarantee it;
  Test 5 pins it (grok's `test_literal_braces_pass_through`, `template.rs:199-206`).
- **Empty env fields.** If `working_directory`/`shell_path` are unknown, render empty
  strings (grok defaults them to `""` via `unwrap_or("")`, `context.rs:243-246`) rather than
  the literal "None". `render_grok_user_info` should omit the block entirely when all env
  fields are empty (a `${%- if %}` guard), to avoid emitting a hollow `<user_info>`.
- **Non-ASCII / newlines in cwd.** minijinja auto-escaping is HTML-oriented and irrelevant
  for a plain-text prompt; construct the env with autoescape **off** (default for
  `Environment::new()` unless a template name ends in `.html`) so paths aren't `&amp;`-mangled.
- **Determinism.** No ambient clock (date is an input), `BTreeMap` for tool ordering →
  byte-identical renders (grok pins this too: `test_prompt_deterministic_across_renders`,
  `template.rs:436`). Required for the snapshot test to be stable.

---

## 5. Design decisions (each: harness `file:line`, why, why-not-alternative, differences)

1. **minijinja with custom `${{ }}`/`${% %}`/`${# #}` delimiters.**
   - Grok: `make_desc_env` sets exactly these — `SyntaxConfig::builder().block_delimiters("${%","%}").variable_delimiters("${{","}}").comment_delimiters("${#","#}")` (`xai-grok-tools/src/types/description.rs:41-44`); dep is `minijinja = { version = "2", features = ["custom_syntax"] }` (`xai-grok-agent/Cargo.toml`).
   - Why: the prompt (and grok's tool descriptions) contain literal `{{ }}` JSON/code
     samples; standard Jinja delimiters would try to render them. The `$`-prefixed forms
     avoid the collision. Also SPEC's chosen templating engine (`SPEC.md:37`).
   - Why not alternatives: **`format!`/string-replace** (what a naive port might do) — loses
     conditionals (the headless branch, per-tool guards) and re-implements escaping badly.
     **Standard `{{ }}` Jinja / Tera / Askama** — Askama is compile-time (can't easily branch
     on runtime tool presence into one asset) and standard delimiters reintroduce the literal-
     brace hazard. **Handlebars** — heavier, no custom delimiters as clean.
   - Difference from grok: we render **one plaintext template**, no XOR decryption
     (`template.rs:17`), no shared description renderer, no fast-path guard.

2. **Branch the identity line on `is_non_interactive`, default `true` for the headless core.**
   - Grok: `templates/prompt.md:1` (`${%- if is_non_interactive %} an autonomous agent…
     ${%- else %} an interactive CLI tool…`); context field `context.rs:145`; behavior pinned
     by `non_interactive_suppresses_…` / `interactive_renders_…` (`template.rs:784-830`).
   - Why: design §8 signal #1 — "A one-line change that measurably shifts behavior in headless
     runs" (`minimal-headless-rust-agent.md:419`); grok is the *only* studied harness that does
     it (`…:417`). `locode-core` is headless-only (ADR-0001), so `locode-exec` always sets
     `true`; but keeping the flag (not hard-coding the autonomous string) preserves the seam
     for a future `locode-app` interactive front-end and lets the toggle test exercise both.
   - Why not: hard-code "autonomous" — throws away the seam the survey explicitly calls out as
     cheap and high-value; and the branch is the headline finding this task exists to reproduce.
   - Difference: none in wording (verbatim port); we just fix the default to headless.

3. **Tool names via a `tools.by_kind.<kind>` map, not literals baked in the template.**
   - Grok: `ToolsContext { by_kind: HashMap<ToolKind,String> }` (`template_renderer.rs:37`),
     resolved as `${{ tools.by_kind.read }}` (`template.rs:159`).
   - Why: the pack owns the real names (ADR-0012:46, ADR-0003 note that `Tool` has no
     `name()` — the pack assigns wire names); rendering from the same kind→name map keeps
     prompt and dispatch in lockstep, and makes the template reusable if names are refined
     (P1 per ADR-0012:33). Design §8 point 2 wants tool names to "track the active dialect"
     via placeholders (`minimal-headless-rust-agent.md:425`).
   - Why not: literal `read_file` in the template — drifts from the registry the moment a name
     changes and defeats cross-pack reuse.
   - Difference: we use a `BTreeMap` (deterministic snapshots) and **our** kind keys
     (`shell/grep/glob`, `tool.rs:59`) not grok's (`execute/search/list`) — see §3.1 note.

4. **Plaintext template embedded via `include_str!`; drop grok's XOR obfuscation + `Zeroizing`.**
   - Grok: `decrypt()` XORs `BASE_PROMPT_ENC` and returns `Zeroizing<String>`
     (`prompt/template.rs:17-33`); staleness guarded by `test_encrypted_templates_not_stale`
     (`template.rs:68`).
   - Why not port it: grok's own comment says it is "obfuscation, not security — seeds live
     in-repo" (`template.rs:5`) — an anti-`strings` measure for a shipped commercial binary.
     For an open study bed it adds a build step, a staleness test, and a `zeroize` dep for
     zero benefit; the snapshot test *wants* the plaintext.
   - Difference: strictly a simplification; the rendered output is unaffected.

5. **Trim to headless-minimal — one base template, no subagent/apply-patch/memory/skills/AGENTS.md.**
   - Grok: `PromptContext::render` selects among base/subagent/codex templates and appends a
     custom body (`prompt/context.rs:260-297`); memory/personas/user-guide are conditional
     sections.
   - Why: ADR-0012:59 scopes v0 to "the `grok` pack only … trimmed to headless-minimal
     (dropping interactive/sandbox/MCP/streaming concerns)"; design §8 defers AGENTS.md and
     skills (`minimal-headless-rust-agent.md:430`). Keeping one asset keeps the snapshot small
     and the guard test tractable.
   - Difference: our template is a strict subset; each dropped section is a reserved seam.

6. **Render fn returns `Result`, called once at session start (not per turn).**
   - Grok renders the prompt once at agent build and reuses it. Why: the System prompt is
     static for a run; re-rendering per turn wastes work and (with a clock input) risks
     nondeterminism. The `Result` surfaces template bugs at the boundary; the snapshot test is
     the place they get caught. No `unwrap` in library code (workspace lint
     `unwrap_used = "deny"`, `Cargo.toml`), except the infallible const `SyntaxConfig` build.

---

## 6. Tests (Task 13 acceptance: snapshot + headless toggle)

Use `insta` for the snapshot (add as a dev-dependency) — or a committed golden string if we
prefer zero new deps (see §7). Fixed inputs (no clock) keep it stable.

1. **`renders_headless_snapshot`** — build `GrokPromptContext { is_non_interactive: true,
   label: "Grok", os_name:"macos", shell_path:"/bin/zsh", working_directory:"/repo",
   current_date:"2026-07-17", by_kind: {shell→run_terminal_command, read→read_file,
   write→write, edit→search_replace, grep→grep, glob→list_dir} }`; assert the full rendered
   System prompt matches a committed snapshot. Freezes the ported wording. (Grok analog:
   `test_base_template_renders` + size budget, `template.rs:231, 672`.)

2. **`headless_branch_toggles_identity`** — render with `is_non_interactive` true vs false;
   assert `true` → contains "autonomous agent" and NOT "interactive CLI tool"; `false` →
   the inverse. This is the task's headline acceptance criterion (`todo.md:241`). (Grok analog:
   `non_interactive_suppresses_…` / `interactive_renders_…`, `template.rs:784-830`.)

3. **`resolves_real_grok_tool_names_no_unresolved_markers`** — assert the render contains
   `read_file` and `search_replace`, and does NOT contain `${{` or `${%` (no unresolved
   markers). (Grok: `test_base_template_contains_resolved_tool_names`, `template.rs:238-250`.)

4. **`every_tool_placeholder_is_guarded`** — static scan of the template asset: each
   `${{ tools.by_kind.X }}` occurs inside an enclosing `${%- if tools.by_kind.X %}`. Render
   with a *reduced* pack (only `read`) and assert no empty artifacts leak. (Ported, scaled
   down, from grok's `assert_guards`, `template.rs:710-771`.)

5. **`literal_braces_pass_through`** — render a context whose e.g. `working_directory`
   contains `{{ x }}`; assert it survives verbatim. (Grok: `template.rs:199-206`.)

6. **`deterministic_across_renders`** — render twice, assert byte-equal. (Grok:
   `template.rs:436`.)

7. **`user_info_block_omitted_when_env_empty`** — `render_grok_user_info` with all-empty env
   returns `""` (no hollow `<user_info>`).

---

## 7. Deps to add (with justification + precedent)

| Dep | Where | Justification | Precedent |
|---|---|---|---|
| `minijinja` v2, feature `custom_syntax` | `locode-packs` runtime | SPEC's chosen templating engine (`SPEC.md:37`); required for the `${{ }}`/`${% %}` delimiters and the headless/tool conditionals. | Grok uses the exact same crate+feature (`xai-grok-agent/Cargo.toml`: `minijinja = { version = "2", features = ["custom_syntax"] }`). |
| `insta` v1 (dev-only) | `locode-packs` `[dev-dependencies]` | Ergonomic snapshot review for the rendered prompt (Test 1). | Widely used; **optional** — if we want zero new deps, commit a golden `.txt` and `assert_eq!` instead (matches the repo's existing golden-file style, `locode-protocol/tests/envelope_golden.rs`). Recommend the golden-file route to stay dependency-light unless snapshot churn justifies `insta`. |

- **`serde`** — already a workspace dep (`Cargo.toml:12`); `GrokPromptContext`/`ToolsContext`
  derive `Serialize` so minijinja can consume them (grok serializes its `TemplateContext` the
  same way, `template_renderer.rs:48`).
- **AGENTS.md "Ask first: adding a dependency"** — `minijinja` is pre-blessed by
  `SPEC.md:37`; still call it out in the PR. `insta` is a genuine new dep — prefer the golden
  file to avoid the ask.

---

## 8. Open questions

1. **System-only vs System+Developer split for `<user_info>`.** §3.3 recommends env in a
   Developer message (cache boundary + ADR-0013 role semantics). Confirm with the engine
   author (Task 6) that the pack may contribute *both* a System and a Developer preamble
   message, or whether v0 should fold `<user_info>` into the single System blob. Ties to how
   `Event::Init.preamble` is assembled (`locode-protocol/src/lib.rs:243`).
2. **Identity label.** Keep `"Grok released by xAI"` verbatim (fidelity, ADR-0012) — confirm
   we're comfortable shipping xAI's identity string in our repo, vs a neutral
   `"a coding agent"`. Faithful port argues for verbatim; flag for the maintainer.
3. **Add the "stop when done" line?** Design §8 point 4 wants it
   (`minimal-headless-rust-agent.md:427`) but grok's trimmed `prompt.md` omits it. Include it
   (helps headless termination) or stay byte-faithful to grok? Recommend include, noted as a
   deliberate, cited addition.
4. **`insta` vs golden `.txt`** (see §7) — pick one before writing Test 1.
5. **Kind-key remap surface.** We remap `execute/search/list` → `shell/grep/glob` (§3.1).
   Confirm the grok pack's `ToolKind` assignments (Task 8/9-11) match this mapping so the
   guards line up.
