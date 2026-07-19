# Task 20 — `locode-packs` claude pack (faithful port of Claude Code's tools + prompt)

> Implementation plan, written **before** code. Faithful mimicry per AGENTS.md: the
> pack reproduces Claude Code's **real tools** (names, arg schemas, verbatim
> descriptions, caps, guardrails) and its **static system prompt + static preamble** —
> and nothing loop-adjacent (STATUS.md open concern #9: reminder machinery, injection
> cadence, compaction policy stay on the one shared engine). Cites the reconstructed
> Claude Code source under `~/dev/coding-cli-survey/submodules/claude-code` (submodule
> commit `6a2590911df240ff5ea56aa355696cfb94d128cb`, read 2026-07-18) as `file:line`,
> plus `survey/01-claude-code/*`. Repo precedent: the grok pack (Tasks 9–13) is the
> pattern for tool ports, template provenance, byte-pin tests, and the
> `strip_identity` knob.
>
> **Wire independence:** unlike the codex pack (Task 19), the claude pack has **no
> wire dependency** — every tool is a JSON-schema function tool and Claude Code's
> native wire *is* our Anthropic wire (Task 12). It runs unchanged on
> `--api-schema anthropic | openai-responses | openai-chat`.

---

## 1. Purpose & scope

Port Claude Code's headless-relevant toolset and static system prompt as
`--harness claude`, the second studied-harness pack (ADR-0012). This is the
highest-value A/B counterpart to the grok pack: same engine, same wire, genuinely
different tool surface (dedicated `Write`/`Glob`, a far richer `Grep`, a
read-before-edit **runtime gate** grok lacks) and a much larger prompt.

### 1.1 Full tool inventory (what exists, commit `6a25909`)

From the registry `getAllBaseTools()` (`src/tools.ts:193-251`) and the name-string
sweep (`*/constants.ts`, `*/prompt.ts`):

| Tool name | Source dir | Headless-v0 verdict |
|---|---|---|
| `Bash` | `tools/BashTool` | **port** |
| `Read` | `tools/FileReadTool` | **port** |
| `Edit` | `tools/FileEditTool` | **port** |
| `Write` | `tools/FileWriteTool` | **port** |
| `Glob` | `tools/GlobTool` | **port** |
| `Grep` | `tools/GrepTool` | **port** |
| `TodoWrite` | `tools/TodoWriteTool` | defer — see §1.2 |
| `Agent` (alias `Task`) | `tools/AgentTool` | defer — subagent runtime = a second loop |
| `WebFetch` / `WebSearch` | `tools/WebFetchTool`, `WebSearchTool` | defer — network/server-side; no host seam yet |
| `NotebookEdit` | `tools/NotebookEditTool` | defer — niche; `Read` covers inspection |
| `Skill`, `AskUserQuestion`, `ExitPlanMode`, `EnterPlanMode` | various | defer — interactive (`requiresUserInteraction()`, `permissions.ts:1230-1236`) |
| `SendMessage`, `TaskCreate/Get/List/Output/Stop/Update`, `TeamCreate/Delete`, `Cron*`, `Sleep`, `REPL`, `PowerShell`, `LSP`, `Config`, `RemoteTrigger`, `ToolSearch`, MCP tools | various | defer — feature-flag / `USER_TYPE==='ant'` gated (`tools.ts:16-134,214-244`) or interactive infra |

**Headless-v0 subset = { Bash, Read, Edit, Write, Glob, Grep } — 6 tools.**
Rationale:
- Claude Code itself ships a minimal-floor proof: `CLAUDE_CODE_SIMPLE` reduces the
  pool to `[BashTool, FileReadTool, FileEditTool]` (`tools.ts:287`) — a working
  agent needs even less than our six.
- These six have **static schemas, no interactive dependency, and host-implementable
  behavior**; everything else is gated on subagent runtime, permission UI, network
  services, or ant-internal flags.
- They align 1:1 with our `ToolKind` set (`Shell/Read/Edit/Write/Glob/Grep`) — the
  cross-pack A/B axis (ADR-0003) gets its first full-width comparison vs grok's 5.

### 1.2 Why TodoWrite is deferred (the fidelity boundary, concern #9)

`TodoWrite` (`TodoWriteTool.ts:13-17`) is schema-trivial, but its *behavior* is
inseparable from Claude Code's system-reminder machinery: the tool's value is that
the loop re-injects the todo list as `<system-reminder>` attachments each turn
(`utils/attachments.ts` todo reminders) and the prompt's task-management section
coaches the model against that cadence. Porting the tool without the reminders
would be an **unfaithful** TodoWrite (a write-only stub the model never sees
again), and porting the reminders violates the fidelity boundary (STATUS #9:
loop-adjacent behaviors stay on the shared engine). Deferred until the "turn
hooks" ADR moment; listed as a faithfulness gap in the pack docs.

