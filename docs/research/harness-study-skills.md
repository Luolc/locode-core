# Harness study — Agent Skills

> **Source freshness.** Last verified against the `coding-cli-survey` submodules:
> **2026-07-24** (the newest dated note below — this stamp was inferred from the
> document's own history, not from a re-read on 2026-07-27).
> Submodule commits as of 2026-07-27: `claude-code` 6a25909 · `codex` f201c30c · `grok-build` b189869 · `opencode` 1754480.
>
> `AGENTS.md` requires a fresh source re-read when planning each task
> ([`autonomous-workflow.md`](../autonomous-workflow.md) Phase 1). **Update this line
> — date and commits — in the same PR as that re-read.** Without it a reader cannot tell
> whether the `file:line` citations below still point at what they claim — which is how a
> wrong injection point survived months in the subagent study (corrected 2026-07-26, #240).

> **Correction (2026-07-24) — Grok Build has no live `Skill` tool.** This study
> repeatedly describes one (the *Grok Build* section, the comparison table's
> "Body → model via" / "Tool args" rows, and archetype 1). Re-read against the
> published snapshot, that is **wrong**: the grok-native skill tool was deleted
> (`implementations/skills/skill.rs:35-37` — "Old `SkillToolImpl` + `impl Tool`
> deleted", pointing at a `grok_build/skill/` directory absent from the published
> tree), and **no `grok_build` toolset registers a skill tool**
> (`xai-grok-agent/src/config.rs:440-517`). The only registered one is
> `opencode::OpenCodeSkillTool`, used solely by `opencode_toolset()`
> (`registry/types.rs:707`, `config.rs:528`) — an opencode-shaped tool
> (`{name}` only, `<skill_content>` wrapper). Corroborating detail: the listing
> header function takes `_tool_name` (discarded) and hardcodes "The following
> skills are available for use:", the no-tool phrasing. What is real in grok is
> the **formatter** `build_skill_message` (`skill.rs:39-64`) plus the
> `<skill_information>` prompt-assembly path — both still used by slash expansion,
> the pager, and agent preloading. So grok's live routes are the
> `<system-reminder>` listing (with `Absolute path:`) and user-invoked assembly
> injection; there is no model-invocable skill tool. Everything else in this
> document held up on re-read. [ADR-0025](../decisions/ADR-0025-agent-skills.md)
> adopts the `<skill …>` format knowingly, as a format.

> **Recommendation and Open questions superseded (2026-07-24) by
> [ADR-0025](../decisions/ADR-0025-agent-skills.md)**, which decides the
> frontmatter contract, listing shape/budget, tool shape, and safety rules.

> **Recommendation partially superseded (2026-07-23) by [ADR-0023](../decisions/ADR-0023-fidelity-boundary-and-agents-md-loading.md).**
> The descriptions below stand. But the *Recommendation*'s per-pack fidelity table
> (item 3 — each pack reproducing its harness's skill discovery/listing/body
> injection) is overruled: skills are **loop-adjacent context machinery**, so if
> built they are a **shared, single implementation** for every pack (fidelity is
> bounded to tools + system prompt). And any listing injection is **`User`-role**
> `<system-reminder>`, **not `Developer`** (ADR-0013 amendment 2026-07-23). A
> future "Agent Skills" ADR builds on ADR-0023's boundary, not the per-pack table.

Source study of the **Agent Skills** feature across the four surveyed harnesses:
**Claude Code** (TypeScript/Bun — the primary source), **Codex**
(`codex-rs`, Rust), **Grok Build** (`xai-grok-*`, Rust — the unification model),
and **opencode** (`packages/core`, TypeScript/Effect). Conducted 2026-07-22
against the `coding-cli-survey` submodules. Citations are `harness: path:line`,
relative to each submodule root. This document feeds a possible skills ADR for
`locode-core`.

Method: one deep source read per harness covering on-disk format, discovery,
frontmatter/validation, prompt injection (listing + body), the invocation
mechanism, governance rules, and bundled resources; then cross-comparison and a
recommendation.

**Headline finding: all four harnesses have skills, and they have converged on
the *same* on-disk contract** — a `SKILL.md` file with YAML frontmatter
(`name` + `description`) in a per-skill directory, discovered from
global/project/plugin roots, advertised to the model as a **name+description
listing** (progressive disclosure), with the **full body loaded only on
invocation**. They diverge on exactly *one* axis: **how the body reaches the
model** — a dedicated `Skill` tool (Claude Code, opencode, Grok), prompt-assembly
injection / a `$mention` (Grok, Codex), or *the model reading the file itself*
with its ordinary Read tool (Codex). Grok Build even ships **vendor-compat
loaders** that read Claude's and Cursor's skill directories, which is the
strongest possible evidence that the format is now a de-facto standard.

---

## Live wire probe (2026-07-24)

The source reads above were checked against what the shipped binaries actually put
on the wire, by routing each client through a local recording reverse proxy
(`cc-reverse-proxy`, Anthropic-Messages-aware, `--simplify-tool-schema=false`) to
OpenRouter and reading the captured request payloads. Three findings, two of which
contradict this document as originally written:

- **Grok Build 0.2.111 sends 26 tools and none of them is a skill tool** —
  `ask_user_question`, `enter_plan_mode`, `exit_plan_mode`,
  `get_command_or_subagent_output`, `grep`, `image_edit`, `image_gen`,
  `image_to_video`, `kill_command_or_subagent`, `list_dir`, `monitor`, `read_file`,
  `reference_to_video`, `run_terminal_command`, `scheduler_create`,
  `scheduler_delete`, `scheduler_list`, `search_replace`, `search_tool`,
  `spawn_subagent`, `todo_write`, `use_tool`, `web_fetch`, `web_search`,
  `workflow`, `write`. It *does* send the `<system-reminder>` skills listing, in
  exactly the shape described above (`- name: desc` + `  Use when:` +
  `  Absolute path:`), with the 400-byte per-entry truncation visible. A skill
  carrying `disable-model-invocation: true` was present on disk and correctly
  absent from the listing. This confirms the correction at the top of this file.
- **Claude Code's shipped tool set is 27 tools** and *does* include `Skill`
  (`{skill: string, args?: string}`, `skill` required) — unconditionally, in a
  session whose only user skill was one directory. Its listing is `- name:
  description` only: **no `Use when:` line and no `Absolute path:` line** (those two
  are grok-isms), and it is concatenated into the *same* user message as the
  agent-types listing and the date context. Note also that the live set omits
  `Glob`, `Grep`, `TodoWrite` and `Task` (embedded search + task-v2 branches), and
  the live `Skill` description is a plain-prose rewrite of the "BLOCKING
  REQUIREMENT" text in the snapshot.
- **Two snapshot-vs-shipped drifts affecting our packs.** grok's shell tool is
  `run_terminal_command` on the wire but `run_terminal_cmd` in the published source
  (`ToolId::new("run_terminal_cmd")`), which is the name our grok pack ports; and
  live grok ships a standalone `write` tool that our grok pack does not have.
  Recorded as a known gap in the tracker — not fixed here.

General lesson, earned three times in one session: **the published source snapshot
is not the shipped binary.** Any fidelity claim about a harness's tools should be
checked on the wire, not only in the submodule.

---

## Scope

- What a "skill" is on disk in each harness (format, layout, discovery, naming).
- Frontmatter fields, validation rules, and progressive-disclosure/size limits.
- How the skill **listing** is injected into model context (role, wrapper, budget).
- How a skill **body** is injected (the `Skill` tool vs. mention vs. model-read).
- Governance: meta-rules, precedence, disable flags, permissions.
- Bundled resources/scripts a skill can carry and how they execute.
- Cross-harness comparison, pros/cons, and a recommendation for `locode-core`.

Out of scope: MCP prompts (touched only where they masquerade as skills), plugin
marketplaces (separate subsystem), and the remote/managed "skill store" backends
(noted but not dissected).

---

## Per-harness findings

### Claude Code — the reference implementation (`Skill` tool + `<system-reminder>` listing)

**On-disk format.** A skill is a directory `skills/<name>/SKILL.md`. Only the
directory form is supported under a `skills/` root — a bare `*.md` file is
ignored (`claude-code: src/skills/loadSkillsDir.ts:424-428`). The skill's **name
is the directory name** (`loadSkillsDir.ts:452`), not the frontmatter `name`
(the frontmatter `name` becomes only a `displayName`, `loadSkillsDir.ts:238`).
Frontmatter is standard YAML fenced by `---`.

**Discovery locations**, loaded and deduplicated by resolved (symlink-canonical)
path with first-wins precedence (`loadSkillsDir.ts:638-763`):
- **Managed/policy:** `<managed>/.claude/skills` (enterprise, highest trust).
- **User/global:** `~/.claude/skills`.
- **Project:** every `.claude/skills` from cwd up to home (`getProjectDirsUpToHome`, `loadSkillsDir.ts:642`).
- **Additional dirs:** `--add-dir` paths' `.claude/skills`.
- **Legacy `commands/`** dirs (deprecated) — both `SKILL.md`-in-dir and flat `*.md` slash commands (`loadSkillsFromCommandsDir`, `:566`).
- **Plugin** and **MCP** skills merge in at listing time (`SkillTool.ts:81-94`).
- **Bundled** skills ship with the binary (`src/skills/bundledSkills.ts`).
There is also **dynamic discovery**: when the agent touches a file under a nested
`.claude/skills` dir, that skill is loaded mid-session and announced
(`discoverSkillDirsForPaths`/`addSkillDirectories`, `:861-975`); and
**conditional skills** with a `paths:` frontmatter glob activate only when a
matching file is edited (`activateConditionalSkillsForPaths`, `:997-1058`).
Gitignored skill dirs are skipped (`:892`).

**Frontmatter fields** (`parseSkillFrontmatterFields`, `loadSkillsDir.ts:185-265`):
`name` (→ display name), `description` (falls back to first markdown paragraph,
`:212-214`), `when_to_use`, `allowed-tools`, `argument-hint`, `arguments`,
`version`, `model` (+ `inherit`), `effort`, `context: fork`, `agent`,
`disable-model-invocation`, `user-invocable`, `paths`, `hooks`, `shell`. No hard
schema rejection — unknown keys are ignored and missing fields get defaults.

**The listing injection (progressive disclosure step 1).** The skill catalog is
injected as a **`<system-reminder>`-wrapped `user` message** (isMeta, hidden from
the UI), not into the static system prompt:

```
<system-reminder>
The following skills are available for use with the Skill tool:

- commit: Create well-formatted git commits …
- pdf: Extract text and tables from PDFs …
</system-reminder>
```
(`claude-code: src/utils/messages.ts:3728-3738`; built by
`getSkillListingAttachments`, `src/utils/attachments.ts:2661-2740`). Only
**name + description (+ `when_to_use`, joined with ` - `)** are sent, capped at
**250 chars/entry** (`MAX_LISTING_DESC_CHARS`, `SkillTool/prompt.ts:29`) inside a
**1%-of-context-window char budget** (`SKILL_BUDGET_CONTEXT_PERCENT = 0.01`,
`prompt.ts:20-41`). Over budget, a 3-tier degrade runs: full → truncate
non-bundled descriptions → names-only (bundled skills never truncate,
`prompt.ts:70-171`). New/dynamic skills are announced incrementally via the same
attachment (a per-agent `sentSkillNames` set, `attachments.ts:2699-2730`). The
listing is only sent to agents that actually have the `Skill` tool
(`attachments.ts:2668-2673`).

**The `Skill` tool (progressive disclosure step 2).** A first-class typed tool
named `Skill` (`SkillTool/constants.ts:1`) with input
`{ skill: string, args?: string }` (`SkillTool.ts:291-298`). Its own prompt tells
the model this is a **BLOCKING REQUIREMENT** — when a skill matches, invoke it
*before* any other response, and never "mention a skill without calling this
tool" (`SkillTool/prompt.ts:173-195`). On call it runs the skill through the
slash-command machinery (`processPromptSlashCommand`) to expand `$ARGUMENTS`,
`${CLAUDE_SKILL_DIR}`, `${CLAUDE_SESSION_ID}`, and inline `!`…`` bash, then returns
the expanded body as **`newMessages`** injected into the current turn — the tool
loads instructions *inline*, it does not itself do the task
(`SkillTool.ts:634-774`; body assembled in `getPromptForCommand`,
`loadSkillsDir.ts:344-399`). The `tool_result` for the tool_use is just
`"Launching skill: <name>"` (`SkillTool.ts:856-861`) — the real content arrives
as the injected user message. Two execution modes: **inline** (default, into the
main conversation) and **forked** (`context: fork` → runs in an isolated
sub-agent with its own token budget via `runAgent`, `SkillTool.ts:122-289`). A
skill may also carry `model`/`effort`/`allowedTools` overrides applied through a
`contextModifier` (`SkillTool.ts:775-838`).

**Governance.** `disable-model-invocation: true` → tool refuses it (only the user
can `/name` it, `SkillTool.ts:412-418`). `user-invocable: false` hides it from
the slash menu. Skills go through the **permission system** keyed by skill name
with `name` / `name:*` allow/deny rules, but a skill using **only "safe
properties"** auto-allows (`SAFE_SKILL_PROPERTIES`, `SkillTool.ts:875-933`) — any
new/unknown property forces an approval prompt (fail-safe default). Invoked
skills are recorded (`addInvokedSkill`) and **re-injected after compaction** as a
`<system-reminder>` "The following skills were invoked in this session. Continue
to follow these guidelines…" (`messages.ts:3644-3662`), so guidance survives
context compression. There is also an experimental **remote skill search**
(`DiscoverSkills` + `_canonical_<slug>` names loaded from GCS/AKI,
`SkillTool.ts:969-1108`) — ant-only, injected as a plain user message.

**Bundled resources.** Because the body carries a `Base directory for this skill:
<dir>` header and `${CLAUDE_SKILL_DIR}` expands to that dir
(`loadSkillsDir.ts:344-363`), a skill can ship `scripts/`, `references/`, schemas,
templates alongside `SKILL.md`; the model reads/runs them with ordinary
Read/Bash. Inline `!`cmd`` in the body executes at expansion time under the
skill's `allowed-tools` (`loadSkillsDir.ts:374-396`) — **disabled for MCP skills**
(untrusted remote source, `:371-374`).

### Codex — no `Skill` tool; context injection + model-reads-the-file

Codex is the important contrast: **there is no `Skill` tool handler at all**
(confirmed: nothing in `codex-rs/core/src/tools/handlers/` matches skill, and
`tools/spec.rs` registers none). Skills are a **context-injection + progressive
self-read** design, implemented in a dedicated crate `codex-core-skills`
(loader, model, render, injection, service, system, root_loader — ~15 modules).

**On-disk format & metadata.** Rich `SkillMetadata`
(`codex-rs/core-skills/src/model.rs:14-26`): `name`, `description`,
`short_description`, `interface` (display name, icons, brand color, default
prompt — for a GUI surface), `dependencies` (declared tool deps, MCP transports),
`policy` (`allow_implicit_invocation`, product gating), `path_to_skills_md`,
`scope`, `plugin_id`. **Scope** is a first-class enum `User | Repo | System |
Admin` (`SkillScope`, used throughout). Skills load from filesystem roots, from an
**execution environment's filesystem** (`environment resource`), from an
**orchestrator** (`orchestrator resource`, opaque, non-filesystem), or a **custom
resource** — the loader abstracts the filesystem behind an `ExecutorFileSystem`
so a skill can live somewhere other than local disk
(`model.rs:126-159`, `HostSkillsSnapshot::read_skill_text`).

**The listing injection.** Rendered as a **`developer`-role** contextual fragment
wrapped in `<skills_instructions>…</skills_instructions>`
(`context/available_skills_instructions.rs:47-63`; tags at
`protocol/src/protocol.rs:108`). Body = an intro line + one line per skill
(**name + description + source locator/short path**) + optionally a big **"How to
use skills"** block. Budget: **2% of the context window**, default 8 000 chars,
per-description cap 1 024, with truncation warnings
(`render.rs:17-27`). Two variants: absolute-path mode vs. **alias mode** with a
`### Skill roots` table the model expands (`render.rs`;
`SKILLS_HOW_TO_USE_WITH_ALIASES`).

**Invocation = the model reads `SKILL.md` itself (progressive disclosure).** The
"How to use skills" text is explicit and is the clearest statement of the pattern
in any harness (`render.rs:30-46`, `SKILLS_HOW_TO_USE_WITH_ABSOLUTE_PATHS`):
> "Trigger rules: If the user names a skill (with `$SkillName` or plain text) OR
> the task clearly matches a skill's description … you must use that skill …
> How to use a skill (progressive disclosure): 1) After deciding to use a skill,
> the main agent must read its `SKILL.md` completely before taking task actions
> … 3) If `SKILL.md` points to extra folders such as `references/`, use its
> routing instructions … The main agent must read each required instruction or
> reference file itself before acting … Do not delegate reading … to a subagent."

