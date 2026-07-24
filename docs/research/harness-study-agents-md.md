# Harness study — AGENTS.md / CLAUDE.md project-instruction loading

> **Recommendation partially superseded (2026-07-23) by [ADR-0023](../decisions/ADR-0023-fidelity-boundary-and-agents-md-loading.md).**
> The seven-axis *descriptions* below remain the authoritative catalogue of each
> harness's behavior. But the *Recommendation* section's two load-bearing choices
> are overruled: (1) there is **no per-pack fidelity** for loading — one **shared**
> loader serves every pack (fidelity is bounded to tools + system prompt); and
> (2) injection is **`User`-role** `<system-reminder>`, **not `Developer`** (the
> `Developer` portable fallback is reverse-lossy — ADR-0013 amendment 2026-07-23).
> Read this study as the menu ADR-0023 picked the one shared best-of design from.

Source study of how the four studied harnesses discover, merge, and inject
project-level instruction files (`AGENTS.md`, `CLAUDE.md`, rules dirs, memory
files). Conducted 2026-07-22 against the `coding-cli-survey` submodules.
Citations are `harness: path:line`, relative to each submodule root. This feeds a
future locode capability (project-instruction loading in the headless core +
packs). Companion to [`tui-harness-study.md`](tui-harness-study.md).

Method: one deep read per harness of the discovery walk, the merge/precedence
rules, and — the central axis — **how the assembled text enters the model
context** (which role/message, exact wrapping tags, where in the request).

---

## Scope

Seven axes per harness:

1. **Which files** are loaded (canonical name, aliases, rules dirs, memory).
2. **Detection order & locations** — cwd only, walk-up, global/home, nested,
   `--add-dir`.
3. **Merge behavior** — override vs additive, precedence, dedup.
4. **HOW content is injected** — role/message, template/tags, request position
   (the central question).
5. **Imports/includes** — recursion depth, cycles, size limits.
6. **Refresh/caching** — reload mid-session, cache, per-turn.
7. **Enable/disable** switches.

---

## Per-harness findings

### Codex — `AGENTS.md` (Rust, `codex-rs/core`)

The most rigorously specified of the four. The module header states the contract
outright (`codex: codex-rs/core/src/agents_md.rs:1-16`): *"We include the
concatenation of all files found along the path from the project root to the
current working directory."*

**1. Files.** `AGENTS.md` is the default; `AGENTS.override.md` is a
higher-precedence local override; plus any configured
`project_doc_fallback_filenames`. The candidate list is built override-first:

```rust
// codex: agents_md.rs:234-248
fn candidate_filenames(config: &Config) -> Vec<&str> {
    names.push(LOCAL_AGENTS_MD_FILENAME);   // "AGENTS.override.md"
    names.push(DEFAULT_AGENTS_MD_FILENAME); // "AGENTS.md"
    for candidate in &config.project_doc_fallback_filenames { … }
}
```
`DEFAULT_AGENTS_MD_FILENAME = "AGENTS.md"`, `LOCAL_AGENTS_MD_FILENAME =
"AGENTS.override.md"` (`agents_md.rs:37-40`). A separate host-provided
`user_instructions` (the `~/.codex` global layer, injected by the extension host)
is prepended ahead of project docs.

**2. Detection order & locations.** Walk **upward** from cwd to the *project
root*, then collect root→cwd inclusive. Project root = nearest ancestor holding a
`project_root_markers` entry, default `.git` (`agents_md.rs:172-187`). It does
**not** walk past the project root (`agents_md.rs:16`). An empty marker list
disables parent traversal (only cwd is scanned). Ancestor metadata probes run
concurrently, bounded to 256 (`agents_md.rs:49,209-224`). Multiple "turn
environments" (Codex's `--add-dir`-equivalent multi-root) each contribute their
own hierarchical set (`agents_md.rs:59-78`).

**3. Merge.** Additive concatenation, root→cwd order (deeper = later = wins on
conflict). User/internal→project transition inserts a marker
`"\n\n--- project-doc ---\n\n"` (`agents_md.rs:44,319-345`). Byte-budgeted:
`project_doc_max_bytes`; files past the budget are truncated with a warning; a
budget of `0` disables loading entirely (`agents_md.rs:95-130`). With ≥2
environments the body self-labels each with `for \`<id>\` with root <path>`
(`agents_md.rs:347-390`).