### 1.3 Deferred (reserved seams, not this task)

- **Subagents** (`Agent`/`Task`) — a nested loop; `DEFAULT_AGENT_PROMPT`
  (`constants/prompts.ts:758`) noted for when it lands.
- **WebFetch/WebSearch** — needs an HTTP-fetch host seam + a server-side search
  backend; nothing in `locode-host` today.
- **Persistent shell session** — Claude Code's Bash keeps one stateful shell;
  our host is per-call `bash -lc` (same simplification as the grok pack). Gap
  documented in §4.1.
- **Sandbox knobs** (`dangerouslyDisableSandbox`) and background Bash
  (`run_in_background`) — no OS sandbox / background-task infra in v0 (SPEC
  assumption 4; grok pack dropped `is_background` the same way).
- **PDF/image reads** (`pages` arg, multimodal output) — `Read` is text-only in
  v0, like the grok pack's `read_file`.
- **CLAUDE.md / memory loading** — session-start context injection; needs a
  file-discovery pass the pack can't do purely. See open question Q3.

---

## 2. Module layout

```
crates/locode-packs/src/claude/
├── mod.rs           # ClaudePack + Pack impl + shared ClaudeSessionState wiring + tests
├── prompt.rs        # static system prompt assembly (verbatim section constants) + env section
├── state.rs         # ClaudeSessionState: read-file freshness store (Read/Edit/Write share it)
├── bash.rs          # Bash
├── read.rs          # Read
├── edit.rs          # Edit
├── write.rs         # Write
├── glob.rs          # Glob
├── grep.rs          # Grep
├── descriptions/    # verbatim tool-description texts, one file per tool (provenance-pinned)
│   ├── bash.md  read.md  edit.md  write.md  glob.md  grep.md
└── snapshots/       # byte-frozen rendered prompt goldens (headless + interactive)
```