So the default path has **no dedicated tool and no auto-injection**: the model
opens the path with its normal Read tool. For `orchestrator` skills only, it
calls `skills.list` / `skills.read` (resource tools) rather than a filesystem
read.

**Explicit mention injection.** When the user writes `$skill-name` (or a linked
`[$name](path)` mention, or a structured `UserInput::Skill`), core resolves it
and **injects the body eagerly** as a **`user`-role** `<skill><name>…<path>…{body}
</skill>` fragment (`injection.rs:71-124` + `collect_explicit_skill_mentions`
`:157-214`; wrapper in `skill_instructions.rs:22-41`). Plain-name mentions only
resolve when unambiguous (`select_skills_from_mentions:404-438`). Implicit
invocations (the model reading a skill because the task matched) are detected and
telemetered separately (`skills.rs:50-124`, `InvocationType::Implicit`).

**Governance.** `policy.allow_implicit_invocation` (default true) gates automatic
use; `disabled_paths` disables a skill (still counted); product gating filters
skills to a Codex "product" (`filter_skill_load_outcome_for_product`,
`model.rs:196-241`). There is a separate `skill_approval` test suite — skills that
run scripts route through the normal exec-approval path.

### Grok Build — the unifier: `Skill` tool **and** slash **and** prompt-assembly injection, plus vendor compat