**4. Injection — CENTRAL.** Codex injects AGENTS.md as a **`role: "user"`**
message (a `ContextualUserFragment`), *not* the system prompt:

```rust
// codex: codex-rs/core/src/context/user_instructions.rs:10-29
fn role(&self) -> &'static str { "user" }
fn type_markers() -> (&'static str, &'static str) {
    ("# AGENTS.md instructions", "</INSTRUCTIONS>")
}
fn body(&self) -> String {
    format!("{directory}\n\n<INSTRUCTIONS>\n{}\n", self.text)
}
```
So the wire sees a user turn beginning `# AGENTS.md instructions for <dir>` then
`<INSTRUCTIONS>…</INSTRUCTIONS>`. It is part of Codex's **world-state** system:
tracked across turns and re-emitted with a diff notice when it changes (see 6).

**5. Imports.** None — no `@import`. Flat file set. The only nesting is the
directory hierarchy itself.

**6. Refresh/caching.** `AgentsMdManager` caches the loaded result keyed on the
environment selection; `refresh()` reloads only when the selection changes
(`agents_md_manager.rs:31-49`). When the content changes mid-session the
world-state layer re-injects with a replacement banner:
`"These AGENTS.md instructions replace all previously provided AGENTS.md
instructions."`, and a removal banner `"The previously provided AGENTS.md
instructions no longer apply."` when it vanishes
(`context/world_state/agents_md.rs:9-11,52-79`).

**7. Enable/disable.** `project_doc_max_bytes = 0` disables; empty
`project_root_markers` disables parent traversal;
`project_doc_fallback_filenames` extends the recognized set.

---

### Grok Build — `AGENTS.md` + `CLAUDE.md` + rules dirs (Rust, `xai-grok-agent`)

The most *vendor-inclusive* loader — it deliberately ingests other harnesses'
files. Module doc: *"Searches from cwd to repo root, plus `~/.grok/`. Also
discovers `*.md` files in `.grok/rules/` and `.claude/rules/` directories."*
(`grok: crates/codegen/xai-grok-agent/src/prompt/agents_md.rs:1-5`).

**1. Files.** A compat-gated filename list (`compat.agent_filenames()`)
including `AGENTS.md`, `Claude.md`/`CLAUDE.md`, plus `.claude/CLAUDE.md` and
`.cursor/AGENTS.md` subdir forms; plus rules dirs `.grok/rules/*.md`,
`.claude/rules/*.md`, `.cursor/rules/*.md` (`agents_md.rs:38-64,147-153`,
tests `:294-321,540-603`). Each vendor surface is gated by a `CompatConfig` cell
so a pure-grok run can switch the Claude/Cursor surfaces off.

**2. Detection & locations.** `~/.grok/` (`grok_home`) is scanned **first**
(lowest priority), then `~/.claude/` and `~/.cursor/` for compat
(`agents_md.rs:88,97-105`); then the cwd→git-root chain, reversed to root→cwd so
deeper files land later (`agents_md.rs:107-143`; the `CRITICAL: Reverse` comment
at `:121`). git root via `git2::Repository::discover`; outside a repo it falls
back to cwd only (`:89-91,141-143`). An optional "workspace user dir" is spliced
in just after repo root (`:124-138`).

**3. Merge.** Additive. Dedup by **canonical path** (case-insensitive FS /
symlink-resolved tmpdirs) via a `HashSet` (`agents_md.rs:159-168`). `.gitignore`
filtering excludes ignored files (`:156`). Rules-dir files are sorted
alphabetically (`:60`) and have YAML frontmatter stripped before injection
(`:214-221`). **No byte cap** — a test asserts a 5000-char file is delivered
verbatim with no truncation (`agents_md.rs:353-371`).

**4. Injection — CENTRAL.** Rendered as a **`<system-reminder>` block appended
to a user message** (`format_agents_md_section` — comment at `:186` says
"user message injection"):