Deltas from the grok pack layout, and why:
- **`descriptions/` as files, not string literals.** Claude Code's tool
  descriptions are *long* (Bash ≈ 21,131 chars, `BashTool/prompt.ts`; TodoWrite
  9,528; Agent 16,672 — vs grok's one-liners). `include_str!` + a provenance
  header per file keeps them verbatim, diffable, and byte-pinnable exactly like
  `grok/templates/prompt.md`. Grok used `#[schemars(description)]` inline because
  its strings are short; the *rule* (verbatim descriptions) is the same, the
  *mechanism* scales to Claude Code's sizes. Field-level descriptions stay inline
  `#[schemars(description = "…")]` (they are short).
- **`state.rs` is new.** Claude Code has a runtime read-before-edit/freshness
  store (`readFileState`; Edit `validateInput` errorCode 6, survey
  `01-claude-code/basic-tools.md`) that grok deliberately lacks. Faithful mimicry
  means the claude pack **ports the gate** even though the grok pack (also
  faithfully) omits it — this is exactly the per-pack behavioral divergence
  ADR-0012 exists to preserve.
- **No `templates/`**: Claude Code's prompt is assembled from TS string constants
  with runtime conditionals (`constants/prompts.ts:444-577`), not a template
  file. We port the **static sections as verbatim Rust constants** (one per
  section, provenance-cited) and assemble in `prompt.rs`, freezing the result in
  snapshots. See §4.7.

---

## 3. Key types & signatures (concrete sketches on the existing APIs)

### 3.1 The pack and its shared session state

```rust
/// The claude harness pack. NOT a unit struct: Read/Edit/Write share one
/// per-pack-instance freshness store (Claude Code's readFileState), so the
/// pack constructs it and hands clones to the three tools at register().
#[derive(Debug, Default)]
pub struct ClaudePack;

impl Pack for ClaudePack {
    fn name(&self) -> &'static str { "claude" }

    fn register(&self, host: &Arc<Host>, registry: &mut Registry) {
        let state = Arc::new(ClaudeSessionState::default());
        registry.register("Bash",  ClaudeBash::new(Arc::clone(host)));
        registry.register("Read",  ClaudeRead::new(Arc::clone(host), Arc::clone(&state)));
        registry.register("Edit",  ClaudeEdit::new(Arc::clone(host), Arc::clone(&state)));
        registry.register("Write", ClaudeWrite::new(Arc::clone(host), Arc::clone(&state)));
        registry.register("Glob",  ClaudeGlob::new(Arc::clone(host)));
        registry.register("Grep",  ClaudeGrep::new(Arc::clone(host)));
    }

    fn preamble(&self, ctx: &PackContext) -> Vec<Message>;  // §4.8
}
```

Registration names are Claude Code's **exact UpperCamelCase wire names**
(`Bash`, `Read`, … — `BashTool.tsx`, `FileReadTool.ts` constants; contrast grok's
snake_case). The registry is name-agnostic, so nothing else changes.

> Note: `register()` is called once per run (`build_registry`), so constructing
> the state inside it gives per-run freshness — matching Claude Code's
> per-session `readFileState`. `GrokPack` stays a zero-sized singleton;
> `ClaudePack` is stateless too (the state lives in the tools).

### 3.2 The freshness store (`state.rs`)

```rust
/// Claude Code's readFileState: path -> mtime observed at last Read.
/// Edit/Write consult it (read-before-edit gate + modified-since-read check);
/// Read and successful Edit/Write update it.
#[derive(Debug, Default)]
pub struct ClaudeSessionState {
    read_state: Mutex<HashMap<PathBuf, SystemTime>>,
}

impl ClaudeSessionState {
    pub fn record_read(&self, path: PathBuf, mtime: SystemTime);
    /// None = never read (errorCode-6 analog); Some(false) = stale
    /// (modified since read); Some(true) = fresh.
    pub fn check_fresh(&self, path: &Path, current_mtime: SystemTime) -> Option<bool>;
}
```

### 3.3 Tool arg structs — verbatim schemas

Claude Code declares every core schema with `z.strictObject` (e.g.
`BashTool.tsx:227-247`, `FileReadTool.ts:227-243`, `FileEditTool/types.ts:6-19`),
i.e. `additionalProperties: false`. Faithful port: `#[serde(deny_unknown_fields)]`
on every Args struct (schemars emits `additionalProperties: false` for it). Field
descriptions are the zod `.describe()` strings **verbatim** via
`#[schemars(description = "…")]` (repo rule, `tool-schema-descriptions` memory).

```rust
/// Bash (BashTool.tsx:227-247). run_in_background + dangerouslyDisableSandbox
/// + _simulatedSedEdit dropped in v0 (§4.1 — gaps documented).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BashArgs {
    #[schemars(description = "The command to execute")]
    command: String,
    #[schemars(description = "Optional timeout in milliseconds (max 600000)")]
    #[serde(default)]
    timeout: Option<u64>,
    #[schemars(description = /* verbatim from BashTool.tsx: the long active-voice
        guidance with ls/git/find/curl examples — stored in descriptions/, see §4.1 */)]
    #[serde(default)]
    description: Option<String>,
}

/// Read (FileReadTool.ts:227-243). `pages` (PDF) dropped in v0.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadArgs {
    #[schemars(description = "The absolute path to the file to read")]
    file_path: String,
    #[schemars(description = "The line number to start reading from. Only provide if the file is too large to read at once")]
    #[serde(default)]
    offset: Option<u64>,       // zod: nonnegative int
    #[schemars(description = "The number of lines to read. Only provide if the file is too large to read at once.")]
    #[serde(default)]
    limit: Option<u64>,        // zod: positive int
}

/// Edit (FileEditTool/types.ts:6-19).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditArgs {
    #[schemars(description = "The absolute path to the file to modify")]
    file_path: String,
    #[schemars(description = "The text to replace")]
    old_string: String,
    #[schemars(description = "The text to replace it with (must be different from old_string)")]
    new_string: String,
    #[schemars(description = "Replace all occurrences of old_string (default false)")]
    #[serde(default)]
    replace_all: bool,
}

/// Write (FileWriteTool.ts:56-65).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteArgs {
    #[schemars(description = "The absolute path to the file to write (must be absolute, not relative)")]
    file_path: String,
    #[schemars(description = "The content to write to the file")]
    content: String,
}

/// Glob (GlobTool.ts:26-36).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GlobArgs {
    #[schemars(description = "The glob pattern to match files against")]
    pattern: String,
    #[schemars(description = /* verbatim, incl. the "IMPORTANT: Omit this field..."
        DO-NOT-enter-undefined sentence */)]
    #[serde(default)]
    path: Option<String>,
}

/// Grep (GrepTool.ts inputSchema) — the full ripgrep passthrough surface.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GrepArgs {
    pattern: String,
    #[serde(default)] path: Option<String>,
    #[serde(default)] glob: Option<String>,
    #[serde(default)] output_mode: Option<GrepOutputMode>, // content|files_with_matches|count
    #[serde(default, rename = "-B")] before: Option<u64>,
    #[serde(default, rename = "-A")] after: Option<u64>,
    #[serde(default, rename = "-C")] context: Option<u64>,
    #[serde(default, rename = "-n")] line_numbers: Option<bool>,
    #[serde(default, rename = "-i")] case_insensitive: Option<bool>,
    #[serde(default, rename = "type")] file_type: Option<String>,
    #[serde(default)] head_limit: Option<u64>,   // "Defaults to 250... Pass 0 for unlimited"
    #[serde(default)] offset: Option<u64>,
    #[serde(default)] multiline: Option<bool>,
}
```

(Descriptions abbreviated here; the implementation carries every `.describe()`
string verbatim — the schema-golden tests in §6 pin them.)

### 3.4 Tool structs

Each tool follows the grok pattern exactly (`grok/terminal.rs`): a struct holding
`Arc<Host>` (+ `Arc<ClaudeSessionState>` for Read/Edit/Write), `impl Tool` with
`kind()` = the matching `ToolKind`, `description()` = `include_str!` from
`descriptions/`, dual `Output` faces (structured report value +
`to_prompt_text()`).

---

## 4. Behavior / algorithms (per tool, faithful caps and guardrails)

Global note: Claude Code caps every result at
`DEFAULT_MAX_RESULT_SIZE_CHARS = 50_000` regardless of per-tool values
(`constants/toolLimits.ts:13`); per-tool `maxResultSizeChars` tightens that
(Bash 30k, Grep 20k, Read ∞). Our central dispatch-door truncation
(`Registry::dispatch` → `truncate_for_model`, ADR-0008 amendment) stays **on
top** as the engine-side belt — same layering as the grok pack.

### 4.1 `Bash`

- **Exec:** through `Host::exec` (`bash -lc`, combined output) — same seam as
  grok's `run_terminal_cmd`. Claude Code's persistent shell session +
  sandbox/permission pipeline is **not** portable headlessly; documented gap
  (grok pack precedent: `is_background` dropped).
- **Timeouts:** default `120_000` ms, hard max `600_000` ms
  (`utils/timeouts.ts`, env-overridable there; we pin the defaults). Contrast
  grok: same default, **max 300k** — a real A/B difference, preserved.
- **Output cap:** `maxResultSizeChars: 30_000` (`BashTool.tsx:424`) — apply as a
  30k-char truncation on the combined output before the prompt face (middle
  truncation with marker, host-provided).
- **Dropped args (gaps):** `run_in_background` (no background infra),
  `dangerouslyDisableSandbox` (no sandbox), `_simulatedSedEdit` (Claude Code
  itself `.omit()`s it from the model-facing schema, `BashTool.tsx:249-259` —
  omitting it is *faithful*).
- **Prompt face:** stdout+stderr combined, exit-code note on failure — mirror
  Claude Code's rendering closely enough for A/B (exact renderer is UI-coupled;
  approximate, flag as P1 per ADR-0012 "names/descriptions P1").