Grok has the most complete and the most *unified* skills system, and is the model
the repo asks us to study for unification. Everything routes through one
canonical formatter (`build_skill_message`), and Grok reads **its own, Claude's,
and Cursor's** skill directories.

**On-disk format & discovery** (docs: `xai-grok-pager/docs/user-guide/08-skills.md`).
`SKILL.md` in a per-skill directory, YAML frontmatter, name = frontmatter `name`
(normalized: spaces/underscores → hyphens, lowercase, ≤64 chars) or the directory
name. **Discovery roots in priority order** (highest first): `./.grok/skills` &
`./.grok/commands` (cwd) → `<repo>/.grok/skills` → `~/.grok/skills` → **`~/.claude/skills`
& `./.claude/skills` (Claude compat)** → **`~/.cursor/skills` & `./.cursor/skills`
(Cursor compat)**. Also scans `.agents/skills` at each tier and walks every dir
cwd→repo-root. Dedup by name, higher priority wins (`08-skills.md:15-35`). Config
`[skills] paths/ignore/disabled` adds/excludes/deactivates
(`08-skills.md:37-48`). Vendor scanning is toggled per-vendor via
`[compat.claude]/[compat.cursor]` or env; Grok filters out known vendor default
skills. Discovery does **not** use `.gitignore`.

