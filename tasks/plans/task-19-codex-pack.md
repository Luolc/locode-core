# Task 19 — `locode-packs` codex pack (faithful port of Codex's tools + prompt)

> Implementation plan, written **before** code. Faithful mimicry per AGENTS.md:
> real names, verbatim descriptions/schemas, real caps and output shapes; the
> fidelity boundary (the fidelity boundary in AGENTS.md) keeps loop-adjacent machinery out.
> Cites codex source under `~/dev/coding-cli-survey/submodules/codex/codex-rs`
> (submodule pinned at `1d941253e9354fe583a033660a6288df66e27488`, read
> 2026-07-18) as `file:line`, plus `survey/02-codex/{provider-api,apply-patch-
> deep-dive}.md`. Pack pattern: the grok pack (verbatim template copies +
> provenance sha/commit pins + byte-pin tests + `strip_identity`).
>
> **Wire coupling — read first:** codex is Responses-API-only (`WireApi` has a
> single `Responses` variant and `wire_api = "chat"` is a hard config error,
> `model-provider-info/src/lib.rs:57-60,80`; no `chat_completions*` file exists),
> and `apply_patch` is a **freeform custom tool with a Lark grammar** — an
> OpenAI-Responses-only delivery (xAI 422s `type:"custom"`, Task 18 probe P3).
> This pack therefore depends on **Task 18** for its native delivery. It still
> *runs* on every other wire via the shared freeform degradation (`{input:
> string}` JSON framing — Task 18 §4.6, ADR-0012's reserved fallback); §4.6 spells
> out the cross-wire story.

---

## Reconciliation (2026-07-24) — read the working doc first; much below is superseded

This plan was written against submodule pin `1d941253` and predates both the claude
pack's conventions and a **2026-07-24 re-survey at the re-pinned commit `f201c30c`**
(codex was 325 commits stale). The **process, resolved decisions, and slice plan now
live in [`../../docs/codex-pack-dev-process.md`](../../docs/codex-pack-dev-process.md)**
— start there. What the re-survey changed (this file's affected sections are stale):

- **Tool set is a DUO: `shell_command` + `apply_patch`.** `update_plan` is **not
  ported** (user decision — deferred entirely; §1.2 / §4.4 / §5.6 no longer apply).
- **Shell tool = `shell_command` (non-PTY)**, marked deprecated in comments. Codex's
  mac/Linux default is now **unified exec** (`exec_command`/`write_stdin`, PTY/session);
  we use `shell_command` (sol's declared `shell_type`, unified-exec disabled) because
  background/session is out of scope — switch when P0.5 background lands. (§1.1's
  "shell_command is stock" and §1.3's unified-exec-deferral framing are updated.)
- **`apply_patch` is freeform-ONLY** — the JSON `{input}` variant was deleted upstream;
  combined with **openai-responses-only** (D5), the untagged two-shape `Args` (§3.3) and
  the whole cross-wire degradation story (§4.6, intro) are **dropped**. One shape: a
  freeform Lark-grammar tool. Add mkdirs parents via `Host::create_dir`.
- **Base prompt = gpt-5.6-sol** (17730-byte `base_instructions` from `models.json`, new
  "You are Codex, an agent based on GPT-5" identity), **not** the model-independent
  `prompt.md` default §4.7 chose. Truth-first: clean. apply_patch instructions: **always
  appended** (§9 Q2 resolved).
- **`<environment_context>` rebuilt** (cwd/shell/current_date/timezone/permission-profiles;
  old approval/sandbox/os tags gone) — §4.8 updated; follow-the-source.
- **Spec builders moved to the new `codex_tools` crate** — all `core/src/tools/handlers/*`
  citations need the new path.
- Conventions since claude: cite **ADR-0023** for the fidelity boundary; AGENTS.md loaded
  by the shared engine (not the pack); `Pack::shape_user_prompt` (default verbatim);
  `PackContext` has `is_git_repo`/`model`/`os_version`; `Host::create_dir`.

The parser/matcher design (§3.4, §4.2–4.3), the `deny_unknown_fields`/type-strict schema
posture (§4.5, §5.5), the approval-params-dropped gap (§5.9), and the freeform-tool
delivery (§5.2) still stand. Everything else: defer to the working doc.

---

## 1. Purpose & scope

Port Codex CLI's headless-relevant toolset and base prompt as `--harness codex`
(ADR-0012). Codex is the extreme point in the A/B space: **no read / grep /
glob / write tools at all** — the shell is the read path (its prompts even coach
`rg` usage), and **all editing goes through one patch-format tool**. Comparing
that against grok's five-tool surface and claude's six-tool surface is exactly
the experiment this repo exists to run.

### 1.1 Full tool inventory (what exists at `1d94125`)

From `core/src/tools/spec_plan.rs` (`add_tool_sources`, `:579-619`;
`add_core_utility_tools`, `:703-745`):

| Tool | Kind on the wire | Headless-v0 verdict |
|---|---|---|
| `shell_command` | function (`handlers/shell_spec.rs:157-225`) | **port** |
| `apply_patch` | **custom/freeform, Lark grammar** (`handlers/apply_patch_spec.rs:9-27`) | **port** |
| `update_plan` | function (`handlers/plan_spec.rs:7-58`) | **port** (§1.2) |
| `exec_command` + `write_stdin` (unified exec) | functions (`shell_spec.rs:21-155`) | defer — PTY session store = stateful background infra (yield/poll loops, session ids); `shell_command` is the non-PTY variant codex itself selects per model (`shell_type_for_model_and_features`, `spec_plan.rs:653-676`) |
| `view_image` | function (`view_image_spec.rs:15-50`) | defer — multimodal tool_result (image chunks) unexercised in v0 (protocol supports it; nothing renders it) |
| `web_search` | hosted server-side tool (`hosted_spec.rs:14-46`) | defer — no dispatch; a Responses-server feature, not a client tool |
| `request_permissions` | function, `Feature::RequestPermissionsTool`-gated (`shell_spec.rs:227-262`) | defer — approval flow is interactive by definition |
| `list_mcp_resources` / `read_mcp_resource` etc. | MCP-gated (`spec_plan.rs:693-700`) | defer — MCP is a repo-wide deferred seam |
| `wait_for_environment`, `request_user_input`, `new_context_window`, `get_context_remaining`, `current_time`, `sleep`, collaboration tools | feature-flag-gated (`spec_plan.rs:703-745,787+`) | defer — flags off in a stock run |

**Headless-v0 subset = { `shell_command`, `apply_patch`, `update_plan` } — 3
tools.** That small number IS the faithful codex shape: a stock non-Windows,
non-unified-exec, no-MCP codex session registers essentially this trio (+
view_image). ToolKind tags: `Shell`, `Edit`, `Other`.

### 1.2 Why `update_plan` is IN (contrast: claude pack's TodoWrite is out)

Both are plan/todo tools, but they sit on opposite sides of the fidelity
boundary (the AGENTS.md fidelity boundary): Claude Code's TodoWrite is fed back by **loop-owned
system-reminder attachments** every turn — porting it without that machinery
misrepresents the harness. Codex's `update_plan` has **no reminder loop**: the
tool records the plan and returns a plain success string; the model sees plan
state only through its own calls (the TUI display is UI, not model context).
It is a pure, static tool — exactly what packs may reproduce. (`plan_spec.rs`
defines only the spec; the handler stores + acks.)

### 1.3 Deferred (reserved seams)

Unified exec (PTY) · `view_image` · hosted `web_search` · MCP tools ·
`request_permissions` / approval params on shell (§4.1 gap) · the per-model
`gpt-5.x-codex` prompt variants (§4.7) · AGENTS.md (`<user_instructions>`)
loading — same session-start-context question as the claude pack's CLAUDE.md
(its Q3); deferred consistently (§9 Q4).

---

## 2. Module layout

```
crates/locode-packs/src/
├── apply_patch/              # SHARED pure library (no I/O): the patch parser
│   ├── mod.rs                #   parse_patch(), Hunk/UpdateFileChunk, ParseError
│   └── seek.rs               #   seek_sequence fuzzy matcher (4 strictness levels)
└── codex/
    ├── mod.rs                # CodexPack + Pack impl + tests
    ├── prompt.rs             # base-prompt rendering + environment_context + strip_identity
    ├── shell.rs              # shell_command
    ├── patch.rs              # apply_patch tool (parser + Host I/O + result rendering)
    ├── plan.rs               # update_plan
    ├── templates/
    │   ├── prompt.md                       # VERBATIM copy of models-manager/prompt.md
    │   ├── apply_patch.lark                # VERBATIM copy of the grammar
    │   └── apply_patch_instructions.md     # VERBATIM copy of the instructions template
    └── snapshots/            # byte-frozen preamble goldens
```

- **`apply_patch/` is a sibling module, not codex-internal**, because ADR-0012
  and SPEC both promise "a shared `apply_patch` parser" (the opencode pack and
  our `locode` pack may reuse it). It is pure text → `Vec<Hunk>`; all I/O stays
  in `codex/patch.rs` behind the Host seam (ADR-0008). (A standalone crate
  would be an ask-first crate-boundary change — not needed for one in-workspace
  consumer set; revisit if externalized.)
- Template files follow the grok provenance pattern exactly: verbatim bytes,
  module-doc provenance (path + submodule commit + sha256), byte-pin tests,
  never edited in place.

---

## 3. Key types & signatures

### 3.1 The pack

```rust
#[derive(Debug, Default, Clone, Copy)]
pub struct CodexPack;

impl Pack for CodexPack {
    fn name(&self) -> &'static str { "codex" }

    fn register(&self, host: &Arc<Host>, registry: &mut Registry) {
        registry.register("shell_command", CodexShellCommand::new(Arc::clone(host)));
        registry.register("apply_patch",   CodexApplyPatch::new(Arc::clone(host)));
        registry.register("update_plan",   CodexUpdatePlan::new());
    }

    fn preamble(&self, ctx: &PackContext) -> Vec<Message>;   // §4.8
}
```

### 3.2 `shell_command` args (verbatim descriptions from `shell_spec.rs:157-225`)

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]                 // codex: additionalProperties=false (:225)
pub(crate) struct ShellCommandArgs {
    #[schemars(description = "Shell script to run in the user's default shell.")]
    command: String,
    #[schemars(description = "Working directory for the command. Defaults to the turn cwd.")]
    #[serde(default)]
    workdir: Option<String>,
    #[schemars(description = "Maximum command runtime. Defaults to 10000 ms.")]
    #[serde(default)]
    timeout_ms: Option<u64>,
}
```

Dropped from codex's schema (documented gaps): `login` (our host's shell mode
is a HostConfig, not a per-call arg), and the entire approval-parameter block
(`sandbox_permissions` / `justification` / `prefix_rule` /
`additional_permissions`, `create_approval_parameters`,
`shell_spec.rs:298-344`) — approvals/sandbox are the interactive permission
flow this repo excludes by charter (ADR-0001, ADR-0008; same category as the
claude pack dropping `dangerouslyDisableSandbox`).

Tool `description()` verbatim (non-Windows variant, `shell_spec.rs:208-210`):

```
Runs a shell command and returns its output.
- Always set the `workdir` param when using the shell_command function. Do not use `cd` unless absolutely necessary.
```

### 3.3 `apply_patch` — the freeform tool on the typed contract

```rust
pub(crate) struct CodexApplyPatch { host: Arc<Host> }

/// Decodes BOTH deliveries through the one dispatch door (Task 18 §4.6):
/// - native custom_tool_call → wire hands dispatch `Value::String(patch)`;
/// - degraded JSON framing  → `{"input": "<patch>"}`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum ApplyPatchArgs {
    Raw(String),
    Wrapped { input: String },
}
// JsonSchema impl: hand-written — schema_for the FALLBACK shape only:
//   {type:"object", properties:{input:{type:"string", description:"The entire
//    contents of the apply_patch command"}}, required:["input"],
//    additionalProperties:false}
// (parameters_schema is what degraded wires publish; the native wire publishes
//  the grammar via input_format() and never reads the JSON schema.)

impl Tool for CodexApplyPatch {
    type Args = ApplyPatchArgs;
    type Output = ApplyPatchOutput;   // { changed: Vec<{path, kind}>, … } + summary prompt_text
    fn kind(&self) -> ToolKind { ToolKind::Edit }
    fn description(&self) -> &str {
        // VERBATIM, apply_patch_spec.rs:9-27:
        "Use the `apply_patch` tool to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON."
    }
    fn input_format(&self) -> ToolInputFormat {           // Task 18 §3.1
        ToolInputFormat::Freeform {
            syntax: GrammarSyntax::Lark,
            definition: include_str!("templates/apply_patch.lark").to_string(),
        }
    }
    async fn run(&self, ctx, args) -> … // §4.3
}
```

The Lark grammar is copied byte-exact (`core/src/tools/handlers/apply_patch.lark`,
19 lines — `start: begin_patch hunk+ end_patch`, `add_hunk: "*** Add File: "…`,
`change_line: ("+" | "-" | " ")…`, `eof_line: "*** End of File" LF`). The
multi-environment `Environment ID` rule injection (`apply_patch_spec.rs:10-17`)
is NOT ported — single-environment headless.

**Key insight recorded for reviewers:** the grammar is a *server-side
constraint spec* — no Lark runtime exists client-side anywhere in codex; the
client parses patches with a hand-written parser. We port the parser, ship the
grammar as bytes.

### 3.4 The shared parser (`apply_patch/`)

Ported from the `apply-patch` crate (`apply-patch/src/parser.rs`,
`seek_sequence.rs`):

```rust
pub fn parse_patch(patch: &str) -> Result<Vec<Hunk>, ParseError>;   // parser.rs:130-137

pub enum Hunk {                                                     // parser.rs:64-82
    AddFile   { path: PathBuf, contents: String },
    DeleteFile{ path: PathBuf },
    UpdateFile{ path: PathBuf, move_path: Option<PathBuf>, chunks: Vec<UpdateFileChunk> },
}
pub struct UpdateFileChunk {                                        // parser.rs:114-128
    pub change_context: Option<String>,   // the "@@ …" anchor line
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    pub is_end_of_file: bool,             // "*** End of File"
}
pub enum ParseError {                                               // parser.rs:55-61
    InvalidPatchError(String),
    InvalidHunkError { message: String, line_number: usize },
}

/// seek_sequence.rs:1-99 — find old_lines in the file at decreasing strictness:
/// (1) exact; (2) rstrip; (3) strip; (4) Unicode-punctuation normalize.
/// `start` biases the search forward; `eof` biases to end-of-file first.
pub fn seek_sequence(lines: &[String], pattern: &[String], start: usize, eof: bool)
    -> Option<usize>;
```

Markers verbatim (`parser.rs:37-45`): `*** Begin Patch`, `*** End Patch`,
`*** Add File: `, `*** Delete File: `, `*** Update File: `, `*** Move to: `,
`@@ ` / `@@`, `*** End of File`. **Lenient mode is the only mode**
(`PARSE_IN_STRICT_MODE = false`, `parser.rs:53`): tolerate the heredoc wrapper
(`<<'EOF' … EOF`) GPT models emit around patches (`parser.rs:139-176,217-239`)
— port that tolerance.

### 3.5 `update_plan` (verbatim from `plan_spec.rs:7-58`)

```rust
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdatePlanArgs {
    #[schemars(description = "Optional explanation for this plan update.")]
    #[serde(default)]
    explanation: Option<String>,
    #[schemars(description = "The list of steps")]
    plan: Vec<PlanItemArg>,
}
#[derive(…)]
pub(crate) struct PlanItemArg {
    #[schemars(description = "Task step text.")]
    step: String,
    #[schemars(description = "Step status.")]
    status: PlanStatus,          // enum: pending | in_progress | completed
}
```

Description verbatim (`plan_spec.rs:44-47`):

```
Updates the task plan.
Provide an optional explanation and a list of plan items, each with a step and status.
At most one step can be in_progress at a time.
```

Behavior: validate ≤1 `in_progress` (soft error otherwise, matching the
contract the description states), store the latest plan in the tool
(`Mutex<Option<Plan>>` — report-visible via the structured output face), ack
with codex's success text (read the handler's exact string at implementation
time; the spec file defines only the schema — provenance note in `plan.rs`).

---

## 4. Behavior / algorithms

### 4.1 `shell_command`

- Through `Host::exec` (ADR-0008); `workdir` resolved against `ctx.cwd`
  (relative allowed; jail policy applies to nothing here — the shell is never
  path-jailed, SPEC assumption 4).
- **Default timeout 10_000 ms** — the schema's own words ("Defaults to 10000
  ms."). No documented max in the spec; clamp to our host ceiling (5 min,
  `ExecLimits`) and note it. Contrast grok/claude (120s defaults): codex's
  10s default is a *real* behavioral divergence — preserve it, it shapes how
  the model batches commands.
- **Model-facing output format — verbatim port of
  `format_exec_output_for_model`** (`core/src/tools/mod.rs:77-103`):

  ```
  Exit code: {exit_code}
  Wall time: {duration_seconds} seconds        # rounded to 1 decimal (:83)
  Total output lines: {n}                      # only when truncated (:91-102)
  Output:
  {output}
  ```

  Timeout: prepend `command timed out after {ms} milliseconds` to the output
  (`build_content_with_timeout`, `:116-121`) and report **exit code 124**
  (`EXEC_TIMEOUT_EXIT_CODE`, `core/src/exec.rs:65`). A non-zero exit is a
  successful capture (soft path), per ADR-0004 — codex agrees (output goes
  back as a normal function_call_output).
- Truncation: codex truncates via `truncate_text(content, truncation_policy)`
  (`mod.rs:89`) with token-based budgets; we apply the host byte-cap +
  central dispatch-door truncation (the standing locode substitution — same
  note as the other packs) and set the `Total output lines` header from the
  pre-truncation line count.

### 4.2 `apply_patch` — parse

`ApplyPatchArgs → &str` (either variant) → `apply_patch::parse_patch`.
`ParseError` → `ToolError::Respond` with the parser's message (both error
variants carry model-actionable text with line numbers — codex returns these
to the model the same way).

### 4.3 `apply_patch` — apply (in `codex/patch.rs`, over the Host)

Per hunk, in patch order; first failure aborts the whole call with a soft
error naming the hunk (codex applies all-or-nothing per invocation — the
deep-dive's affected-paths pass validates before mutation; we mirror:
**two-phase = validate all hunks (read files, locate chunks), then write**):

- `AddFile{path, contents}` → create (parents included); path resolved under
  the jail; existing file → soft error.
- `DeleteFile{path}` → remove; missing → soft error.
- `UpdateFile{path, move_path, chunks}` → read file → for each chunk in order:
  locate `old_lines` via `seek_sequence` (starting after the previous chunk's
  match; `change_context` line, when present, is located first and anchors the
  search — parser semantics per `parser.rs`/deep-dive) → splice `new_lines`.
  `is_end_of_file` chunks bias the search to EOF (`seek.rs` `eof` flag).
  No match at any strictness → soft error quoting the failing chunk. Then
  write to `move_path` (and remove the original) when set, else in place.
- All paths jail-resolved (`PathPolicy`), errors soft (`Respond`) — jail
  substitution note as usual.

Result faces: structured `{changed: [{path, kind: add|update|delete|move}]}`;
prompt_text = codex's success summary shape (exact string from
`handlers/apply_patch.rs` at implementation time — the deep-dive documents a
per-file summary; byte-pin once ported).

### 4.4 `update_plan`

Pure state + ack (§3.5). No reminders, no injection — see §1.2.

### 4.5 Schemas: `deny_unknown_fields` everywhere

Codex emits `additionalProperties: false` on its function tools
(`shell_spec.rs:225`, `plan_spec.rs` parameters) — mirror with serde attr, as
in the claude pack.

### 4.6 Cross-wire delivery story (the pack's defining constraint)

| Wire | `apply_patch` delivery | Mechanism |
|---|---|---|
| `openai-responses` (native, OpenAI models) | **freeform custom tool + Lark grammar**; model emits `custom_tool_call` raw text | Task 18 §4.6; probe P2 proved the loop live |
| `openai-responses` + xAI models | JSON fallback `{input: string}` (xAI 422s `custom`) | config `custom_tools_supported=false` |
| `anthropic` | JSON fallback | ADR-0012's reserved framing; the wire never sees `Freeform` natively |
| `openai-chat` | JSON fallback | Task 17 §4.2 |

The tool itself is delivery-agnostic (`ApplyPatchArgs` decodes both shapes,
§3.3) — **no pack code branches on the wire**. What changes across wires is
only what the server constrains: with the grammar, malformed patches are
impossible; degraded, the parser's soft errors do the teaching. That asymmetry
is itself an A/B result the report captures via `api_schema`.

`shell_command`/`update_plan` are plain JSON tools — identical on every wire.

### 4.7 The base prompt (`prompt.rs` + `templates/prompt.md`)

**Which prompt is "the" codex prompt:** the compiled default is
`models-manager/prompt.md` (20 903 bytes), embedded as `BASE_INSTRUCTIONS`
(`models-manager/src/model_info.rs:17`) and byte-identical to
`protocol/src/prompts/base_instructions/default.md` (verified `diff`-identical
at the pinned commit). The `core/gpt_5_*.md` files are **reference snapshots,
not compiled sources** — live per-model prompts come from the bundled
`models.json` catalog (`models-manager/src/lib.rs:12-15`; 8 models carry
`base_instructions`). We port the **default `prompt.md`** verbatim (identity
opener: *"You are a coding agent running in the Codex CLI, a terminal-based
coding assistant. Codex CLI is an open source project led by OpenAI. You are
expected to be precise, safe, and helpful."*); per-model variants (e.g.
gpt-5-codex's 6 647-byte prompt) are a deferred config knob — we run arbitrary
models, so the model-independent default is the honest baseline.

- Provenance: sha256 + submodule commit + byte length pinned (grok pattern,
  `grok/prompt.rs` `template_copy_is_pinned`).
- **No template engine needed**: codex's prompt.md is plain markdown (its only
  templating is the `{{ personality }}` placeholder machinery for
  catalog prompts, `model_info.rs:22-23,75-102` — absent from the default
  prompt). Assert no `{{` tokens at build.
- `strip_identity` (PackContext knob, default off): remove the first two
  identity sentences ("You are a coding agent running in the Codex CLI, a
  terminal-based coding assistant. Codex CLI is an open source project led by
  OpenAI. ") — the third sentence ("You are expected to be precise…") and
  everything after stays. Pin test guards rewording.
- **`apply_patch` instructions** (`prompts/templates/
  apply_patch_tool_instructions.md`, 3 084 bytes, `prompts/src/apply_patch.rs:2-3`):
  shipped as a verbatim template; appended to the preamble as a second System
  block **only in degraded delivery** is tempting but wire-invisible to the
  pack — instead: config knob on `CodexPack`-construction/`PackContext`?
  Proposal (§9 Q2): **append by default** (a separate System text block after
  the base prompt) — our target models are not codex-tuned, and in degraded
  JSON delivery there is no grammar to teach the format; codex itself ships
  these instructions for exactly the models that need them.

### 4.8 Preamble

```rust
fn preamble(&self, ctx: &PackContext) -> Vec<Message> {
    vec![
        Message { role: Role::System,
                  content: vec![text(prompt::base_prompt(ctx)),
                                /* + text(APPLY_PATCH_INSTRUCTIONS) per §4.7 */] },
        Message { role: Role::User,
                  content: vec![text(prompt::environment_context(ctx))] },
    ]
}
```

- **System** → on the `openai-responses` wire this lands in `instructions`
  (Task 18 §4.2) — reproducing codex's real request shape
  (`ResponsesApiRequest.instructions`, `client.rs:862`).
- **`<environment_context>`** rides as a leading **user** input item — codex's
  placement (`ContextualUserFragment` → user-role `Message` items,
  `core/src/context/world_state/environment.rs:153-209`; markers
  `ENVIRONMENT_CONTEXT_OPEN_TAG`/`_CLOSE_TAG`, `protocol/src/protocol.rs:104-105`).
  Body: port the pinned renderer (`core/src/context/environment_context.rs`,
  `render()` — nested `<filesystem>`/`<workspace_roots>`/`<shell>` etc. with
  XML escaping, `:198-210`) restricted to the fields `PackContext` has
  (cwd/workspace root, shell, OS); exact bytes fixed against source at
  implementation with a byte-pin test. Same pattern as grok's `<user_info>`
  (env in the first user message, NOT the system prompt).
- **`<user_instructions>` (AGENTS.md)** — NOT rendered in v0 (§9 Q4; the
  claude pack defers CLAUDE.md identically). Tag constants
  (`protocol.rs:102-103`) noted for when it lands.
- Date: codex has no date line in its static context (its `current_time` tool
  is feature-gated) — nothing to add; `PackContext.date` goes unused here.
  Honest per-harness difference.

---

## 5. Design decisions (source `file:line` · why · why-not · differences)

1. **Three-tool subset is the faithful codex.** — *Source:* registration
   `spec_plan.rs:579-619,703-745`; gates: unified-exec per-model
   (`:653-676`), view_image/web_search/MCP/feature flags (`:288-315,693-745`).
   *Why:* a stock codex session (non-Windows, defaults) exposes essentially
   shell + apply_patch + update_plan (+ view_image); there is genuinely no
   read/grep tool to port. *Why-not (add read/grep "for usability"):* that
   would be the `locode` pack's job — fidelity is the contract (ADR-0012).
   *Difference:* grok 5 tools / claude 6 / codex 3 — the tool-surface axis of
   the A/B.

2. **`apply_patch` stays a typed `Tool` with an untagged two-shape `Args`.** —
   *Source:* Task 18 §3.1/§4.6 (`input_format()` + `Value::String` dispatch);
   codex freeform-only at pin (`ApplyPatchToolType`,
   `protocol/src/openai_models.rs:284-288`; spec `apply_patch_spec.rs:9-27`);
   the JSON `{input}` shape is codex's own historical fallback (survey
   apply-patch-deep-dive; removed at pin — fallback shape is ours to keep).
   *Why:* one dispatch door (ADR-0008), one tool for both deliveries, zero
   registry special-casing. *Why-not (a DynTool bypass):* loses typed-contract
   guarantees for no gain — `String` satisfies `DeserializeOwned + JsonSchema`.

3. **Hand-ported parser; grammar shipped as bytes only.** — *Source:* parser
   `apply-patch/src/parser.rs:130-196`, markers `:37-45`, lenient heredoc
   `:139-176,217-239`; fuzzy `seek_sequence.rs:1-99`; no client-side Lark
   runtime anywhere in codex. *Why:* the grammar constrains the *server*; the
   client's ground truth is the parser — porting both keeps them decoupled the
   way codex has them. *Why-not (a Rust Lark/pest dependency to "validate
   against the grammar"):* codex doesn't; double-validation would create
   parser-vs-grammar drift bugs and an ask-first dep.

4. **Lenient-only parsing incl. heredoc tolerance.** — *Source:*
   `PARSE_IN_STRICT_MODE = false` (`parser.rs:53`); heredoc stripping for
   GPT-4.1-style invocations (`:139-176`). *Why:* faithful — this tolerance is
   load-bearing field behavior, models really emit the wrapper. *Difference:*
   grok/claude edits have no analog (exact-string tools).

5. **Fuzzy matching: the exact 4-level ladder.** — *Source:*
   `seek_sequence.rs:3-4,34,44,57,68-99` (exact → rstrip → strip → Unicode
   punctuation normalize; EOF bias). *Why:* this ladder IS codex's edit
   semantics — its tolerance profile differs measurably from grok's
   byte-exact `search_replace` and claude's uniqueness-gated `Edit`; flattening
   it would contaminate the A/B. *Why-not (byte-exact like grok):* wrong
   harness.

6. **`update_plan` in, TodoWrite-style reminder machinery nonexistent.** —
   *Source:* `plan_spec.rs:7-58` (spec only; no attachment/reminder path in
   codex for plans). *Why/why-not:* §1.2 — the fidelity boundary cuts between
   the two look-alike tools, which is itself evidence the boundary is
   well-drawn.

7. **Default `prompt.md` as the pack prompt; catalog variants deferred.** —
   *Source:* `model_info.rs:17,142` (embedded default);
   `models.json` per-model `base_instructions` (`lib.rs:12-15`); `core/*.md`
   = uncompiled reference snapshots (verified: no `include_str!` outside
   default/test). *Why:* model-independent baseline for arbitrary models; the
   catalog mechanism is codex-release-coupled. *Why-not (gpt-5-codex
   variant):* only correct when running that model family; knob later.
   *Difference:* grok = minijinja template; claude = TS section constants;
   codex = one plain markdown file — three provenance patterns, one pinning
   discipline.

8. **`environment_context` as a leading user item; System → `instructions`.** —
   *Source:* codex fragments are user-role input items
   (`world_state/environment.rs:153-209`), base prompt is `instructions`
   (`client.rs:862`); Task 18 §4.2 makes our System hoist land there. *Why:*
   byte-level request fidelity on codex's own wire. *Difference:* grok's env
   block is `<user_info>`, claude's is in-system — each pack reproduces its
   harness's placement (ADR-0013 gives them the roles to say so).

9. **Approval/sandbox params dropped.** — *Source:* `shell_spec.rs:298-344`
   (approval params), `request_permissions` gate (`spec_plan.rs:723-725`).
   *Why:* no interactive permission flow in this repo (ADR-0001); the params
   only mean something to that flow. *Why-not (accept-and-ignore):* a schema
   promising `with_escalated_permissions` that does nothing is worse than its
   absence — listed as the pack's top faithfulness gap instead.

---

## 6. Tests

**Parser (`apply_patch/` unit suite — the deep test surface):**
- add / delete / update round-trips; multi-hunk; multi-chunk update with `@@`
  context anchors; `*** Move to:`; `*** End of File` EOF bias.
- Fuzzy ladder: match at each strictness level (trailing-ws, leading-ws,
  Unicode punctuation — e.g. curly quote vs ASCII), and a no-match soft
  failure quoting the chunk.
- Lenient heredoc: `<<'EOF' … EOF`-wrapped patch parses identically.
- Error taxonomy: missing Begin/End markers, malformed hunk header, garbage
  mid-hunk → `InvalidPatchError`/`InvalidHunkError` with line numbers.

**Tools via `build_registry` + `dispatch` (tempdir host):**
- shell_command: echo; non-zero exit soft-ok with `Exit code: N` header;
  timeout → 124 + "command timed out after" prefix; output-format golden
  (exact header layout incl. `Wall time:` rounding); workdir honored.
- apply_patch: Add creates (with parents); Update via fuzzy match; Delete;
  Move; two-phase atomicity (second hunk invalid → first hunk NOT applied);
  jail escape → soft error; both arg shapes (`Value::String` and
  `{"input": …}`) dispatch identically.
- update_plan: valid plan acks; two `in_progress` → soft error; report record
  carries the structured plan.

**Spec/schema goldens:** the three `ToolSpec`s serialized (freeform
`apply_patch` carries `{syntax:"lark", definition}` matching the byte-pinned
grammar; `shell_command`/`update_plan` schemas with verbatim descriptions +
`additionalProperties:false`); plus the degraded `{input: string}` rendering
via the shared wire helper.

**Prompt/preamble:** template byte-pins (length + sha256 + opener) for all
three template files; preamble snapshots (System [+instructions block] + the
`<environment_context>` user item); `strip_identity` branch + pin;
no-`{{`-token assert.

**Wire integration (with Task 18, canned server):** one codex-pack request
through `OpenAiResponsesProvider` — asserts `instructions` == base prompt,
tools array contains the custom tool, and a scripted `custom_tool_call`
round-trips into a dispatched patch. Live: covered by Task 18's smoke #1 run
with `--harness codex` (manual, post-merge).

---

## 7. Dependencies to add

**None.** Parser and matcher are std-only ports; templates ship via
`include_str!`; the pack reuses the in-tree stack. (Explicitly rejected:
`lark`/`pest` grammar runtimes — §5.3.)

---

## 8. Proposed ADR/SPEC deltas (apply at implementation time — do NOT edit now)

### 8.1 ADR-0012 (harness packs) — dated amendment

> **Amendment (Task 19): codex pack scope + freeform delivery.** The `codex`
> pack ports codex's stock headless trio — `shell_command`, `apply_patch`,
> `update_plan` — with verbatim names/schemas/descriptions, codex's 10s default
> shell timeout and `Exit code:`/`Wall time:` output framing, the hand-ported
> patch parser (lenient heredoc tolerance; 4-level fuzzy matching), and the
> default `prompt.md` base instructions (provenance-pinned). The "apply_patch
> and provider coupling" section is now half-superseded: with the
> `openai-responses` wire (Task 18) the **freeform grammar delivery is real**,
> not deferred; the JSON-string `{input}` framing remains the automatic
> degradation on the Anthropic/Chat wires and for models rejecting custom
> tools. The parser lives in `locode-packs::apply_patch` as the shared library
> this ADR promised. Approval/sandbox parameters and unified exec are excluded
> (no interactive permission flow in this repo — ADR-0001).

### 8.2 SPEC.md

- Success criterion 7 + Open Question 2 ("When to add `apply_patch` — with the
  codex pack"): mark landed by Tasks 18+19; apply_patch is grammar-delivered on
  `openai-responses` and JSON-framed elsewhere.
- Project-structure blurb for `locode-packs`: mention the shared
  `apply_patch` parser module.

### 8.3 No ADR-0003 delta beyond Task 18's

The freeform `ToolSpec`/`input_format()` change is Task 18's amendment (its
§8.1); this pack is its first consumer.

---

## 9. Open questions (for user sign-off)

1. **Tool subset {shell_command, apply_patch, update_plan}** — confirm,
   especially `update_plan` IN (§1.2) and `view_image`/unified-exec OUT.
2. **apply_patch instructions block:** append the 3 084-byte
   `apply_patch_tool_instructions.md` as a second System block **by default**
   (my proposal — non-codex-tuned models + degraded delivery need it), or only
   when delivery is degraded, or never (strict fidelity to codex-on-gpt-5.x,
   which relies on model training)? This is the pack's main
   faithfulness-vs-effectiveness call.
3. **Default prompt = `models-manager/prompt.md`** (model-independent default)
   with per-model catalog variants deferred — confirm (§4.7).
4. **AGENTS.md (`<user_instructions>`) loading deferred** — consistent with
   the claude pack's CLAUDE.md deferral (its Q3). If you want session-start
   file context in packs, both should gain it together via a `preamble`-time
   host handle (a `Pack` trait change) — separate mini-task. Confirm deferral.
5. **`workdir`/patch-path jail posture:** patch paths and `workdir` resolve
   under the standard `PathPolicy` (jailed by default, `--yolo` opt-out).
   Codex itself has a sandbox-approval matrix here that we replace with the
   jail (documented substitution). Confirm.
6. **Two-phase apply (validate-all-then-write, §4.3)** — codex's affected-path
   validation implies it, but if implementation-time source reading shows
   codex applies hunk-by-hunk with partial success, faithfulness says copy
   that instead. Pre-authorize following the source, whichever it shows?