### 4.2 `Read`

- Absolute-path required (description says so; relative → soft error message
  like Claude Code's validate).
- Default window **2000 lines** (`MAX_LINES_TO_READ`, `FileReadTool/prompt.ts:10`),
  `offset`/`limit` window into the file.
- Output format: `cat -n` style — line numbers starting at 1, tab separator —
  per the description text ("Results are returned using cat -n format"); grok
  pack's `read.rs` numbering differs (`N→content`) — genuine A/B texture.
- Long lines: truncate per line at 2000 chars (the description promises "Any
  lines longer than 2000 characters will be truncated"; the reconstructed
  source clips bytes in `utils/readFileInRange.ts` — honor the **documented
  contract**, i.e. the description text, and note the source nuance).
- Empty file / nonexistent → soft `Respond` with Claude Code's wording
  (system-reminder-style warning for empty files).
- **Updates the freshness store** (`state.record_read`) with the file's mtime.
- Jail: paths resolve through `Host` (`PathPolicy` applies — ours, not Claude
  Code's permission system; documented as the standing locode substitution,
  ADR-0008).

### 4.3 `Edit`

Guardrails, in Claude Code's check order (survey `01-claude-code/basic-tools.md`
error codes; `FileEditTool.ts`):

1. **Read-before-edit gate:** file never `Read` this session → soft error
   ("File has not been read yet. Read it first before writing to it.") —
   errorCode 6 analog. THE behavioral divergence from grok (which has no gate).
2. **Staleness:** mtime newer than recorded read → soft error ("File has been
   modified since read...") — re-read required.
3. `old_string == new_string` → soft error (schema note: "must be different").
4. `old_string` not found → soft error.
5. Multiple matches without `replace_all` → soft error (uniqueness rule);
   `replace_all: true` replaces every occurrence.
6. Success updates the freshness store (post-edit mtime) so sequential edits to
   the same file don't false-trip the staleness check.
- File-size cap: `MAX_EDIT_FILE_SIZE = 1 GiB` (`FileEditTool.ts:84`) — port as a
  constant, realistically un-hit.
- **No file creation via Edit** — creation is `Write`'s job (contrast grok's
  empty-`old_string` creation; the packs genuinely differ here and that is the
  point).