**Frontmatter fields** (`SkillInfo`,
`xai-grok-tools/.../implementations/skills/skill.rs` tests enumerate them;
docs `08-skills.md:90-114`): core `name`, `description`; optional `when-to-use`,
`allowed-tools`, `argument-hint`, `user-invocable`, `disable-model-invocation`,
`model`, `effort`, `license`, `compatibility`, `metadata` (arbitrary KV, with
`metadata.author`/`metadata.short-description` promoted), plus internal `scope`,
`plugin_name`, `paths`, `enabled`. Kebab-case multi-word keys — a deliberate
superset of Claude's fields (compat) plus Grok extras.

**The listing injection.** A **`<system-reminder>`** with header
`"The following skills are available for use:"` and per-entry `- name:
description\n  Use when: <triggers>\n  Absolute path: <path>`
(`skill_discovery_tracker/listing.rs:44-96`). Budget is generous — **50% of the
context window** default (`SKILL_BUDGET_CONTEXT_PERCENT = 0.5`, `listing.rs:12-16`),
per-entry combined cap 400 bytes — with the same 3-tier degrade (full → shortened
→ names-only + "… and N more skills in <dir>" overflow, `listing.rs:224-401`).
Grok **auto-extracts trigger phrases** from the description (splits on "Use when",
"Triggers on", "MUST invoke when", … `extract_trigger_suffix:431-453`) into a
separate `Use when:` line even when there is no explicit `when-to-use`. There is
**also a vendor-compat XML renderer** (`<agent_skill fullPath=…>desc</agent_skill>`,
Verbatim vs Budgeted modes, `format_announcement_xml:575-607`) — this is Grok
projecting *its* catalog into the shape another harness/model expects. A
post-compaction listing is re-emitted byte-for-byte
(`format_compaction_skill_listing:643-653`).

