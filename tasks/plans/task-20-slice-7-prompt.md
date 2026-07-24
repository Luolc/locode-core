# Task 20 · Slice 7 — full system prompt + env + preamble (pack complete)

> The final slice: replace S1's minimal prompt with the full byte-exact system
> prompt (all D7 static sections), add the `# Environment` block (D9) and the
> `currentDate` User reminder (D10), and grow `PackContext`. Source: submodule
> `6a25909`. Follows [`../../docs/claude-pack-dev-process.md`](../../docs/claude-pack-dev-process.md) (S7).

## Phase 0 — status analysis
- **Merged:** S1–S6 — all six tools (Bash/Read/Edit/Write + gate, Glob, Grep) + S1's
  minimal prompt (identity + intro).
- **Next unit:** the full prompt. Grows `PackContext` (`is_git_repo`, `model`,
  `os_version`); reshapes `preamble()` to `[System(prompt+env), User(reminder)]`.
- **Prereqs (hold):** `join_blocks` seam (S1) ✓; exec/tui build the provider (→ model)
  and can probe git/uname ✓.
- **Risks:** byte-exact transcription of 7 large sections; `PackContext` growth breaks
  all construction sites; env facts vs CC's product-catalog lines (D9 scope).

## Phase 1 — source revisit (`6a25909`, `constants/prompts.ts`)
- **Assembly** (`getSystemPrompt`, `:444-577`): the static array before
  `SYSTEM_PROMPT_DYNAMIC_BOUNDARY` = intro → system → doing-tasks → actions →
  using-your-tools → tone → output-efficiency; then the boundary + dynamic sections
  (excluded, D7) — except `env_info_simple` (`computeSimpleEnvInfo`), included per D9.
- **Sections** (all non-ant branches; `prependBullets` = ` - ` top, `  - ` sub):
  `getSimpleSystemSection` (`:186`, incl. the always-rendered hooks bullet),
  `getSimpleDoingTasksSection` (`:199`; mentions `AskUserQuestion` — D8 gap; `/help`
  + `report issues at …` from `MACRO.ISSUES_EXPLAINER`), `getActionsSection` (`:255`),
  `getUsingYourToolsSection` (`:269`, our six-tool branch — no TodoWrite/Task bullet),
  `getSimpleToneAndStyleSection` (`:430`), `getOutputEfficiencySection` (`:403`).
- **Env** (`computeSimpleEnvInfo`, `:651-710`): `# Environment` + "You have been
  invoked in the following environment: " + ` - Primary working directory:` +
  `  - Is a git repository:` (a sub-bullet — CC nests it) + ` - Platform:` +
  ` - Shell:` (`getShellInfoLine` zsh/bash name) + ` - OS Version:` (uname) +
  model/cutoff lines. **D9 renders the facts only** (cwd/git/platform/shell/OS/model),
  not CC's product-catalog lines (model-family, CLI-availability, fast-mode) — those
  are beyond D9's env-facts enumeration (documented).
- **Preamble** (`prependUserContext`, `utils/api.ts:449-474`): a `User`
  `<system-reminder>` with the `currentDate` entry only (D10), byte-exact (incl. the
  6-space indent + trailing newline).

## Design
- `prompt.rs`: 6 new section constants (SYSTEM/DOING_TASKS/ACTIONS/USING_YOUR_TOOLS/
  TONE/OUTPUT_EFFICIENCY) + `render_env(ctx)` + `render(ctx)` = `join_blocks([identity,
  INTRO, …sections…, env])` + `context_reminder(ctx)`. Byte-frozen snapshots
  (headless + interactive) regenerated with `UPDATE_SNAPSHOTS=1`.
- `pack.rs`: `PackContext` grows `is_git_repo: bool`, `model: Option<String>`,
  `os_version: Option<String>`.
- exec/run.rs + tui/engine.rs: build the provider first (→ model), then populate the
  new fields (`detect_git_repo` walk, `uname -s -r`).
- `mod.rs`: `preamble()` = `[System(render), User(context_reminder)]`.

## Gaps (this slice)
- Env renders D9's facts, not CC's product-catalog lines (model-family / CLI-availability
  / fast-mode) — beyond D9's enumeration; version-specific, excluded.
- `AskUserQuestion` mentioned in the doing-tasks section (tool not in our pool) — kept
  verbatim (D8).
- The model "powered by" line uses the raw model id (no CC marketing-name mapping);
  knowledge-cutoff line omitted (no cutoff mapping).
- Flattening: CC sends sections as separate wire blocks; we join with `\n\n` (§5.6).

## Test matrix
Byte-frozen snapshots (headless → Agent SDK identity; interactive → Claude Code
identity); `strip_identity` removes both; all 7 section headers present; using-your-tools
names our six pool + omits the TodoWrite/Task bullet; env renders D9 fields + skips
model/OS-version when absent; `context_reminder` byte-exact.

## Result (2026-07-24) — Task 20 COMPLETE
Shipped. The full prompt + env + preamble landed; the claude pack is complete (six
tools + byte-exact prompt). 177 pack tests + full workspace suite pass; four-part gate
green; preset runs end-to-end with the full prompt (both identity branches verified).

**Decisions/deviations (in-scope, autonomous):**
- `PackContext` grew `is_git_repo` + `model` (D9) + `os_version` (for the env OS-Version
  line — same cheap exec/tui-computed pattern; a small extension of D9's stated growth).
- exec/tui reordered to build the provider before the preamble so the env block can
  name the model.
- Env = D9 facts only (product-catalog lines excluded); model line uses the raw id.

**Task 20 is complete** — flip the tracker checkbox; ADR-0012 amended; SPEC reconciled.