```rust
// grok: agents_md.rs:194-227
pub const LEGACY_AGENTS_MD_REMINDER_PREFIX: &str =
    "\n\n<system-reminder>\nAs you answer the user's questions, you can use the following context";
// …" (ordered from repo root to current directory - deeper files take precedence on conflicts):\n"
// per file: "\n## From: {file_path}\n{content}\n"
// footer: "Follow these instructions exactly. When working in subdirectories not
//          listed above, check for additional project instruction files …"
// "\n</system-reminder>"
```
(This is the exact reminder envelope wrapping *this* study's own context.) It is
injected idempotently via `SyntheticReason::ProjectInstructions`
(`session/acp_session_impl/prompt_build.rs:68`) and re-injected across compaction
(`session/compaction.rs:1208-1431`) — with structural detection of legacy
untagged copies on resumed sessions (the `LEGACY_…_PREFIX` const).

**5. Imports.** None (`@import`-style). Nesting is the directory walk + rules
dirs only.

**6. Refresh/caching.** Discovered per prompt build; idempotence gating avoids
duplicate injection on forks/resumes (`session/acp_session_tests/
project_instructions_idempotence_tests.rs`); re-injected after compaction.

**7. Enable/disable.** `CompatConfig` cells toggle which vendor surfaces
(`.claude`, `.cursor`) are scanned (`agents_md.rs:72-77,147-148`).

---

### Claude Code — `CLAUDE.md` memory hierarchy (TypeScript, `src/utils/claudemd.ts`)

The richest model: a 4-tier "memory" hierarchy with recursive `@import`s. Doc
comment enumerates the tiers (`claude-code: src/utils/claudemd.ts:4-16`):
Managed → User → Project → Local.

**1. Files & tiers.**
- **Managed** (policy): `<managed>/CLAUDE.md` + `<managed>/.claude/rules/*.md`
  — always loaded first (`claudemd.ts:803-823`, path `config.ts:1789-1790`).
- **User** (global): `~/.claude/CLAUDE.md` + `~/.claude/rules/*.md`
  (`claudemd.ts:825-847`, `config.ts:1783-1784`).
- **Project** (checked in): `CLAUDE.md`, `.claude/CLAUDE.md`, `.claude/rules/*.md`
  in each dir along the walk (`claudemd.ts:886-921`).
- **Local** (private): `CLAUDE.local.md` (`claudemd.ts:922-935`,
  `config.ts:1785-1786`).
- Plus **AutoMem** (`~/.claude` auto-memory) and, behind a feature flag,
  **TeamMem** (org-synced) (`config.ts:1791-1797`, `claudemd.ts:1173-1183`).

**2. Detection & locations.** User/Managed from fixed home/managed dirs. Project
& Local by walking cwd **up to filesystem root** collecting dirs, then processing
**root→cwd** (`claudemd.ts:850-878`). `--add-dir` directories are processed as
extra Project roots (`claudemd.ts:941-970`). Nested-git-worktree dedup skips
checked-in files from the main repo working tree (`claudemd.ts:859-884`).

**3. Merge.** Additive across tiers, in the fixed order Managed→User→Project→
Local→additional. Dedup via a `processedPaths` set (`claudemd.ts:796,630`).
`claudeMdExcludes` glob settings can exclude User/Project/Local files (never
Managed) (`claudemd.ts:547-565`). Cap: `MAX_MEMORY_CHARACTER_COUNT = 40000`
(`claudemd.ts:92`).

**4. Injection — CENTRAL.** Assembled by `getClaudeMds` — each file becomes
`Contents of <path> <(description)>:\n\n<content>`, all prefixed with a strong
OVERRIDE preamble (`claudemd.ts:90,1153-1194`):

> *"Codebase and user instructions are shown below … IMPORTANT: These
> instructions OVERRIDE any default behavior and you MUST follow them exactly as
> written."*

That string becomes the `claudeMd` entry of `getUserContext` (`context.ts:155-189`),
which `prependUserContext` wraps and prepends as a **`role: "user"` meta
message** inside a `<system-reminder>`:

```ts
// claude-code: src/utils/api.ts:461-473
createUserMessage({
  content: `<system-reminder>\nAs you answer the user's questions, you can use the following context:\n`
    + Object.entries(context).map(([k,v]) => `# ${k}\n${v}`).join('\n')
    + `\n\n IMPORTANT: this context may or may not be relevant … \n</system-reminder>\n`,
  isMeta: true,
})
```
So CLAUDE.md rides under a `# claudeMd` heading in a synthetic user-role
`<system-reminder>` at the head of the message list. (Again — the exact shape of
this session's own context block.) TeamMem content additionally gets its own
`<team-memory-content source="shared">` tags (`claudemd.ts:1180-1183`).

**5. Imports — the differentiator.** `@path` imports: a regex
`(?:^|\s)@((?:[^\s\\]|\\ )+)` extracts include paths from markdown tokens
(`claudemd.ts:459-486`), plus frontmatter `paths:` includes
(`:254-290`). Recursion is depth-limited to `MAX_INCLUDE_DEPTH = 5`
(`claudemd.ts:537`) and cycle-guarded by `processedPaths` (skip if already seen
or depth exceeded, `:630`). External (outside-cwd) imports require approval
(`hasClaudeMdExternalIncludesApproved`) unless it's User memory, which may always
include external (`claudemd.ts:667,798-834`). HTML comments and frontmatter are
stripped (`:292-343`).

**6. Refresh/caching.** `getUserContext`/`getMemoryFiles` are `memoize`d for the
conversation (`context.ts:155`, `claudemd.ts:790`); `setSystemPromptInjection`
clears the caches (`context.ts:29-34`). Conditional (path-matched) rules are
loaded lazily per target file via `getManagedAndUserConditionalRules`
(`claudemd.ts:1205-`).

**7. Enable/disable.** `CLAUDE_CODE_DISABLE_CLAUDE_MDS` = hard off; `--bare`
skips auto-discovery **but still honors explicit `--add-dir`** (the "skip what I
didn't ask for, not what I asked for" rule) (`context.ts:162-172`). Per-source
toggles: `userSettings`/`projectSettings`/`localSettings` enablement
(`claudemd.ts:826,887,922`).

---

### opencode — `AGENTS.md` (TypeScript, `packages/core`)

The leanest loader, and the **only one that injects into the system prompt**.

**1. Files.** `AGENTS.md` only for discovery (`targets: ["AGENTS.md"]`), plus a
single global `<globalConfig>/AGENTS.md` (`opencode:
packages/core/src/instruction-context.ts:52,58`). A config field
`instructions: string[]` ("Additional paths or URLs supplying ambient
instructions") lets users add custom paths/URLs (`packages/core/src/config.ts:96-97`).

**2. Detection & locations.** Walk **up from cwd to the project root**
(`fs.up({ targets: ["AGENTS.md"], start: cwd, stop: project.directory })`) — but
only when cwd is *inside* the project (`insideProject` guard), plus the global
config file (`instruction-context.ts:40-58`). No per-vendor aliases, no rules
dirs.

**3. Merge.** Additive; `Array.dedupe` over `[globalAGENTS, ...discovered]`
(`instruction-context.ts:58`). No byte cap. If a discovered file becomes
unreadable the whole source reports `unavailable` rather than a partial baseline
(`:71-72`).

**4. Injection — CENTRAL & DISTINCT.** opencode appends the rendered
instructions to the **`system` prompt array**, after the agent's base system
text:

```ts
// opencode: packages/core/src/session/runner/llm.ts:208-210
system: [agent.info?.system, system.baseline]
  .filter((part): part is string => part !== undefined && part.length > 0)
  .map(SystemPart.make),
```
`render` = per file `Instructions from: <path>\n<content>` joined by blank lines
(`instruction-context.ts:99-101`). This is the one harness that treats AGENTS.md
as *system*-role material rather than a user/`<system-reminder>` fragment.

**5. Imports.** None.

**6. Refresh/caching.** Modeled as a typed `SystemContext` source with
`baseline`/`update`/`removed` renderers (`system-context/index.ts:135-165`) and a
durable **context-epoch** (`session/context-epoch.ts`). On change mid-session it
emits `"These instructions replace all previously loaded ambient instructions."`;
on removal `"Previously loaded instructions no longer apply."`
(`instruction-context.ts:36-37`) — the same replace/remove-diff idea as Codex,
but landing in the system prompt.

**7. Enable/disable.** `Flag.OPENCODE_DISABLE_PROJECT_CONFIG` skips the project
walk (keeps the global file); the `insideProject` guard skips project discovery
when cwd is outside the project (`instruction-context.ts:48-49`).

---

## Comparison

| Axis | Codex | Grok Build | Claude Code | opencode |
|---|---|---|---|---|
| **Canonical file** | `AGENTS.md` | `AGENTS.md` | `CLAUDE.md` | `AGENTS.md` |
| **Aliases / extra** | `AGENTS.override.md`, configurable fallbacks | `CLAUDE.md`, `.claude/CLAUDE.md`, `.cursor/AGENTS.md`, `.grok/.claude/.cursor rules/*.md` | `.claude/CLAUDE.md`, `CLAUDE.local.md`, `.claude/rules/*.md`, AutoMem/TeamMem | `instructions[]` custom paths/URLs |
| **Walk** | cwd→project root (`.git`), root→cwd order | cwd→git root, root→cwd order | cwd→FS root, root→cwd order | cwd→project root |
| **Global/home** | `~/.codex` user layer (host) | `~/.grok/`, `~/.claude/`, `~/.cursor/` | `~/.claude/CLAUDE.md` + rules | `<globalConfig>/AGENTS.md` |
| **`--add-dir`** | multi turn-environments | workspace-user dir | additional Project roots | — |
| **Merge** | additive, `--- project-doc ---` sep | additive, canonical dedup, gitignore, frontmatter-stripped | additive 4-tier, `processedPaths` dedup, excludes | additive, `Array.dedupe` |
| **Precedence** | deeper wins (later) | deeper wins; home lowest | Managed→User→Project→Local; deeper wins | deeper wins |
| **Size cap** | `project_doc_max_bytes` (0=off) | **none** | 40 000 chars | none |
| **Injection role** | **user** msg | **user** msg (`<system-reminder>`) | **user** meta msg (`<system-reminder>`) | **system** prompt |
| **Wrapping tags** | `# AGENTS.md instructions` + `<INSTRUCTIONS>…</INSTRUCTIONS>` | `<system-reminder>` … `## From: <path>` … `</system-reminder>` | `<system-reminder>` + `# claudeMd` + OVERRIDE preamble | `Instructions from: <path>` (plain, in system) |
| **`@import`** | no | no | **yes** (depth 5, cycle-guarded, approval-gated external) | no |
| **Mid-session diff** | replace/remove banners (world-state) | idempotent re-inject; compaction re-inject | memoized per conversation | replace/remove banners (context-epoch) |
| **Disable switch** | `project_doc_max_bytes=0`, empty markers | `CompatConfig` cells | `CLAUDE_CODE_DISABLE_CLAUDE_MDS`, `--bare` | `OPENCODE_DISABLE_PROJECT_CONFIG` |

**The one structural split:** three of four inject AGENTS.md/CLAUDE.md as a
**user-role** fragment (Codex a bare user message; Grok & Claude a user-role
`<system-reminder>`), while **opencode alone puts it in the `system` prompt**.

---

## Pros/cons & best practice

**Injection role — user/`<system-reminder>` vs system.**
- *User `<system-reminder>` (Grok, Claude; Codex bare-user):* keeps the base
  system prompt fixed and provider-prompt-cache-friendly (the volatile,
  per-project content lives in the message stream, not the cached system param),
  and lets the content be **re-emitted with a diff banner** when it changes
  mid-session without invalidating the system-prompt cache. The `<system-reminder>`
  tag signals "framing, not user speech" so the model doesn't answer it as a
  question (Claude's explicit *"you should not respond to this context"* footer).
  Failure mode: user-role instructions are lower-authority than the system prompt
  and can be diluted by long histories — mitigated by strong preambles ("OVERRIDE
  … follow exactly").
- *System (opencode):* highest authority, simplest mental model. Cost: mutating
  the system prompt mid-session busts the prompt cache and forces the whole
  epoch/baseline machinery opencode built to manage it.
- **Best practice:** inject as a **user-role `<system-reminder>`** with an
  explicit authority preamble and a relevance disclaimer; reserve the true system
  prompt for the harness's own fixed instructions. This is the majority design
  and the one that composes with prompt caching + mid-session refresh.

**Walk & precedence.** Universal agreement: walk cwd→root, apply **root→cwd** so
the most-specific (deepest) file wins on conflict, and stop at the project root
(`.git`) rather than the filesystem root (Claude is the outlier that walks to FS
root — noisier, occasionally surprising). **Label each file with its source
path** (all four do) so the model can attribute conflicting rules.

**Dedup & robustness.** Canonicalize paths before dedup (Grok) — case-insensitive
FS and symlinked tmpdirs otherwise double-load. Honor `.gitignore` (Grok).
Nested-worktree dedup (Claude) is a real corner case.

**Size discipline.** A byte/char cap (Codex 0-disable, Claude 40k) prevents a
runaway `AGENTS.md` from eating the window; Grok/opencode's "no cap" trusts the
author. A cap with a truncation warning is the safer default.

**Imports.** Claude's `@import` (depth-limited, cycle-guarded, external-approval)
is powerful but is the only one to ship it — real complexity (frontmatter/HTML
stripping, approval prompts, cycle sets) for real modularity. Worth it only once
single-file instructions become unwieldy.

**Mid-session refresh.** Codex and opencode both model instructions as
**diff-able state** (replace/remove banners) so edits to AGENTS.md take effect
without a restart and without silently duplicating. This is the mature pattern;
Grok's idempotent re-inject is a lighter version; Claude memoizes per
conversation (no live reload without cache clear).

**Enable/disable.** Every harness has an off switch. Claude's `--bare` rule —
*"skip auto-discovery but still honor explicit `--add-dir`"* — is the right
nuance: disabling discovery ≠ ignoring what the user explicitly pointed at.

---

## Recommendation for locode

locode is a headless core + packs (ADR-0012), with a four-role conversation model
— **System / Developer / User / Assistant** (ADR-0013). `Developer` already *is*
the "mid-conversation client-injected context" role, rendered either as a beta
`role:"system"` message or the portable `role:"user"` `<system-reminder>`
fallback (ADR-0013:74,90-92,107). Project instructions map onto this cleanly.

**Where it lives.** A small headless loader in the core (candidate:
`locode-host` or a new `locode-instructions` module) that produces neutral
`ProjectInstructions { entries: [{ source_path, content }] }`. It routes through
the existing seams (no direct FS in tools) and emits protocol content — never
touches the wire directly.

**Injection → role.** Inject as a **`Developer`** fragment, not `System` and not
a plain `User` turn. Rationale: it is exactly the "client-injected mid-stream
context" `Developer` was created for; the portable rendering
(`role:"user"` + `<system-reminder>…</system-reminder>`) reproduces the
Grok/Claude shape byte-for-byte, and the beta-message rendering is available for
higher authority — all decided by the wire flag, not the loader. This keeps the
System prompt (pack preamble) prompt-cache-stable and lets instructions be
re-emitted on change. (Rejects opencode's system-prompt placement — it fights
prompt caching and our System/Developer split already gives us the higher-authority
option without mutating System.)

**Ported packs mimic exactly (faithfulness wins — MEMORY: harness-fidelity-boundary,
verify-fidelity-claims):**
- **codex pack:** `AGENTS.md` + `AGENTS.override.md` + fallbacks; walk to `.git`
  root; `--- project-doc ---` separator; `project_doc_max_bytes`; user-role
  message wrapped `# AGENTS.md instructions` / `<INSTRUCTIONS>…</INSTRUCTIONS>`;
  replace/remove diff banners. (`Developer` in **portable user** rendering to
  match Codex's literal `role:"user"`.)
- **grok pack:** the vendor-inclusive set (`AGENTS.md`, `CLAUDE.md`,
  `.claude/CLAUDE.md`, `.cursor/AGENTS.md`, `.grok/.claude/.cursor` rules dirs),
  `~/.grok`/`~/.claude`/`~/.cursor` homes, canonical-path dedup, gitignore filter,
  frontmatter strip, **no cap**, and the *exact* `<system-reminder>` envelope
  (`agents_md.rs:194-227`) — including the "deeper files take precedence" header
  and "Follow these instructions exactly" footer. CompatConfig-style vendor gates.
- **claude pack:** the 4-tier Managed/User/Project/Local hierarchy,
  `.claude/rules/*.md`, `@import` (depth 5, cycle-guarded), 40k cap, the OVERRIDE
  preamble + `# claudeMd`-keyed `<system-reminder>`, `--bare` /
  `CLAUDE_CODE_DISABLE_CLAUDE_MDS`, honoring `--add-dir` under `--bare`.
- **opencode pack:** `AGENTS.md` only + global file, project-inside guard, epoch
  replace/remove banners, and — faithfully — inject into the **System** prompt
  (the one pack that does), `OPENCODE_DISABLE_PROJECT_CONFIG`.

**The `locode` (best-of) pack** — distilled defaults:
- **Files:** `AGENTS.md` canonical (this repo already standardizes on it), with a
  local override (`AGENTS.override.md`, à la Codex) and optional `.locode/rules/*.md`.
- **Walk:** cwd→`.git` root, apply root→cwd (deeper wins); global
  `~/.locode/AGENTS.md`; honor `--add-dir` as extra roots. Stop at project root
  (not FS root).
- **Merge:** additive; canonical-path dedup + gitignore filter (Grok's
  robustness); label each entry with its source path; **byte cap with truncation
  warning** (Codex's discipline).
- **Injection:** `Developer` role, portable `<system-reminder>` rendering, with a
  short authority preamble and a relevance disclaimer (Claude's framing), source
  paths inline (all four), "deeper wins" note (Grok).
- **Refresh:** model as diff-able state — re-emit with a replace/remove banner
  when the files change mid-session (Codex/opencode), idempotent (Grok) so
  compaction/resume never double-injects.
- **Imports:** defer `@import` to a later slice; single-file + rules-dir first.
- **Disable:** an env switch + a `--bare`-style flag that still honors explicit
  `--add-dir`.

**ADR.** Warranted. Propose **a new ADR, "Project-instruction loading &
injection"** (next free number; ADRs run through 0022). It should: (a) define the
neutral `ProjectInstructions` shape and the headless loader seam; (b) fix the
default injection to the `Developer` role + portable `<system-reminder>` and cite
ADR-0013's mapping; (c) record the per-pack fidelity table above (ported = mimic,
`locode` = best-of); (d) set the `locode`-pack defaults (canonical `AGENTS.md`,
`.git`-root walk, cap, dedup, refresh-as-diff). Reconcile SPEC.md's crate-layout /
boundaries in the same change (ADR-first — MEMORY: adr-first-reconcile).

---

## Open questions

1. **Loader home — `locode-host` vs new `locode-instructions` crate?** The walk +
   FS reads must route through the dispatch/host seam (ADR-0008), not a tool.
   Which crate owns discovery, and does it reuse the ripgrep/glob machinery
   (ADR-0011) for rules-dir enumeration?
2. **`Developer` vs `User` default rendering.** ADR-0013 makes `Developer`
   fidelity a wire flag (beta system-message vs portable `<system-reminder>`).
   Do we standardize project instructions on the portable rendering always (max
   compat, matches Grok/Claude), or let the beta path raise authority?
3. **Refresh granularity.** Do we re-scan the filesystem every turn (cost) or
   watch/invalidate on edit? Codex keys a cache on environment selection; is a
   per-turn cheap `stat` acceptable in the headless loop?
4. **Cap default for the `locode` pack** — adopt Codex's byte budget or Claude's
   40k char cap, and what truncation marker?
5. **`@import` scope.** Ship it in the `locode` pack (Claude-style modularity) or
   keep it claude-pack-only for faithful A/B and avoid the external-approval
   complexity in core?
6. **Multi-root / `--add-dir` semantics** in the headless core vs TUI — does the
   core accept multiple roots (Codex turn-environments) or is that a
   host/TUI-level concern?
7. **Identifier confirmation (voice-input hygiene, per AGENTS.md):** this study
   read `AGENTS.md`/`CLAUDE.md` handling. No ambiguous identifiers were inferred
   from the task prompt; the four module paths cited above are the authoritative
   sources.
