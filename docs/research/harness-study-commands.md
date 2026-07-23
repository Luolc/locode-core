# Harness study — slash commands

Source study of the **slash-command systems** of the four reference harnesses —
**Claude Code** (`src/commands*`, TS/React), **Codex** (`codex-rs/tui`, Rust),
**Grok Build** (`xai-grok-pager/src/slash`, Rust), **opencode**
(`packages/{opencode,core,app}`, TS) — conducted 2026-07-22 against the
`coding-cli-survey` submodules. Citations are `harness: path:line`, relative to
each submodule root. This document feeds the locode TUI's holistic
slash-command pass; the TUI today ships only `/new` `/quit` `/exit`
(`locode-tui/src/app.rs:581-606`).

Method, per the repo's source-grounded rule: one deep read per harness of the
registry, custom/plugin command loading, the parse→register→dispatch pipeline,
the autocomplete menu, and the client-action-vs-prompt-template split; then
cross-comparison and a locode recommendation with a proposed ADR.

---

## Scope

The six questions this study answers, per harness:

1. Built-in command set + where the registry lives.
2. Custom / user commands: on-disk format, frontmatter, argument placeholders,
   discovery locations, namespacing.
3. Plugin commands: contribution, load order, precedence/conflicts.
4. Loading pipeline: parse → register → dispatch; client-side action vs
   prompt-template.
5. Suggestion / autocomplete: the `/`-menu, fuzzy match, ranking, arg hints.
6. Command → model injection: role/wrapping for prompt-template commands.

The central axis throughout is the **client-action vs prompt-template split**:
does a command *do something in the client* (`/quit`, `/model`, `/clear`) or
*expand into text sent to the model* (`/commit`, `/review`, a user's
`.md` command)?

---

## Per-harness findings

### Claude Code — richest custom/plugin system, typed command union

**Built-in set + registry.** ~120 commands live in `src/commands/` and are
registered in `src/commands.ts` (`getCommands()`). The full catalog is
documented in `claude-code: docs/commands.md:37-204` (git: `/commit`
`/commit-push-pr` `/branch` `/diff` `/rewind`; quality: `/review`
`/security-review` `/bughunter`; session: `/compact` `/context` `/resume`
`/clear` `/export`; config: `/config` `/permissions` `/model` `/theme` `/vim`
`/output-style`; memory: `/memory` `/add-dir`; MCP/plugins: `/mcp` `/plugin`
`/reload-plugins` `/skills`; auth: `/login` `/logout`; agents: `/agents`
`/plan` `/ultraplan`; diagnostics: `/doctor` `/status` `/cost` `/usage`;
misc: `/help` `/init` `/rename`, …).

**Three typed command kinds** — the split is explicit in the type
(`claude-code: src/types/command.ts:16-206`, `docs/commands.md:13-17`):

- `PromptCommand` (`type: 'prompt'`) — `getPromptForCommand(args, ctx)` returns
  `ContentBlockParam[]` injected into the conversation. Carries
  `allowedTools`, `model`, `argNames`, `progressMessage`, and a
  `context: 'inline' | 'fork'` field: `inline` expands into the current
  conversation, `fork` runs as a sub-agent with its own token budget
  (`command.ts:42-48`).
- `LocalCommand` (`type: 'local'`) — runs in-process, returns plain text
  (`/cost`, `/version`).
- `LocalJSXCommand` (`type: 'local-jsx'`) — runs in-process, returns a React
  node (`/install`, `/doctor`). Its `onDone` callback controls how the result
  is threaded back: `display: 'skip'|'system'|'user'`, `shouldQuery` (send to
  model after), `metaMessages` (model-visible but hidden)
  (`command.ts:107-126`).

`CommandBase` (`command.ts:175-203`) carries the metadata used everywhere:
`description`, `aliases`, `argumentHint`, `isHidden`, `isEnabled()`,
`userInvocable`, `disableModelInvocation` (block the *model* from invoking it),
`isSensitive` (redact args from history), `immediate` (bypass the stop-point
queue).