**Invocation — three routes, one formatter.** The canonical
`build_skill_message` wraps the body as
`<skill name="…" description="…" path="…">\n{body}\n</skill>` and is used by
**every** path: the `Skill` tool (model-invoked), TUI slash-command expansion,
the pager, and agent-definition preloading (`skill.rs:39-64`). Routes:
1. **`Skill` tool** — `{ skill: string, args?: string }` (`skill.rs:9-19`), model
   invokes it; identical shape to Claude's.
2. **Slash command** `/name args` — user-invoked; every skill is a slash command
   (`08-skills.md:147-164`).
3. **Zero-round-trip prompt-assembly injection** — when a skill is user-invoked,
   the body is expanded at prompt-assembly time and appended after `<user_query>`
   inside a `<skill_information>` envelope containing a `<skills_referenced>`
   index + one `<skill name args>{body}</skill>` block each
   (`build_skill_information:101-136`, `build_skill_block:77-83`) — no extra model
   round-trip to fetch the body.
Argument substitution supports `$ARGUMENTS`, `$ARGUMENTS[N]`, `$N`,
`${SKILL_DIR}`/`${CLAUDE_SKILL_DIR}`, `${SESSION_ID}`/`${CLAUDE_SESSION_ID}`,
`${GROK_PLUGIN_ROOT}`/`${CLAUDE_PLUGIN_ROOT}` (Claude aliases for compat), with an
`**ARGUMENTS:**` fallback suffix (`apply_substitutions:264-360`). Relative
markdown links in the body are resolved to absolute paths, **with path-traversal
protection** (a `../../secret.md` link outside `skill_dir` is left untouched,
`resolve_skill_internal_links:364-453` + test `:1269-1278`).

**Governance.** `user-invocable`/`disable-model-invocation` split (docs
`:108-109`) matches Claude exactly. Qualified names on collision (`local:commit`,
`user:commit`, `plugin:name`, `format_skill_name:141-148`). Bundled skills extract
to `~/.grok/skills` on startup and are overridable. `grok inspect [--json]`
enumerates every discovered skill and source.

### opencode — a lean `Skill` tool, three source types, permission-gated

opencode (v2 core, Effect/TypeScript) has a compact but complete implementation.

**On-disk format & sources.** `SkillV2` loads from three **source types**
(`packages/schema/src/skill.ts:7-33`): `directory` (a folder of
`{*.md, **/SKILL.md}` — glob at `packages/core/src/skill.ts:79`), `url` (pulled
via `SkillDiscovery`), and `embedded` (an in-memory `Info`). Name = frontmatter
`name`, else the `*.md` filename stem for top-level files
(`skill.ts:88-94`). Frontmatter schema is tiny and explicit:
`{ name?, description?, slash? }` (`skill.ts:33-38`). `content` = the markdown
body after frontmatter. Loading is cached per source
(`skill.ts:109-119`).

