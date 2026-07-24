# Task 20 · Slice 1 — pack scaffold + `Bash` + minimal prompt

> Faithful port of Claude Code's `Bash` tool + a minimal-but-real system prompt,
> wiring `--harness claude` end-to-end. Follows the autonomous loop in
> [`../../docs/claude-pack-dev-process.md`](../../docs/claude-pack-dev-process.md)
> (S1). Source: reconstructed Claude Code under
> `~/dev/coding-cli-survey/submodules/claude-code`, submodule commit `6a25909`.

## Phase 0 — status analysis

- **Merged state:** grok pack (Tasks 8–13, 26) is the template; `locode-packs`
  wires exactly one pack (`GrokPack`). `PackContext` = {cwd, os, shell, date,
  headless, strip_identity}. `Harness` enum (exec/cli.rs) has one variant `Grok`;
  exec/run.rs + tui/engine.rs shape the user prompt via `grok::prompt::user_query`.
- **Minimal next unit:** stand up `crates/locode-packs/src/claude/` with `ClaudePack`
  + `Pack` impl + `Bash` (simplest tool, no shared freshness state) + a minimal
  system prompt (identity prefix + intro section), and wire `--harness claude`
  through exec/tui. Proves the pack runs end-to-end against `--api-schema mock`.
- **Why now / unblocks:** Bash has no dependency on `ClaudeSessionState` (S2+), so it
  is the cleanest first vertical slice. Establishes the module layout, the
  `descriptions/` provenance pattern, the identity-branch prompt seam, and the
  `Pack`-owns-prompt-shaping seam that S2–S7 extend.
- **Prereqs (hold):** grok template present ✓; `Host::exec` + `FrontBackSpec` seam
  ✓ (Bash reuses it); `resolve`/`Harness`/`build_registry` path ✓.
- **Risks:** (1) byte-exact transcription of the large `getSimplePrompt()` Bash
  description; mitigated by the provenance header + sha256 pin. (2) the description
  embeds a *dynamic* model name in the commit-attribution line — frozen (below).
  (3) the description references `TodoWrite`/`Agent` (not in our pool) — kept
  verbatim, logged as a gap (D8). (4) TUI prompt-shaping is currently hardcoded to
  grok; refactored to a `Pack` method.

## Phase 1 — source revisit (commit `6a25909`, fresh citations)

- **Registry / six-tool subset** — `src/tools.ts:193-251` (`getAllBaseTools`);
  `CLAUDE_CODE_SIMPLE` floor `tools.ts:287`. This slice ports `Bash` only.
- **Identity prefix** — `src/constants/system.ts:10-46` (`getCLISyspromptPrefix`):
  non-interactive → `AGENT_SDK_PREFIX` = "You are a Claude agent, built on
  Anthropic's Claude Agent SDK."; interactive → `DEFAULT_PREFIX` = "You are Claude
  Code, Anthropic's official CLI for Claude." (D6). The API layer prepends the
  prefix; the prompt array itself starts at the intro.