**Custom / user commands — on-disk markdown.** Discovered from `.claude/<subdir>`
directories for `subdir ∈ {commands, agents, output-styles, skills, workflows}`
(`claude-code: src/utils/markdownConfigLoader.ts:29-38`). Discovery walks
**cwd → git root**, then adds the **user** dir (`~/.claude/commands`) and a
**managed/policy** dir, with precedence **managed > user > project**
(`markdownConfigLoader.ts:234-378`); files are deduped by inode to survive
symlinked `~/.claude` (`:384-414`). The scanner is ripgrep
(`--files --hidden --follow --no-ignore --glob *.md`) with a native fallback
(`:558-575`).

- **Format**: a `.md` file, name = filename without `.md`; nested dirs become a
  `:`-namespace (`a/b/foo.md` → `a:b:foo`)
  (`src/utils/plugins/loadPluginCommands.ts:82-96`).
- **Frontmatter fields** (`loadPluginCommands.ts:263-298`): `description`,
  `argument-hint`, `arguments` (named args), `allowed-tools`, `model`
  (`inherit` | alias like `haiku`/`sonnet`/`opus`), `effort`,
  `disable-model-invocation`, `user-invocable`, `shell` (commands to run and
  splice in). Every custom command becomes a `PromptCommand`.
- **Argument placeholders** (`src/utils/argumentSubstitution.ts:86-145`):
  `$ARGUMENTS` (full string), `$ARGUMENTS[n]` / `$n` (positional, shell-quote
  parsed), and named `$foo` when `arguments:` names them. If a template has *no*
  placeholder and args were passed, `\n\nARGUMENTS: {args}` is appended
  (`:140-142`). Body also expands `${CLAUDE_PLUGIN_ROOT}`,
  `${CLAUDE_SESSION_ID}`, `${user_config.X}` (secrets redacted), and runs
  `!`-shell blocks (`loadPluginCommands.ts:326-400`).

**Plugin commands.** Loaded from each enabled plugin's `commands/` dir (plus
extra `commandsPaths`), always namespaced `<plugin>:<name>`
(`loadPluginCommands.ts:414-677`, `:532-542`). Plugins may declare commands via
files, extra paths, object-metadata (per-command `description`/`model`/
`allowedTools` overrides), or **inline content** with no source file
(`:607-668`). Skills (`SKILL.md` dirs) load through the same
`createPluginCommand` path with `isSkillMode` (`:687-838`). MCP servers also
contribute commands (`source: 'mcp'`), merged by `useMergedCommands`
deduping on `name` (`src/hooks/useMergedCommands.ts:5-15`).

**Pipeline.** `parseSlashCommand` splits `/name args`, with a special `(MCP)`
suffix marker (`src/utils/slashCommandParsing.ts:25-60`). Local commands run
in-process; prompt commands call `getPromptForCommand` and the returned blocks
enter the conversation as a user turn (`disableModelInvocation` gates whether
the *model* can call it as a tool; `userInvocable` gates whether the *user* can
type it). `immediate` bypasses the queue (`command.ts:199`).

**Autocomplete.** `Fuse.js` fuzzy over a cached index keyed by the (memoized)
command array; weighted keys `commandName:3, partKey:2, aliasKey:2,
description:0.5`, `threshold 0.3`, prefer prefix
(`src/utils/suggestions/commandSuggestions.ts:30-80`). Separators `[:_-]`
split names into parts so `git:commit` matches `commit`. Supports **mid-input**
slash tokens (a `/` preceded by whitespace, not just at column 0)
(`:99-120`). Skill-usage score feeds ranking (`:9`).

**Injection role.** Prompt commands inject as a normal **user** turn (the
`ContentBlockParam[]` from `getPromptForCommand`); `metaMessages` inject
model-visible-but-UI-hidden `isMeta` messages (`command.ts:124`).

### Codex — pure client-side enum, prompts are the exception

**Built-in set + registry.** A single `#[derive(EnumIter, EnumString)]` enum
`SlashCommand` in `codex: tui/src/slash_command.rs:12-79`. Enum order **is**
popup order ("DO NOT ALPHA-SORT" `:13`); kebab-case serialization; `strum`
aliases (`quit`→`exit`, `clean`→`stop`, `pet`→`pets`, `approve`→`AutoReview`).
~55 commands: `/model` `/ide` `/permissions` `/keymap` `/vim` `/review`
`/rename` `/new` `/resume` `/fork` `/init` `/compact` `/plan` `/goal` `/agent`
`/side` `/status` `/usage` `/mcp` `/theme` `/quit`, plus debug/experimental
ones. Per-command **capability predicates** rather than a type union:
`description()`, `supports_inline_args()` (`:153-171`),
`available_in_side_conversation()` (`:174-185`), `available_during_task()`
(`:188-244`), `is_visible()` (platform/debug gates, `:246-254`).