### 4.4 `Write`

- Overwrites/creates at absolute `file_path`; parent dirs created (Claude Code
  writes through, our `Host::write_file` does not auto-create parents — add the
  mkdir in the tool, since `Write`'s contract is create-or-overwrite).
- **Must-read-first for existing files:** the description bakes it in ("If this
  is an existing file, you MUST use the Read tool first... This tool will fail
  if you did not read the file first.", `FileWriteTool/prompt.ts:10-18`) and the
  freshness store enforces it: existing file never read → soft error; stale →
  soft error. New file → allowed.
- Success records the new mtime in the store.

### 4.5 `Glob`

- Match `pattern` (optionally under `path`, default cwd), return matching file
  paths **sorted by modification time**, capped at **100** files with a
  `truncated` note (`GlobTool.ts:157`; output text says "limited to 100 files").
- Implementation: `rg --files` under the search root through
  `Host::run_capture` (ADR-0011's resolved `rg`; respects .gitignore like Claude
  Code's globber effectively does), filter with the glob pattern, stat + sort by
  mtime descending, truncate to 100. No new dependency: glob→regex conversion is
  avoidable by passing `--glob <pattern>` to rg itself (`rg --files -g pattern`).
- No matches → soft-ok text ("No files found").

### 4.6 `Grep`

- Full ripgrep passthrough (the schema *is* rg's surface — `-A/-B/-C`, `-n`,
  `-i`, `type`, `glob`, `multiline` → `--multiline --multiline-dotall`).
- `output_mode` default **`files_with_matches`**; `content` adds context/line
  flags; `count` → `--count`.
- `head_limit` default **250** (`GrepTool.ts:108` `DEFAULT_HEAD_LIMIT`), `0` =
  unlimited; applied to lines of rg output post-capture. `offset` skips prior
  lines (pagination).
- Result cap 20_000 chars (`GrepTool.ts:164`).
- rg exit 1 (no match) → soft-ok "No matches found"; exit ≥2 → soft error with
  stderr (same convention as grok pack's grep).

### 4.7 The static system prompt (`prompt.rs`)

Claude Code assembles `string[]` blocks in `getSystemPrompt`
(`constants/prompts.ts:444-577`); the API layer prepends the identity prefix
(`services/api/claude.ts:1358-1369`). The split we port (concern #9 rules the
line):

**IN — static sections, verbatim constants (order preserved):**
1. **Identity prefix** (`constants/system.ts:10-12` + `getCLISyspromptPrefix`
   `:30-46`): headless (`ctx.headless == true`) →
   `"You are a Claude agent, built on Anthropic's Claude Agent SDK."`
   (`AGENT_SDK_PREFIX` — what real non-interactive Claude Code sends);
   interactive → `"You are Claude Code, Anthropic's official CLI for Claude."`
   (`DEFAULT_PREFIX`). Same identity-branch pattern as grok's
   `is_non_interactive` (Task 13).
2. `getSimpleIntroSection` (`prompts.ts:175-184`) — "You are an interactive
   agent that helps users with software engineering tasks…" + the cyber-risk
   refusal (`constants/cyberRiskInstruction.ts`) + the URL-guessing ban.
3. `getSimpleSystemSection` (`:186-197`) — `# System` (minus the hooks bullet:
   we have no hooks; render the no-hooks branch Claude Code itself renders when
   none are configured).
4. `getSimpleDoingTasksSection` (`:199-253`) — `# Doing tasks` (non-ant branch
   only; `USER_TYPE==='ant'` bullets are ant-internal traffic, not ours).
5. `getActionsSection` (`:255-267`) — `# Executing actions with care`.
6. `getUsingYourToolsSection` (`:269-314`) — `# Using your tools`, rendered for
   **our six-tool set** (the section is conditional on `enabledTools` in the
   source; we render exactly the branch matching {Bash,Read,Edit,Write,Glob,Grep},
   which drops the TodoWrite/Task/Skill bullets — the same output real Claude
   Code produces for that pool).
7. `getSimpleToneAndStyleSection` (`:430-442`) — `# Tone and style`.
8. `getOutputEfficiencySection` (`:403-428`) — `# Output efficiency` (non-ant).

**IN — runtime-valued but static-shaped (rendered from `PackContext`):**
9. `# Environment` (`computeSimpleEnvInfo`, `prompts.ts:651-710`): working dir,
   `Is a git repository: <bool>`, `Platform:`, shell line, `OS Version:`, the
   "You are powered by the model named X. The exact model ID is Y." line and
   knowledge-cutoff line (`:659-667`, `getKnowledgeCutoff` `:712-730`).
   Needs `PackContext` growth (open question Q2): `is_git_repo`, and a
   `model: Option<String>` for the powered-by line (skip the line when absent —
   pack shouldn't guess).

**OUT — loop-adjacent, excluded per concern #9 (documented in module docs):**
- The `SYSTEM_PROMPT_DYNAMIC_BOUNDARY` dynamic registry (`prompts.ts:114,491-555`):
  memory/CLAUDE.md, output-style, language, MCP instructions, scratchpad,
  session guidance — all runtime-managed injection.
- Git-status tail (`appendSystemContext`, `utils/api.ts:437-447`) — a per-run
  snapshot produced by running git; loop machinery (open question Q4).
- All `<system-reminder>` attachments (`utils/attachments.ts`) — todo reminders,
  skill discovery, memory surfacing, plan-mode nudges.

Assembly: `prompt.rs` holds one `const` per section (verbatim bytes of the
rendered TS output for our configuration, provenance-commented with
`prompts.ts` line ranges + submodule commit + sha256 of the concatenation),
joins them with Claude Code's block separator (`\n\n` — blocks are separate
system-array entries on the wire, but our protocol carries one System message;
the Anthropic wire re-splits… **no**: keep one text block; Claude Code's
multi-block split exists for its own cache_control granularity, and our wire's
2-marker policy caches the whole system anyway — noted as a deliberate
flattening, §5.6).

`strip_identity` knob (PackContext, grok precedent): removes the identity
prefix block (both variants) from the rendered output; pinning tests keep it
honest.

### 4.8 Preamble & first-user-message shaping

```rust
fn preamble(&self, ctx: &PackContext) -> Vec<Message> {
    vec![
        Message { role: Role::System, content: vec![text(prompt::render(ctx))] },
        Message { role: Role::User,   content: vec![text(prompt::context_reminder(ctx))] },
    ]
}
```

- The **System** message = §4.7's render (identity + static sections + env).
- The **User** message = Claude Code's first-turn context reminder
  (`prependUserContext`, `utils/api.ts:449-474`), which real Claude Code
  prepends as an `isMeta` user message:

  ```
  <system-reminder>
  As you answer the user's questions, you can use the following context:
  # currentDate
  Today's date is {date}.

        IMPORTANT: this context may or may not be relevant to your tasks. You
        should not respond to this context unless it is highly relevant to your task.
  </system-reminder>
  ```

  We render the `currentDate` entry only (format string verbatim, incl. the
  odd indentation); the `# claudeMd` entry is omitted until CLAUDE.md loading
  is decided (Q3). This mirrors the grok pack's `[System(prompt),
  User(<user_info>)]` split exactly — the static preamble is in-scope,
  per-turn re-injection is not.
- Why `Role::User` and not `Role::Developer`: it IS a user-message
  system-reminder on Claude Code's real wire (not a mid-conversation system
  message), and our Anthropic wire would render Developer *as* a
  `<system-reminder>`-wrapped user block anyway (ADR-0013) — using User with the
  verbatim wrapper keeps the bytes identical on the native wire and honest on
  the OpenAI wires.

---

## 5. Design decisions (each: source `file:line` · why · why-not · differences)

1. **Six-tool headless subset.** — *Source:* registry `tools.ts:193-251`; the
   `CLAUDE_CODE_SIMPLE` floor `tools.ts:287`; interactive gating
   `permissions.ts:1230-1236`. *Why:* static schemas + host-implementable; full
   `ToolKind` coverage for A/B. *Why-not-more:* every additional tool drags in a
   loop dependency (TodoWrite→reminders, Task→subagent loop, WebFetch→network
   seam). *Difference vs grok pack:* 6 vs 5; dedicated `Write` + `Glob` exist
   here, `list_dir` doesn't — comparisons stay honest per ADR-0012.

2. **TodoWrite excluded on fidelity-boundary grounds.** — *Source:*
   `TodoWriteTool.ts:13-17`; reminder machinery `utils/attachments.ts`;
   STATUS.md concern #9. *Why:* the tool minus its reminder loop is a
   misrepresentation of Claude Code, worse for the A/B than absence. *Why-not
   (port tool only):* silently unfaithful. *Difference:* grok's TodoGate was
   excluded from the grok pack for the identical reason — consistent boundary.

3. **Port the read-before-edit + staleness gate (state.rs).** — *Source:* Edit
   validateInput errorCode 6 + readFileState (survey
   `01-claude-code/basic-tools.md`); Write prompt "This tool will fail if you
   did not read the file first" (`FileWriteTool/prompt.ts:10-18`). *Why:*
   faithful mimicry — this is Claude Code's signature edit guardrail. *Why-not
   (skip like grok):* the grok pack skips it because **grok** has none; per-pack
   fidelity is the whole point (ADR-0012). *Difference:* first pack with
   cross-tool shared state; contained in one `Arc<ClaudeSessionState>`.

4. **UpperCamelCase wire names.** — *Source:* name constants (`Bash`, `Read`, …).
   *Why:* the registry key IS the model-facing name (Task 4 identity model);
   faithful means `Bash`, not `bash`. *Why-not (snake_case normalize):* breaks
   fidelity and A/B honesty. *Difference:* grok = snake_case; codex = snake_case.

5. **`deny_unknown_fields` to mirror `z.strictObject`.** — *Source:*
   `BashTool.tsx:227` etc. (`strictObject`). *Why:* Claude Code rejects unknown
   args; schemars emits `additionalProperties:false` from the serde attr, so
   schema and runtime behavior both match. *Why-not (permissive):* grok's args
   are permissive; Claude Code's are not — a real behavioral difference worth
   preserving.

6. **One flattened System text block.** — *Source:* multi-block system built at
   `services/api/claude.ts:1358-1369` + `buildSystemPromptBlocks`; cache split
   `splitSysPromptPrefix` (survey `01-claude-code/provider-api.md:34-37`).
   *Why:* the block split exists for Claude Code's own `cache_control`
   granularity; our wire owns cache placement (2-marker policy, Task 12) and
   ADR-0013 hoists all System text anyway. *Why-not (multi-block System
   messages):* protocol supports it (multiple Text blocks), but it would imply a
   cache semantics we don't implement — revisit if we ever port CC's 3-marker
   system caching.

7. **Headless identity = the Agent SDK prefix.** — *Source:*
   `constants/system.ts:10-12`, selection `:30-46` (`isNonInteractiveSession` →
   `AGENT_SDK_PREFIX`). *Why:* that IS what headless Claude Code sends; we run
   headless. *Why-not (always "You are Claude Code…"):* would be faithful to the
   *interactive* product in a *headless* run — the same reasoning that picked
   grok's `is_non_interactive` branch (Task 13). Interactive branch preserved
   behind `ctx.headless == false`.

8. **Descriptions as provenance-pinned files.** — *Source:* `BashTool/prompt.ts`
   (~21k chars) et al. *Why:* verbatim fidelity at that size needs
   `include_str!` + byte pins (grok `templates/prompt.md` pattern:
   sha256 + commit + length pin, `grok/prompt.rs` tests). *Why-not (inline
   literals):* unreviewable diffs, escaping hazards.

9. **Tool-set-conditional prompt sections rendered for OUR pool.** — *Source:*
   `getUsingYourToolsSection` branches on `enabledTools` (`prompts.ts:269-314`);
   session-guidance bullets likewise (`:352-400`). *Why:* real Claude Code with
   this six-tool pool would render exactly these bytes — that is the faithful
   target, not the maximal prompt. *Why-not (render all bullets):* would
   reference tools the model doesn't have (worse than unfaithful: incoherent).
   Same argument as grok's `test_no_monitor_tool_omits_watch_section`
   precedent (Task 13).

10. **Env block in the system prompt; date reminder in the first user turn.** —
    *Source:* `computeSimpleEnvInfo` in the prompt array (`prompts.ts:651-710`)
    vs `prependUserContext` user message (`utils/api.ts:449-474`, wired
    `query.ts:660`). *Why:* that placement is measured fact (also matches the
    proxy capture in the `claude-code-system-surfaces` note). *Difference vs
    grok:* grok puts env in the FIRST USER message (`<user_info>`), Claude Code
    puts env in SYSTEM and only date/CLAUDE.md in the user turn — the packs
    differ and both are right.

---

## 6. Tests

**Schema goldens (per tool):** serialize each registered `ToolSpec` (name,
description, parameters) to a committed JSON snapshot — pins names, every field
description byte, `additionalProperties:false`, and required lists. (The A/B
contract: schemas ARE the experiment surface.)

**Description provenance pins:** per `descriptions/*.md`: byte-length + opening
line + sha256 pin (grok `template_copy_is_pinned` pattern).

**Prompt snapshots:** byte-frozen goldens for headless + interactive renders
(UPDATE_SNAPSHOTS=1 regen); identity-branch asserts (headless → "You are a
Claude agent, built on Anthropic's Claude Agent SDK."); `strip_identity`
removes both variants (pin test so a section refresh can't silently no-op);
tool-conditional sections: rendered prompt must NOT mention TodoWrite/Task/
WebFetch (our pool excludes them).

**Behavior (via `build_registry` + `dispatch`, tempdir host — grok/mod.rs
pattern):**
- Bash: echo ok; non-zero exit soft-ok; timeout arg honored (+ clamped at 600s);
  30k output truncated.
- Read: numbered `cat -n` output; 2000-line default window; offset/limit;
  missing file soft error; records freshness.
- Edit: **unread file → soft error** (the gate!); read-then-edit succeeds;
  stale file (touch after read) → soft error; `old==new` rejected; not-found;
  multi-match without replace_all rejected; replace_all works; success updates
  freshness (second sequential edit passes).
- Write: new file ok (+ parent dirs); existing-but-unread → soft error;
  read-then-overwrite ok.
- Glob: matches sorted by mtime; >100 truncated note; no-match soft-ok.
- Grep (gated on rg): files_with_matches default; content mode + `-n -i -C`;
  count mode; head_limit; no-match soft-ok.
- Pack: `resolve("claude")` works; specs list exactly 6 tools with exact names.

**No live/network tests** — the pack is wire-independent; live A/B happens via
the binary once merged (`--harness claude`).

---

## 7. Dependencies to add

**None expected.** The pack reuses in-tree `schemars`/`serde`/`async-trait`/
`tokio` + `locode-host` (rg resolution, exec, fs). Glob matching rides on
`rg --files -g <pattern>` (no `globset` dep needed — ask-first avoided
deliberately). If mtime sorting needs nothing beyond `std::fs::metadata`, the
dependency delta is zero. (If implementation finds rg's `-g` semantics diverge
from Claude Code's globber on some pattern class, adding `globset` becomes an
**ask-first** item at that point, with the divergence documented.)

---

## 8. Proposed ADR/SPEC deltas (apply at implementation time — do NOT edit now)

1. **ADR-0012 (harness packs) — dated amendment** (minor):

   > **Amendment (Task 20): claude pack scope.** The `claude` pack ports the
   > six headless-relevant core tools (`Bash`, `Read`, `Edit`, `Write`, `Glob`,
   > `Grep`) with Claude Code's real names, schemas, descriptions, caps, and the
   > read-before-edit/staleness guardrails (a per-run `ClaudeSessionState`).
   > Tools whose behavior is inseparable from loop machinery (`TodoWrite` and
   > its reminder cadence, `Task` subagents) are excluded under the fidelity
   > boundary (STATUS #9): packs reproduce tools + prompts + static preamble;
   > loop-adjacent behaviors stay on the shared engine.

2. **SPEC.md** — success criterion 7 / assumptions 3: mark `codex`/`claude`
   packs as landing (Tasks 19/20), remaining `opencode` + `locode` packs stay
   the next milestone. One-line edit at merge.

3. **No ADR-0003 change**: the six tools fit the existing typed contract; the
   shared `ClaudeSessionState` is tool-internal state, not a contract change
   (`ToolCtx` stays small).

---

## 9. Open questions (for user sign-off before implementation)

1. **Tool subset confirmation.** {Bash, Read, Edit, Write, Glob, Grep}, with
   TodoWrite excluded on fidelity-boundary grounds (§1.2). Alternative: include
   TodoWrite as a knowingly-degraded stub (schema-faithful, reminder-less) for
   schema-level A/B. My recommendation: exclude.
2. **`PackContext` growth for the env block:** add `is_git_repo: bool` and
   `model: Option<String>` (for "Is a git repository:" and "You are powered
   by…" lines, `prompts.ts:651-710`). Exec layer supplies both cheaply. OK?
   (Without them: drop those two lines and list as gaps.)
3. **CLAUDE.md loading.** Claude Code injects CLAUDE.md/AGENTS.md content in the
   first-user-turn reminder (`utils/api.ts:449-474`). It is session-START
   context (arguably static preamble) but needs file discovery + a read at
   preamble time. Proposal: defer (gap), render only `# currentDate`. Sign-off?
4. **Git-status tail** (`appendSystemContext`): also session-start, needs
   running `git status` at preamble build. Proposal: defer with Q3 (both are
   the same "preamble needs the host" question — if you want them, `preamble()`
   likely grows an optional host handle, a `Pack` trait change worth its own
   look).
5. **Bash `description` arg optionality.** Claude Code marks it optional with a
   long guidance description; grok made its analog required. Keep optional
   (faithful). Confirm no schema massaging.
6. **Description texts referencing dropped features.** The Bash description
   (21k) references `run_in_background`, sandbox, TodoWrite etc. Options:
   (a) verbatim-full (schema mentions features that error as unknown fields —
   confusing), (b) trim the paragraphs for dropped features, mirroring how CC
   renders when those features are off **where the source is conditional**, and
   documenting each removed paragraph as a gap where it is not. Proposal: (b),
   with the trimmed spans listed file-by-file in the provenance headers.
7. **Identity default.** Headless default = `AGENT_SDK_PREFIX` (faithful to
   headless CC). If you'd rather A/B against the marquee "You are Claude Code…"
   line even in headless runs, that's a one-line `PackContext` decision — say
   which.