**The listing injection.** A **`SystemContext`** fragment from `SkillGuidance`
(`packages/core/src/skill/guidance.ts:16-32`): a fixed preamble
("Skills provide specialized instructions … Use the skill tool to load a skill
when a task matches its description.") followed by
`<available_skills><skill><name>…</name><description>…</description></skill>…
</available_skills>` — **name + description only**, sorted, skills with no
description dropped. It is a *live/updating* context source: on change it emits
"The available skills have changed. This list supersedes the previous list…",
and on removal "Skill guidance is no longer available. Do not use any previously
listed skill." (`guidance.ts:62-68`). Permission-denied skills are filtered out
(`SkillV2.available` + `guidance.ts:49-51`).

**The `Skill` tool.** Named `skill`, input `{ name: string }` only — **no args**
(`packages/core/src/tool/skill.ts:14-19`). `execute` finds the skill, asserts a
**permission** (action `skill`, resource = skill name, saveable,
`skill.ts:76-83`), then returns the body wrapped as
`<skill_content name="…"># Skill: name\n{content}\nBase directory for this skill:
<dir>\nRelative paths … are relative to this base directory.\n<skill_files>
<file>…</file>…</skill_files></skill_content>` — and it **samples up to 10 sibling
files** (`FILE_LIMIT = 10`, `toModelOutput:35-52`, glob `:87`) so the model
sees the bundled `scripts/`/`reference/` without a separate list step. The tool's
description states plainly that it "inject[s] the skill's instructions and
resources into the current conversation" (`skill.ts:27-33`). Skill discovery in
the tool always re-reads the live list (`skills.list()`, `:72`).

**Governance.** Fully **permission-system-gated** (`PermissionV2`): a skill can be
`deny`ed per-name or `*`; `slash: true` frontmatter marks a skill exposed as a
slash command. No forked-subagent mode, no model/effort overrides, no arguments —
the leanest of the four.

---

## Comparison

| Axis | Claude Code | Codex | Grok Build | opencode |
|---|---|---|---|---|
| Has skills? | **Yes** (primary) | **Yes** | **Yes** (most complete) | **Yes** (leanest) |
| On-disk unit | `skills/<name>/SKILL.md` | `SKILL.md` (+ resource-backed) | `<root>/skills/<name>/SKILL.md` | `dir`/`url`/`embedded` → `SKILL.md`/`*.md` |
| Name source | **dir name** (fm `name` = display) | fm `name` | fm `name`→hyphenated, else dir | fm `name`, else filename stem |
| Global root | `~/.claude/skills` | user scope | `~/.grok/skills` (+`~/.claude`,`~/.cursor`) | source-configured |
| Project root | `.claude/skills` (cwd→home) | repo scope | `.grok/skills` (cwd→repo) | directory source |
| Plugin/MCP/remote | plugin, MCP, remote(GCS) | plugin, orchestrator, env, custom | plugin, server store | url source |
| Vendor compat | — | — | **reads Claude + Cursor dirs** | — |
| Listing role/wrapper | `user` in `<system-reminder>` | **`developer`** in `<skills_instructions>` | `user` in `<system-reminder>` | `SystemContext` `<available_skills>` |
| Listing content | name + desc(+when_to_use) | name + desc + path/locator | name + desc + Use-when + abs path | name + desc |
| Listing budget | **1%** ctx, 250/entry | **2%** ctx, 1024/entry | **50%** ctx, 400/entry | (unbudgeted) |
| Body → model via | **`Skill` tool** (inline/fork) | **model reads file** / `$mention` inject | **`Skill` tool + slash + assembly inject** | **`skill` tool** |
| Body wrapper | injected user message (expanded) | `<skill><name><path>{body}</skill>` | `<skill name desc path>{body}</skill>` | `<skill_content name>{body}<skill_files>` |
| Tool args | `{skill, args?}` | (no tool) | `{skill, args?}` | `{name}` only |
| Arg substitution | `$ARGUMENTS`,`${CLAUDE_SKILL_DIR}`,… | — (no expansion) | `$ARGUMENTS[N]`,`$N`,`${SKILL_DIR}`,aliases | — |
| Bundled files | via `${CLAUDE_SKILL_DIR}` + base-dir header | via referenced paths (model reads) | via `${SKILL_DIR}` + link resolve | **sampled (10) `<skill_files>`** |
| Disable/hide flags | `disable-model-invocation`,`user-invocable` | `policy.allow_implicit_invocation`,`disabled` | same as Claude | permission deny, `slash` |
| Permission-gated | yes (name/name:* rules, safe-props auto-allow) | via exec-approval on scripts | yes (allowed-tools, plugin trust) | **yes (per-skill assert)** |
| Survives compaction | **re-inject invoked skills** | (fragment re-render) | **re-emit listing** | live SystemContext update |
| Meta-rule to model | "BLOCKING: invoke before responding" | "How to use skills" progressive-disclosure block | canonical `<skill>` framing | "load when task matches description" |

**The three invocation archetypes:**
1. **Tool-loads-inline** (Claude, opencode, Grok): a `Skill`/`skill` tool call
   pulls the body into the turn as a message/tool-result. Deterministic, one
   round-trip, the transcript records exactly which skill fired.
2. **Model-reads-the-file** (Codex default): the listing gives paths; the model
   opens `SKILL.md` with its ordinary Read tool. Zero new machinery, but relies
   on the model choosing to read and re-read.
3. **Prompt-assembly / mention injection** (Grok user-invoke, Codex `$mention`):
   for *user*-initiated invokes the body is spliced into the user message at
   assembly time — no model round-trip at all.
Grok is notable for supporting **all three** behind one formatter; that is the
unification lesson.

---

## Pros / cons & best practice

**Progressive disclosure is unanimous and load-bearing.** Every harness injects
only **name + description** up-front and defers the (potentially large) body until
a skill is actually chosen. Rationale, stated in the code: the listing is
"for discovery only — the Skill tool loads full content on invoke, so verbose
`whenToUse` strings waste turn-1 cache_creation tokens without improving match
rate" (`claude-code: SkillTool/prompt.ts:26-29`). **Best practice: never inline
skill bodies into the system prompt.** Advertise cheaply, load lazily.

**Budget the listing, then degrade gracefully.** Claude 1% / Codex 2% / Grok 50%
of the context window, all with the identical 3-tier fallback (full → truncated
descriptions → names-only + overflow marker). The number varies with how central
skills are to the product; the *mechanism* (hard char budget + graceful
truncation, bundled/native skills protected from truncation) is the transferable
part.

**The description is the router — treat it as such.** Automatic invocation keys
entirely off `description` (+ `when-to-use`). Every harness's docs push "write a
specific description, name the trigger phrases" (`grok 08-skills.md:96-97,210-216`).
Grok even parses trigger phrases out of prose. For our *own* pack, a structured
`when_to_use` beats stuffing triggers into the description.

**Skill bodies are semi-trusted content — wrap and fence them.** Skills can come
from a repo you cloned, a plugin, a vendor dir, or a remote store. Two concrete
safety patterns worth copying:
- **Clear identity boundary.** Every harness wraps the body in a named envelope
  (`<skill …>`, `<skill_content>`, `<skills_instructions>`) so the model treats
  it as *instructions to follow*, not as the user speaking, and knows exactly
  where skill text starts/ends (Grok's comment: the tags "give the model a clear
  identity and boundary", `skill.rs:46-55`).
- **Suppress side effects for untrusted sources.** Claude **refuses to execute
  inline `!`bash`` from MCP (remote) skills** (`loadSkillsDir.ts:371-374`); Grok
  **blocks path traversal** in body link resolution (`skill.rs:1269-1278`); Codex
  routes script execution through the normal exec-approval path. A skill from
  disk should not be a silent RCE.

**Tool-loads-inline vs. model-reads-the-file.** The tool approach (Claude/opencode/
Grok) is more *legible* — the `tool_use`/`tool_result` pair is a durable record
of exactly which skill fired, it plugs into the permission system, and it forces
the "invoke before responding" discipline. The model-read approach (Codex) needs
zero new tool surface and composes with resource-backed (non-filesystem) skills,
but leans on the model's discipline to read and re-read the whole file. **For a
tool-registry-centric core, the tool approach is the better fit** — it reuses the
one dispatch door we already have.

**Governance minimum set** that all four share and we should mirror: a
model-can-invoke flag (`disable-model-invocation` / `allow_implicit_invocation`),
a user-invocable/slash flag, name-collision qualification (`scope:name`), and
per-skill permission gating. opencode's per-skill permission assert is the
cleanest single hook.

**Cons / cautions observed:** listing budget pressure is real once you have
dozens of skills (hence remote skill-search in Claude and 50% budget in Grok);
name-vs-directory-vs-frontmatter naming is inconsistent across harnesses and is a
foot-gun (Claude ignores frontmatter `name` for identity, others don't); and
dynamic/conditional discovery (Claude) adds real complexity for a modest UX gain.

---

## Recommendation for `locode-core`

**1. Yes — the headless core should support skills, because they are a
tool-registry + conversation-protocol feature, not a UI feature.** A skill is
"data that becomes either a listing in context or a body injected on invoke."
Both map cleanly onto seams we already have (ADR-0013 conversation protocol,
ADR-0012 harness packs, the typed tool registry). Nothing about skills requires a
TUI or an interactive prompt, so the headless boundary holds.

**2. Injection maps onto our conversation protocol as two message kinds:**
- **Listing** → a system-reminder-style meta message (or a system surface) built
  once per turn from the discovered skill set: name + description(+ when_to_use),
  under a **char budget with 3-tier degrade**. This is the Claude/Grok/opencode
  consensus; pick the `user`+`<system-reminder>` shape unless our protocol has a
  distinct developer surface (Codex uses `developer`).
- **Body** → injected as the result of a first-class **`Skill` tool** in the
  registry, `{ skill: string, args?: string }`, returning the expanded body as a
  turn message wrapped in a named `<skill …>` envelope. This reuses the single
  dispatch door and the `tool_use`↔`tool_result` pairing invariant (one tool_use,
  one tool_result — the tool_result is a short "launched" ack, the body is a
  paired injected message, exactly as Claude does).

**3. Ported-pack faithfulness vs. `locode`-pack best-of** (per ADR-0012):
- The **grok / claude / opencode packs** must reproduce their real behavior: the
  Claude pack advertises via `<system-reminder>` and a `Skill` tool with
  `disable-model-invocation`/fork semantics; the Grok pack uses its
  `<skill name desc path>` envelope, 50% budget, `Use when:` extraction, and reads
  `~/.claude`/`~/.cursor` dirs; the opencode pack uses the `skill` tool with
  `{name}` only and the `<skill_content>`+sampled-files wrapper. A **codex pack**,
  if built, must *not* add a Skill tool — it injects a `<skills_instructions>`
  developer listing and lets the model read the file (the honest A/B).
- The **`locode` pack** gets our best-of: **`SKILL.md`** file, dir-per-skill,
  frontmatter `{name, description, when_to_use?, allowed-tools?, disable-model-invocation?,
  user-invocable?}` (a small, validated schema — reject unknown keys loudly,
  unlike the harnesses' silent-ignore), discovery from `~/.locode/skills` +
  `.locode/skills` (cwd→repo), a `Skill` tool that loads inline, a budgeted
  system-reminder listing, and the two safety rules baked in from day one:
  **envelope the body** and **no silent side effects from skill text** (no inline
  bash execution from a skill body in v1; scripts run only through the normal
  approved Bash tool with `${SKILL_DIR}` expansion). Skip in v1: forked-subagent
  skills, dynamic/conditional discovery, remote skill stores, model/effort
  overrides — all are later-phase.

**4. Propose an ADR.** This is a load-bearing, cross-cutting decision (new tool in
the registry, new context-injection surface, a new on-disk contract, pack-fidelity
rules) — it warrants a new **ADR, "Agent Skills"**. It should record: the
`SKILL.md`/frontmatter contract; progressive disclosure (listing vs. body) as an
invariant; the `Skill` tool shape and the tool_use/tool_result pairing; the
budget+degrade rule; the two safety rules; and the per-pack fidelity table above
(what each ported pack must reproduce vs. what the `locode` pack chooses). It
interacts with ADR-0012 (packs), ADR-0013 (protocol/system surfaces), ADR-0017
(approval seam — skill permission gating), and the tool-schema-description policy.

---

## Open questions

1. **Listing surface & role.** Does our protocol expose a distinct `developer`
   surface (Codex) or should the listing ride the `user`+`<system-reminder>`
   channel (Claude/Grok)? This depends on the ADR-0013 system-surface model and
   affects prompt-cache stability.
2. **Budget number.** 1% (Claude), 2% (Codex), or 50% (Grok)? Grok's 50% signals
   "skills are central"; for a general core, a conservative default (≈1–2%) plus a
   config knob seems right — decide in the ADR.
3. **Name identity.** Adopt Claude's "directory name wins, frontmatter `name` is
   display-only", or Grok/opencode's "frontmatter `name`, hyphen-normalized"? The
   latter is less surprising; the former is what the reference pack must mimic.
4. **`allowed-tools` semantics.** Do we honor a skill's `allowed-tools` as a
   per-skill permission widening (Claude's `contextModifier`), or ignore it in v1?
   Widening permissions from a disk file is a trust decision worth an explicit
   call.
5. **Do we need arg substitution in the `locode` pack** (`$ARGUMENTS`,
   `${SKILL_DIR}`), or is passing `args` as a trailing block enough for v1? The
   ported packs must implement their harness's exact substitution set regardless.
6. **Fidelity boundary (ties to the standing "harness fidelity boundary"
   question).** Skill *discovery + injection* is loop-adjacent engine behavior;
   only the *format, listing wording, tool shape, and body wrapper* are
   pack-specific. Confirm the split so we don't fork the engine per pack.
7. **Vendor compat.** Should the `locode`/grok pack read `~/.claude/skills` like
   Grok does? Useful for adoption, but couples us to Claude's dir-name identity
   rule. Defer unless there's demand.