**Everything is a client-side action.** `dispatch_command` is one giant `match`
(`codex: tui/src/chatwidget/slash_dispatch.rs:141-535`): each arm sends an
`AppEvent` or opens a popup (`/model` → `open_model_popup`, `/new` →
`AppEvent::NewSession`, `/diff` → spawn `get_git_diff`, `/quit` →
`request_quit`). The **only prompt-templates** are `/init` (bundles
`prompt_for_init_command.md` via `include_str!` and `submit_user_message`,
`:252-255`) and `/review …` (submits an `Op` with custom instructions,
`:869-873`); `/compact` triggers a summarization op (`:256-262`). Inline-arg
commands (`/goal`, `/plan`, `/side …`, `/rename …`) get a second dispatch path
`dispatch_command_with_args` (`:542-607`).

**Custom / plugin commands.** This snapshot's TUI popup carries **only**
`CommandItem::Builtin` and `CommandItem::ServiceTier`
(`tui/src/bottom_pane/command_popup.rs:31-85`) — no on-disk custom-prompt files
in the popup (`docs/slash_commands.md` just links out). `CustomPromptView` is a
review-instructions textarea, not a command loader
(`tui/src/bottom_pane/custom_prompt_view.rs:32-69`). So Codex is the **least
extensible**: a fixed, code-defined command set.

**`/model` seam (load-bearing for locode).** Selecting a model in the popup
fires **two** events — a runtime override and a persistence write
(`tui/src/chatwidget/model_popups.rs:685-707`):
`AppEvent::UpdateModel(model)` + `AppEvent::UpdateReasoningEffort` apply now,
then `AppEvent::PersistModelSelection { model, effort }` writes config. There
is an explicit `apply_model_and_effort_without_persist` variant for
session-only changes. Model choice is a first-class app event, not a mutation
buried in the command.

**Autocomplete.** Case-insensitive prefix/exact filtering (not a fuzzy library)
over the enum's presentation order, with alias rows hidden
(`command_popup.rs:20-23`, filter at `:146-196`); availability flags
(`BuiltinCommandFlags`) gate which commands appear per session state.

### Grok Build — the unification model: one trait, many sources