- **Intro section** — `getSimpleIntroSection` (`src/constants/prompts.ts`): the
  "You are an interactive agent…" line (null-output-style branch → "with software
  engineering tasks."), the `CYBER_RISK_INSTRUCTION`
  (`src/constants/cyberRiskInstruction.ts:24`), and the URL-guessing ban.
- **Bash schema** — `src/tools/BashTool/BashTool.tsx:227-260` (`z.strictObject`):
  `command` (string), `timeout` (optional number, "max 600000"), `description`
  (optional string, the long active-voice guidance). `run_in_background` +
  `dangerouslyDisableSandbox` + `_simulatedSedEdit` are **omitted** faithfully:
  `_simulatedSedEdit` CC always `.omit()`s (`:249-259`); `run_in_background` is
  `.omit()`ed when `isBackgroundTasksDisabled` (`:254-256`), which is our config.
- **Bash description** — `BashTool.tsx:431-433` (`prompt()` → `getSimplePrompt()`,
  `src/tools/BashTool/prompt.ts:275-370`). Rendered for our config:
  - `hasEmbeddedSearchTools()` → **false** (default) → the tool-preference bullets
    include Glob/Grep and `avoidCommands` lists find/grep/cat/… (`prompt.ts:279-296`).
  - `feature('MONITOR_TOOL')` → **off** → the non-Monitor sleep bullets
    (`prompt.ts:315-333`).
  - `isBackgroundTasksDisabled` (⟺ `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS`,
    `BashTool.tsx:224-226`) → **true** in our config → `getBackgroundUsageNote()`
    returns `null` (`prompt.ts:35-41`), so **no** background paragraph. (The sleep
    bullets still mention `run_in_background` unconditionally — a CC inconsistency
    we reproduce verbatim; logged as a gap.)
  - `getSimpleSandboxSection()` → `''` (sandbox off, `prompt.ts:172-175`).
  - `getCommitAndPRInstructions()` → external branch (`USER_TYPE!='ant'`),
    `shouldIncludeGitInstructions()` default **true** (`gitSettings.ts`) → the full
    inline git-commit/PR instructions (`prompt.ts:79-171`) with attribution from
    `getAttributionTexts()` (`attribution.ts:52`): commit =
    `Co-Authored-By: <model> <noreply@anthropic.com>`, pr =
    `🤖 Generated with [Claude Code](https://claude.com/claude-code)`
    (`PRODUCT_URL`, `product.ts:1`).
  - Timeouts: default 120000 (2 min), max 600000 (10 min) (`timeouts.ts:2-3`).
  - Tool-name constants: `Bash`/`Read`/`Edit`/`Write`/`Glob`/`Grep`/`TodoWrite`/`Agent`.
- **grok cross-check:** grok's `run_terminal_cmd` uses the same `Host::exec` +
  `FrontBackSpec` seam; we reuse it. grok's identity-branch precedent (`is_non_interactive`)
  maps to our `headless` branch. grok stores its short descriptions inline; ours are
  long → `descriptions/*.md` files (plan §2).

## Design

### Module layout (this slice)
```
crates/locode-packs/src/claude/
├── mod.rs            # ClaudePack + Pack impl + register(Bash) + preamble + tests
├── prompt.rs         # identity constants (both variants) + intro constant + render()
├── bash.rs           # Bash tool (Host::exec, 30k cap, timeout clamp, arg schema)
└── descriptions/
    └── bash.md       # verbatim getSimplePrompt() render (provenance-pinned)
```

### `Bash` (bash.rs)
- **Args** (`#[serde(deny_unknown_fields)]` — D5, mirrors `z.strictObject`):
  `command: String`; `timeout: Option<u64>` ("Optional timeout in milliseconds
  (max 600000)"); `description: Option<String>` (verbatim long guidance).
  Dropped: `run_in_background`, `dangerouslyDisableSandbox`, `_simulatedSedEdit`.
- **kind()** = `ToolKind::Shell`. **description()** = `include_str!("descriptions/bash.md")`.
- **run():** `Host::exec` (`bash -lc` combined output via `ShellSpec`); timeout
  default 120000, clamp to max 600000; output cap 30000 chars via `FrontBackSpec`
  (CC `maxResultSizeChars: 30_000`, `BashTool.tsx:424`). Prompt face: exit-code
  header + combined output (approximate CC's UI-coupled renderer; result *text* is
  P0, D4 — exact interactive rendering is not).
- **No guardrails port** (background-`&` / pkill): those are grok's, not CC's. CC's
  Bash has permission/sandbox machinery = our `PathPolicy` substitution (ADR-0008).

### Minimal prompt (prompt.rs) — replaced by S7
- `AGENT_SDK_PREFIX` / `DEFAULT_PREFIX` constants + `INTRO` constant (identity +
  intro, verbatim). `render(ctx)` = `<prefix by ctx.headless>\n<intro>`; honors
  `strip_identity` (drops the prefix, both variants). S7 adds the remaining D7
  sections + env + the currentDate User reminder + PackContext growth.
- **preamble()** = `[System(render(ctx))]` (single System message for S1). CC sends
  the raw user prompt (no `<user_query>` wrap) — handled by the new `Pack` method.

### `Pack::shape_user_prompt` (pack.rs) — new default method
Packs own their user-prompt shaping. Default returns the text unchanged; `GrokPack`
overrides to wrap in `<user_query>` (`grok::prompt::user_query`). Replaces the
hardcoded `grok::prompt::user_query` in exec/run.rs + tui/engine.rs with
`pack.shape_user_prompt(text)`. ClaudePack uses the default (plain). Autonomous
(pack-framework/`Pack` additions, per the autonomy contract).

### Wiring
- `lib.rs`: `pub mod claude; pub use claude::ClaudePack; static CLAUDE; PACKS = [GROK, CLAUDE]`.
- exec/cli.rs: `Harness::Claude` variant + `as_str()` arm `"claude"`.
- exec/run.rs + tui/engine.rs: `pack.shape_user_prompt(&prompt)`.

## Gap log (this slice — restated in the PR)
- **Bash description model name** — CC computes the commit-attribution model name
  per-run from the active model (`attribution.ts`); `Tool::description()` returns
  `&str` (constructed without `PackContext`), so it is **frozen** to CC's own
  external fallback literal `Claude Opus 4.6`. Batched question: parameterize the
  description on `PackContext.model` later? Reversible default = frozen.
- **`run_in_background` mentions in sleep bullets** — CC references it unconditionally
  in the sleep guidance even when background is disabled; kept verbatim (D8).
- **`TodoWrite`/`Agent` references in git instructions** — unconditional CC mentions
  of tools not in our pool; kept verbatim, logged (D8).
- **Bash guardrails / persistent shell** — CC's permission+sandbox pipeline →
  `PathPolicy` substitution (ADR-0008); per-call `bash -lc` (no persistent shell) —
  same simplification as the grok pack.

## Test matrix
- **Schema golden:** `Bash` spec — name `Bash`, `additionalProperties:false`,
  `command`/`timeout`/`description` present with verbatim field descriptions, and
  `run_in_background`/`dangerouslyDisableSandbox`/`_simulatedSedEdit` **absent**.
- **Description provenance pin:** `descriptions/bash.md` byte-length + opening line +
  sha256 (grok `template_copy_is_pinned` pattern).
- **Prompt:** headless render starts with `AGENT_SDK_PREFIX`; interactive starts with
  `DEFAULT_PREFIX`; `strip_identity` removes both; render contains the cyber-risk +
  URL-ban lines; render does **not** mention Glob/Grep/Read as "tools you have" yet
  (minimal prompt — only identity+intro).
- **Behavior (`build_registry`+`dispatch`, tempdir host):** echo → exit 0 + output;
  non-zero exit is soft-ok (ADR-0004); `timeout` honored + clamped; 30k output
  truncated with a marker; `deny_unknown_fields` rejects an unknown arg.
- **Pack:** `resolve("claude")` works; `available()` lists grok + claude; specs list
  exactly `["Bash"]` this slice; `shape_user_prompt` is identity for claude,
  `<user_query>`-wrapping for grok.

## Preset targets (binary, testable)
- `cargo run -p locode-exec -- --harness claude --api-schema mock "hi"` produces a
  Report with `harness: "claude"` and a valid transcript (no panic; single JSON on
  stdout).
- The four-part gate (`fmt · clippy · test · doc`) is green.

## Result (2026-07-24)

Shipped. `crates/locode-packs/src/claude/` scaffolded with `ClaudePack` + `Bash` +
a minimal system prompt (identity + intro); `--harness claude` runs end-to-end
(`locode -p --harness claude --api-schema mock "…"` → a valid Report with
`harness: "claude"`). Four-part gate green; 16 claude-pack tests + the full
workspace suite pass.

**Deviations / decisions from the plan (all in-scope, autonomous):**
- **`Pack::shape_user_prompt`** added (default = verbatim; `GrokPack` overrides to
  `<user_query>`). Replaces the hardcoded `grok::prompt::user_query` in exec/run.rs
  + tui/engine.rs — the pack now owns its user-prompt shaping, so codex/opencode
  won't touch the exec/tui layers. A pack-framework addition (autonomy contract).
- **Bash result rendering** — Phase 1 found CC runs a **merged fd** (stderr in
  stdout, `BashTool.tsx:692,717`) and `mapToolResultToToolResultBlockParam`
  (`:555-624`): strip leading whitespace-only lines + `trimEnd`, append `Exit code
  N` on non-zero exit, `is_error` only when interrupted. Implemented against the
  host's `FrontBackSpec` merged capture (aligns with plan §4.1). The engine
  dispatch-door belt (`MODEL_OUTPUT_BUDGET` = 30k, ADR-0008) truncates on top and
  supplies the visible marker — a documented two-layer interaction at the shared
  30k budget.
- **Bash `description` = `getSimplePrompt()`** rendered verbatim for our config
  (`descriptions/bash.md`, 9649 bytes, sha256 `236d397f…`, pinned). Documented
  gaps (D8): the commit-attribution model name is frozen to CC's external fallback
  `Claude Opus 4.6` (`description()` is `&str`, built without `PackContext`); the
  sleep bullets + git section mention `run_in_background`/`TodoWrite`/`Agent`
  unconditionally in CC — kept verbatim.

**Batched open questions (non-blocking; reversible defaults taken):**
1. Should the Bash-description commit-attribution model name become dynamic
   (parameterize `description()` on `PackContext.model`)? Default = frozen literal.

**Not yet done (later slices):** Read/Edit/Write + `ClaudeSessionState` (S2–S4),
Glob (S5), Grep (S6), full byte-exact prompt + env + preamble (S7).