Grok is the reference for **how to unify multiple command sources behind one
abstraction** (the repo's stated model). The whole system lives in
`grok: xai-grok-pager/src/slash/` (`mod.rs`, `command.rs`, `registry.rs`,
`commands/` with ~60 files).

**One trait, typed result.** `SlashCommand` (`slash/command.rs:132-306`) with
`&str` (not `&'static str`) returns so **runtime-sourced** commands work.
`run()` is **synchronous** and returns a typed `CommandResult`
(`command.rs:34-77`) — this is the client-action-vs-prompt-template split made
into data:

- `Handled` / `HandledNoOp` — client action, no output.
- `Message(String)` / `Error(String)` — client-side text into scrollback.
- `Action(Action)` — dispatch a pager `Action` (e.g. `SwitchModel`, `Quit`);
  async work is deferred to the reducer/effect layer.
- `QueueCommand(String)` — route through the queued-command pipeline
  (`/compact`).
- `InjectSkill { display_text, prompt_blocks, display_as_skill,
  scheduled_task_preview }` — **prompt-template**: pager reads `SKILL.md`,
  substitutes, and sends structured `ContentBlock`s to the model while showing
  `display_text` in scrollback.
- `PassThrough(String)` — send as a plain prompt / let the shell resolve it
  (covers ACP-advertised commands *and* unknown commands).

Rich metadata gates: `takes_args`/`args_required` (the "two-bit completeness
model", `command.rs:148-163`, `mod.rs:1153-1186`), `visible(ctx)`,
`session_scoped()` + `offered_when_session_less()` + `dashboard_only()` (surface
scoping), `available_in_minimal()` (denylist for the scrollback-native mode),
`required_tools()` (hide until the agent advertises the tool),
`suggest_args()`, and live `preview_arg`/`cancel_preview` (e.g. theme preview
while arrowing).

**Registry = builtins + ACP-advertised, builtins win.**
`CommandRegistry` (`slash/registry.rs:83-123`) holds commands tagged
`CommandSource::{Builtin, Acp}`. `set_acp_state`/`set_acp_commands`
(`:426-505`) **replace only the Acp-sourced entries** on every
`AvailableCommandsUpdate`, preserving builtins; ACP commands **colliding with a
builtin name/alias are silently skipped** (`:491-501`), and a `BLOCKED_NAMES`
list (`help`, per-server `hooks-*`, `reload-plugins`) refuses shell commands
that would duplicate a unified pager modal (`:478-488`). Custom commands and
skills therefore reach the pager **through the ACP protocol from the shell/agent
backend**, not from TUI-side files (`docs/user-guide/04-slash-commands.md:5-10`:
"Shell builtins … Pager builtins … Skills installed via SKILL.md also appear").

**Layered visibility** (`registry.rs`): `hidden` (hard, fail-closed — not
offered, not executable: `/dashboard` `/recap` `/voice` `/auto` until a feature
gate flips them), `menu_hidden` (menu-only — hidden from completion but a typed
invocation still dispatches via `get_for_dispatch`, `:194-201`), `restricted`
(per-user/tier deny — *stays* in the dropdown for discoverability but execution
shows an upsell, `:230-261`), and `available_tools` gating
(fail-closed when the toolset is unknown, `:276-285`). `get()` applies every
gate; `get_for_dispatch()` bypasses only the menu-only one.

**Autocomplete — the most sophisticated.** `SlashController`
(`slash/mod.rs:250-984`) derives an immutable `SlashSnapshot` from text+cursor
on every keystroke. Features: leading-`/` **and** mid-text `/token` completion
with teal highlighting of recognized tokens (`scan_inline_slash_tokens`,
`:1300-1339`); a two-phase model/args flow (`command_suggestions` →
`arg_suggestions`); **nucleo** fuzzy matcher with "smart case"
(`command_prefix_matches_smart`, `:82-98`); dedup to best trigger per command
with tiebreakers exact > canonical-over-alias > lexicographic (`:851-914`);
**MRU/recency** ranking persisted off-thread (`mru.rs`, ranking at `:898-914`);
inline **ghost text** completing the selected row (`:104-139`); an argument
`placeholder` shown when args are empty. `is_command_complete`
(`:1165-1186`) decides whether Enter executes or the dropdown blocks
(`/model` with no arg blocks; `/compact` executes).

**`/model` seam.** `ModelCommand.run` returns `Action::SetDefaultModel(id)` for
a bare name — the dispatcher routes that through **both** `Effect::SwitchModel`
(session mutation) **and** `Effect::PersistSetting` (next-session default) — or
`Action::SwitchModel { model_id, effort }` for a session-only change
(`slash/commands/model.rs:77-101`, `:444-466`). Same two-layer pattern as
Codex, expressed as typed reducer Actions. `suggest_args` drives chained
autocomplete: pick a reasoning model → a trailing space re-opens the dropdown
into a `low|medium|high|xhigh` sub-menu (`:55-65`, `:153-201`).

### opencode — config-driven, everything is a template, app-actions are separate

**Two clean layers.**

1. **Prompt-template commands** live in the core `Command` service
   (`opencode: packages/opencode/src/command/index.ts:58-176`). Sources merged
   into one `Record<name, Info>`: two built-ins (`init`, `review`, from
   `template/*.txt`, `:70-88`), **config commands** (`cfg.command`, `:90-103`),
   **MCP prompts** (`:105-132`), and **skills** (`:134-152`). Every entry is a
   template with `{ name, description, agent, model, subtask, template, hints }`
   (`Info`, `:22-34`). `subtask: true` runs it as a sub-agent (parallel to
   Claude's `context:'fork'`).

2. **Client-side app commands** are entirely separate, in the TUI app context
   (`packages/app/src/context/command.tsx:75-108`): `CommandOption { id, title,
   category, keybind, slash, onSelect, when }`. These are the `/new`,
   switch-model, palette actions — dispatched by `onSelect`, never sent to the
   model. Prompt-template commands and app commands are surfaced together in the
   command palette but are structurally distinct.

**Custom / user commands — on-disk markdown.** `ConfigCommand.load(dir)`
globs `{command,commands}/**/*.md` (`packages/opencode/src/config/command.ts:13-39`).
Discovery scopes: **project** `.opencode/command(s)/<name>.md` (opencode walks
cwd → worktree root) and **global** `~/.config/opencode/command(s)/<name>.md`;
scopes are **deep-merged, project overrides global**
(`packages/core/src/plugin/skill/customize-opencode.md:46-58`).

- **Format**: markdown; the **body is the `template`** (required), frontmatter
  supplies `description`, `agent`, `model`, `variant`, `subtask`
  (`packages/core/src/v1/config/command.ts:5-13`, doc `customize-opencode.md`
  Commands section). Name = path with prefix stripped and extension removed →
  nested dirs namespace with `/` (`git/commit.md` → `git/commit`,
  `packages/opencode/src/config/entry-name.ts`).
- **Placeholders**: `$ARGUMENTS` (full) and `$1`,`$2`,… (positional). `hints()`
  extracts them from the template for the arg-hint UI
  (`command/index.ts:36-44`). MCP prompt args map to `$1..$n`
  automatically (`:117`).

**Plugin commands.** Auto-discovered `.opencode/plugin(s)` plus npm/file specs;
plugins register through the same config-merge path. MCP prompts are the primary
"external command" contributor at runtime (`:105-132`).

**Pipeline & injection.** All template commands resolve to a prompt string
(`$ARGUMENTS`/`$n` substituted) that is submitted as a **user** message to the
selected `agent`/`model`; `subtask` spawns a sub-session. The server exposes
`command.list` for clients (`packages/server/src/handlers/command.ts`).

**Autocomplete.** The command palette (`packages/app/src/components/command-palette.ts`,
`packages/tui/src/component/command-palette.tsx`) lists app commands + template
commands with category grouping, keybind display, and `slash` triggers; hints
render remaining `$n`/`$ARGUMENTS`.

---

## Comparison

| Axis | Claude Code | Codex | Grok Build | opencode |
|---|---|---|---|---|
| **Registry** | `getCommands()` merges builtin + md + plugin + mcp | `SlashCommand` enum (`strum`) | `CommandRegistry`: Builtin + ACP, builtins win | Core `Command` service (templates) + separate app-command palette |
| **Built-in count** | ~120 | ~55 | ~60 pager + shell/ACP | 2 core + N app-actions |
| **Command kinds** | 3 typed (`prompt`/`local`/`local-jsx`) | 1 (all client actions; 2 inject) | 1 trait, 7-variant `CommandResult` | 2 layers: template + app-action |
| **Custom on-disk** | `.claude/commands/**/*.md` (project↑git-root, user, managed) | none in TUI popup | via ACP from shell + `SKILL.md` | `.opencode/command(s)/**/*.md` (project, global) |
| **Frontmatter** | `description, argument-hint, arguments, allowed-tools, model, effort, shell, user-invocable, disable-model-invocation` | n/a | (ACP-advertised metadata) | `description, agent, model, variant, subtask` |
| **Placeholders** | `$ARGUMENTS`, `$ARGUMENTS[n]`, `$n`, named `$foo`; append fallback | n/a (inline args positional) | args string | `$ARGUMENTS`, `$n` |
| **Namespacing** | `:` (dir → `a:b:foo`); plugins `<plugin>:name` | flat kebab-case + aliases | flat name + aliases; ACP collisions skipped | `/` (dir → `a/foo`); scope merge |
| **Plugins** | commands/ dir, extra paths, inline metadata, MCP | none | ACP + blocked-names list | `.opencode/plugin(s)` + MCP prompts |
| **Precedence** | managed > user > project; inode dedup | n/a | Builtin > ACP; hidden > menu_hidden > restricted | project > global (deep merge) |
| **Autocomplete** | Fuse.js weighted, mid-input, skill-usage score | prefix/exact, enum order | nucleo fuzzy + MRU + ghost + 2-phase args + mid-text | palette, category groups, hints |
| **Client vs template split** | typed union | almost all client; `/init`,`/review` template | `CommandResult` variant | two separate layers |
| **`/model` persistence** | `/model` command, `model` config | `UpdateModel` + `PersistModelSelection` events | `SetDefaultModel`→ SwitchModel + PersistSetting | app-action + config `model` |

---

## Pros / cons & best practice

**Client-action vs prompt-template — make it explicit and typed.**
Codex proves an all-client-action set is simple but inextensible; opencode and
Claude prove templates need a real loader. The cleanest expressions are **Grok's
one-trait, typed-`CommandResult`** (a single dispatch surface, the variant says
whether the client acts or the model is prompted) and **opencode's two-layer
separation** (app-actions vs templates never confused). Best practice for a
headless core with a TUI: a **typed result enum** where prompt-template variants
carry `ContentBlock`s and client variants carry a typed Action — so the *core*
can own template expansion while the *TUI* owns client actions.

**Discovery — walk cwd→git-root, add user + managed, dedupe by inode.**
Claude's `getProjectDirsUpToHome` (stop at git root so parent-dir commands don't
leak into a repo) + inode dedup (survive symlinked config homes) is the most
robust; opencode's project↑worktree + global deep-merge is the simplest that's
still correct. Both beat a single flat dir.

**Namespacing — derive from path.** Claude's `a:b:foo` and opencode's `a/foo`
both make nested command dirs unambiguous and collision-resistant; plugin/source
prefixes (`<plugin>:name`) keep third-party commands from shadowing built-ins.

**Precedence & conflicts — a source hierarchy plus explicit skip.** Grok's
"builtins always win, colliding ACP commands silently skipped, `BLOCKED_NAMES`
for would-be duplicates" is the strongest conflict story; Claude's
managed>user>project and opencode's project>global are good defaults. Always
make one source authoritative rather than last-write-wins.

**Safety.**
- `allowed-tools` per command (Claude) scopes what a template may do — critical
  when a project `.md` can inject a prompt.
- `disable-model-invocation` / `userInvocable` (Claude) separate "user can type
  it" from "model can call it as a tool."
- `isSensitive` redacts args from history (Claude) — matters for secrets in args.
- Grok's **fail-closed** tool-gating (hide a command until the agent advertises
  the tool) prevents offering `/loop` before a session can run it.
- `!`-shell expansion and `${…}` interpolation in bodies (Claude) are powerful
  but are an injection surface; secrets must resolve to placeholders, never real
  values, in model-bound text (`loadPluginCommands.ts:346-354`).

**Autocomplete.** A real fuzzy matcher (Fuse/nucleo) beats prefix-only once the
set grows; weight name > alias > description; add MRU/recency (Grok) and prefix
ghost text for fast completion; show `argument-hint`/placeholder and remaining
`$n`. Mid-input `/token` recognition (Claude, Grok) is a nice-to-have, not
essential for v1.

**Availability gating is per-surface and per-state.** Every harness gates
commands by session state (`available_during_task`, `session_scoped`,
`available_in_side_conversation`, `visible`). A headless core with one TUI
needs at least: "requires a live session" and "allowed while a run is active."

---

## Recommendation for locode

locode's constraints: the **core crates stay headless** (no TUI deps, no
interactive prompts; ADR-0001), harness **packs faithfully mimic** their harness
while the **`locode` pack is best-of** (ADR-0012), and the TUI is a reducer with
a `Cmd` effect enum (ADR-0019, `locode-tui/src/app.rs`). The current
`try_slash` (`app.rs:581-606`) is a hard-coded `match` on `/quit|/exit|/new`.

### Core vs TUI split

Slash commands are fundamentally a **TUI/interaction concern**, but
prompt-template expansion touches the conversation the core owns. Split it:

- **`locode-tui` owns the command registry, parsing, autocomplete, and
  client-action dispatch.** Client-action commands (`/quit` `/new` `/clear`
  `/help` `/model` `/theme` `/resume` `/compact` `/context`) map to the existing
  `Cmd` enum. This is the grok/opencode app-action layer. Keep it out of the
  core — the core has no business knowing about `/theme`.
- **The core exposes seams the template commands need**, not the commands
  themselves: (a) a way to inject a user turn built from `ContentBlock`s
  (prompt-template expansion result); (b) a **model-selection seam** (below);
  (c) session lifecycle (already: `NewSession`, continuity per ADR-0016). The
  core already appends user turns; a template command is just "expand `.md` +
  args → user message → submit," which the TUI can do against the existing
  submit path. **No new core public trait is required for v1** — templates
  resolve to text in the TUI.
- **Custom-command *file loading* is a shared utility, not a core trait.** A
  small `locode-tui`-side loader (or a tiny neutral crate) reads the `.md`
  files. It must not print to stdout (headless I/O contract, ADR-0009) — but it
  lives in the TUI/app layer anyway.

### Client-action vs prompt-template — one typed result

Adopt Grok's typed-result shape, sized to locode. A `SlashOutcome` enum in
`locode-tui`:

```
enum SlashOutcome {
    Cmd(Vec<Cmd>),                 // client action: Quit, NewSession, SetModel…
    Prompt(Vec<ContentBlock>),     // prompt-template: submit as a user turn
    Notice(String),                // client-side message into the transcript
    PassThrough,                   // not a command → fall through to submit
}
```

`try_slash` returns `SlashOutcome` instead of today's `Option<Vec<Cmd>>`. This
keeps the single dispatch surface (grok's lesson) while respecting locode's
`Cmd`/reducer architecture (ADR-0019). Unknown `/foo` → `Notice` (as today), not
silently passed to the model.

### Built-in command set (v1, `locode` pack)

Best-of, not a mimic: `/help` `/new` (alias `/clear`) `/quit` (alias `/exit`)
`/model` `/resume` `/compact` `/context` `/theme`. Defer `/review` `/commit`
`/init` to a later slice (they're prompt-templates and can ship as bundled
`.md`). Each carries `description`, optional `aliases`, `arg_placeholder`,
`args_required`, and an "available while running" flag (from Codex's
`available_during_task`).

### Custom-command file format

Follow the **Claude/opencode markdown convention** (the de-facto standard; users
and other harnesses already write these):

- **Discovery**: project `./.locode/commands/**/*.md` (walk cwd → git root, stop
  at git root like Claude) + user `~/.config/locode/commands/**/*.md`; **project
  overrides user** (opencode's rule; simpler than Claude's 3-tier for v1).
- **Name**: filename without `.md`; nested dirs namespace with `:` (Claude's
  separator; matches our `kebab-case`/`snake_case` conventions better than `/`).
- **Frontmatter**: `description`, `argument-hint`, `model` (interacts with the
  model seam below), and — because packs matter — an optional `pack` scoping
  field is *not* needed; custom commands are pack-neutral user content.
- **Body = template**; placeholders `$ARGUMENTS` and `$1..$n` (the common subset
  across Claude/opencode). **Do not** ship `!`-shell expansion or `${…}`
  interpolation in v1 — it's the biggest injection surface and can be added
  behind an explicit opt-in later.
- Every custom command is a **prompt-template** → `SlashOutcome::Prompt`.

### `/model` and the model-selection seam

This is the load-bearing finding. Today the model is a bare
`EngineConfig.model: String` baked in at construction
(`locode-engine/src/config.rs:18`, `run.rs:40`) with **no way to change it at
runtime** — so `/model` cannot be implemented without a new seam. Both Codex
(`UpdateModel` + `PersistModelSelection`, `model_popups.rs:685-707`) and Grok
(`SetDefaultModel` → `SwitchModel` + `PersistSetting`, `model.rs:444-466`)
converge on the **same two-layer pattern**, which locode should copy:

1. **Runtime override** — a `Cmd::SetModel(model_id)` effect that updates the
   engine's active model for subsequent turns (session-scoped). Needs the engine
   to accept a model change between turns; this is a small, real addition to the
   engine's public surface → **ask-first per the Boundaries rule** and reconcile
   ADR-0007 (provider trait) / ADR-0015 (custom-provider injection) *before*
   coding it.
2. **Persistence** — a separate `Cmd::PersistModelSelection(model_id)` that
   writes the choice as the next-session default (config file), kept distinct so
   session-only switches don't clobber the default (Codex's
   `apply_model_and_effort_without_persist`).

`/model`'s argument autocomplete should enumerate models from a
**model-catalog** the provider seam exposes (grok's `ModelState`/`suggest_args`,
`model.rs:55-65`). For **ported packs**, mimic that harness's `/model` UX
faithfully; for the **`locode` pack**, a clean two-phase (model → optional
effort) picker like grok's is the best-of choice.

### Plugins

Out of scope for v1 (locode has no plugin system yet). When it lands, follow
Grok's unification: one registry, **built-ins win**, third-party sources
namespaced and colliding names skipped, with a fail-closed gate for anything
requiring capabilities not yet present.

### Ported-pack faithfulness vs `locode` best-of

Per ADR-0012 and the "faithful mimicry" rule: a **ported pack's** slash set
should mirror that harness (names, which are client-action vs template, the
`/model` UX, arg placeholders) even where a better choice exists — the point is
an honest A/B. The **`locode` pack** gets the best-of design above (typed
`SlashOutcome`, markdown custom commands, two-phase `/model`, fuzzy+MRU
autocomplete). Note explicitly in the pack when faithfulness forces a deviation
from these defaults.

### Proposed ADR

**A new ADR — Slash-command system** (next free number; highest today is
ADR-0022). It should record:

- The **core/TUI boundary**: registry, parsing, autocomplete, and client-action
  dispatch live in `locode-tui`; the core exposes only the model-selection seam
  and the existing user-turn submit path. No new core *command* trait.
- The **typed `SlashOutcome`** split (client-action `Cmd` vs prompt-template
  `ContentBlock`s vs notice vs pass-through).
- The **custom-command markdown format** (dirs, precedence, namespacing,
  frontmatter subset, `$ARGUMENTS`/`$n`, no shell/`${}` in v1).
- The **model-selection seam** (runtime override + separate persistence) —
  since this changes an engine public surface, this new ADR must **supersede/amend
  ADR-0007** in the same change (ADR-first rule), and reconcile with ADR-0015.
- **Faithfulness clause**: ported packs mimic; `locode` pack is best-of.

Deliver as thin slices: (1) refactor `try_slash` → `SlashOutcome` + a builtin
registry with `/help`; (2) autocomplete `/`-menu; (3) markdown custom-command
loader; (4) the model seam + `/model`.

---

## Open questions

1. **Model seam shape.** Does `Cmd::SetModel` mutate `EngineConfig` on the live
   engine, or rebuild the session (as `/new` does)? Between-turn model change
   touches ADR-0007/0015 and is **ask-first** (engine public surface) — confirm
   the intended seam before implementing.
2. **Where does the registry live** — inside `locode-tui`, or a small neutral
   `locode-commands` crate the TUI and any future headless "run a saved command"
   path can share? v1 leans TUI-local; flag if a shared crate is wanted.
3. **`$ARGUMENTS` fallback.** Adopt Claude's "append `ARGUMENTS: {args}` when the
   template has no placeholder"? Convenient but surprising; opencode does not.
4. **Namespacing separator** — `:` (Claude, matches our naming feel) vs `/`
   (opencode). Recommending `:`; confirm.
5. **`/compact` & `/context`** depend on features not yet in the TUI (compaction,
   token accounting). Ship as client-actions now (no-op notice) or defer the
   commands until the features exist?
6. **Faithfulness scope for ported-pack slash sets** — this overlaps the open
   "harness fidelity boundary" concern (mimicry stops at tools+prompts+preamble;
   loop-adjacent behavior stays shared). Are *slash commands* part of a pack's
   mimic surface, or a TUI-only concern shared across packs? This determines
   whether ported packs even *have* distinct slash sets. **Needs a user
   decision.**

### Guessed / reconstructed identifiers (please confirm)

- Read the existing seam from `locode-engine/src/config.rs:18` (`model: String`)
  and `run.rs:40` — inferred there is **no** runtime model-change path today.
- `Cmd::SetModel` / `Cmd::PersistModelSelection` / `SlashOutcome` are **proposed**
  names, not existing code.
- The proposed new ADR assumes 0022 is the current max (confirmed from
  `docs/decisions/`).
