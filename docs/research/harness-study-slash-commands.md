# Harness study — slash commands (`/…`)

Source study of the **slash-command** surface across the surveyed harnesses, split the
way the work will be done: a **core** half (what a command *is*, how it is registered,
matched, and dispatched) and a **UI** half (the dropdown, fuzzy highlighting, submenus,
ghost text). Conducted 2026-07-24 against the `coding-cli-survey` submodules; citations
are `harness: path:line` relative to each submodule root.

**Grok Build is the model to copy for the UI** (user decision — its dropdown is the
reference: fuzzy matching, blue matched letters, grey selected row, argument submenus,
ghost completion). Codex and Claude Code are read for the **core** shape only.

Why this lands before background agents/subagents: **skills are hard to use without
it.** ADR-0025 ships discovery + a listing, and leaves `user-invocable` parsed but
inert because there is no user-invocation channel. Slash commands are that channel —
grok's own `CommandResult::InjectSkill` exists for exactly this.

---

## Headline findings

1. **The two Rust harnesses disagree on the central design question**, and it is the
   one decision our ADR has to make: grok models a command as a **trait object with
   behavior** (`trait SlashCommand`, dynamic registry, per-command argument
   suggestions); codex models it as a **static enum** whose order *is* the menu order.
   Claude Code sits closer to grok (rich per-command metadata) but with commands as
   data + handlers rather than a trait.
2. **Only grok does fuzzy matching.** It wraps **`nucleo`** (Helix's matcher) and uses
   the returned match indices to colour individual letters. Codex does exact/prefix
   bucketing and highlights a *contiguous* run. This is a dependency decision for us.
3. **Argument suggestions are a first-class part of grok's command trait**
   (`suggest_args`), which is what produces the second-level menu (`/model` → the model
   list). Neither of the other two has an equivalent.
4. **grok has a dedicated `InjectSkill` result variant**, so a user-invoked skill goes
   through the *same* dispatch path as a builtin and lands as structured prompt blocks.

---

## Core

### grok — a trait, a registry, and a result enum

**The command** (`xai-grok-pager/src/slash/command.rs:132-180`):

```rust
pub trait SlashCommand: Send + Sync {
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&str] { &[] }
    fn description(&self) -> &str;          // dropdown text
    fn usage(&self) -> &str;                // "/model <name>"
    fn takes_args(&self) -> bool { false }
    fn args_required(&self) -> bool { false }
    fn suggest_args(&self, ctx: &AppCtx, args_query: &str) -> Option<Vec<ArgItem>> { None }
    fn visible(&self, ctx: &AppCtx) -> bool { true }
    // … execute(ctx) -> CommandResult
}
```

The `takes_args`/`args_required` pair is documented in-source as a deliberate
**two-bit model**, with the truth table spelled out (`command.rs:157-165`):

| `takes_args` | `args_required` | Example | Enter with no args |
|---|---|---|---|
| `false` | `false` | `/exit` | executes |
| `true` | `false` | `/compact [ctx]` | executes |
| `true` | `true` | `/model <id>` | **blocks** |

**The result** (`command.rs:34-77`) — a command does not touch the world directly; it
returns one of:

`Handled` · `HandledNoOp` · `Error(String)` · `Message(String)` · `Action(Action)`
(a pager action such as SwitchModel/Quit) · `QueueCommand(String)` (rides the normal
queued-prompt pipeline, e.g. `/compact`) · **`InjectSkill { display_text,
prompt_blocks, display_as_skill, scheduled_task_preview }`** · `PassThrough(String)`
(send as an ordinary prompt — used for both server-advertised and *unknown* commands,
with an in-source note that the two are deliberately merged for now).

**Argument suggestions** are `ArgItem { display, match_text, insert_text, description }`
(`command.rs:81-93`) — note the split between *what is shown*, *what is matched*, and
*what is inserted*, which is what lets `/model` show "Grok 4.5 (current)" while
inserting a model id.

**The registry** (`slash/registry.rs`) tracks provenance with a `CommandSource`
(`Builtin` vs `Acp` — server-advertised), uses it for precedence and replacement, and
can hide commands at runtime (`set_plugins_visible`). A separate `slash/mru.rs` (395
lines) keeps most-recently-used ordering as a tiebreak, because single-letter queries
tie many commands at the same fuzzy score (`matcher.rs` test
`query_p_ties_personas_and_pager_headless_at_same_score` documents exactly that).

### codex — a static enum whose order is the menu

`codex-rs/tui/src/slash_command.rs:12` is a plain `enum SlashCommand` with `strum`
serialization, carrying an explicit warning:

> `// DO NOT ALPHA-SORT! Enum order is presentation order in the popup, so more
> frequently used commands should be listed first.`

Simple and very cheap, at the cost of: no per-command argument suggestions, no runtime
registration (an MCP- or plugin-provided command cannot join the enum), and
presentation coupled to declaration order.

### Claude Code — data + handlers, with availability gates

`src/types/command.ts:205` defines `Command` as a `CommandBase & …` union (type
`'prompt'` among others), and `src/commands.ts` runs an **availability check before
`isEnabled()`** so provider-gated commands are hidden rather than shown-and-refused
(`commands.ts:413`), with the caching note that "availability and isEnabled checks run
fresh every call" (`:475`). The lesson worth taking: **visibility is dynamic and
evaluated per keystroke**, not fixed at registration.

### Core comparison

| Axis | grok | codex | Claude Code |
|---|---|---|---|
| Command shape | `trait` object | static `enum` | data + handler |
| Runtime registration | yes (`CommandSource`) | no | yes |
| Argument suggestions | **`suggest_args` → submenu** | — | — |
| Dynamic visibility | `visible(ctx)` | — | availability + `isEnabled`, per call |
| Result model | 8-variant `CommandResult` | direct dispatch | handler return |
| Skill invocation | **`InjectSkill` variant** | — | slash-invokes a skill |
| Ordering | fuzzy score → MRU → tiebreaks | enum order | grouping |

---

## UI (grok, the copy target)

### Fuzzy matching — `nucleo`

`slash/matcher.rs` is a thin wrapper over nucleo's `MultiPattern` + `Matcher`:

- `rank(items, query, limit, key_fn) -> Vec<(index, score)>` — sorted by descending
  score then **ascending key text**; an empty query returns insertion order, so the
  menu is populated before the user types anything (`matcher.rs:44-88`).
- `indices(text) -> Vec<u32>` — the **character positions that matched**
  (`matcher.rs:93-102`). This is the entire basis of the coloured letters.
- Case handling is `CaseMatching::Smart` + `Normalization::Smart`.

### Blue matched letters

`views/slash_dropdown.rs` → `build_highlighted_spans(text, indices, normal_style,
match_style)`: walk the characters, group **consecutive runs** of matched/unmatched
into single spans, style each run with `match_style` or `normal_style`. Runs, not
per-character spans — fewer spans and no styling seams.

Colours come from the theme: `theme.fuzzy_accent` for matches, `theme.text_primary`
for the rest, `theme.gray` for the description column.

### Selected row

Same file, the row builder:

- **background**: `row_bg` from `embedded_row_style(theme, is_selected)`, falling back
  to `theme.bg_visual` — the grey band in the screenshot. Applied to *every* span in
  the row (including the padding spans) so the fill spans the full width.
- **bold**: `Modifier::BOLD` on the selected row's label.
- **prefix**: `glyphs::prompt_arrow()` (`❯`) when selected, two spaces otherwise — so
  the text never shifts as the selection moves.
- The description column is word-wrapped to the remaining width
  (`simple_word_wrap`), and scrolling keeps the selected row centred
  (`completion_dropdown.rs:40-52`).

### Two dropdowns, not one

`views/slash_dropdown.rs` (slash commands, **with** highlighting) and
`views/completion_dropdown.rs` (generic completions, single-span labels, **no**
highlighting). Worth knowing so the right one is used as the reference.

### Ghost text — two distinct mechanisms

1. **Name completion** — `inline_ghost_from_selected_command`
   (`slash/mod.rs:105-127`): the user typed `/comm`, the best match is `commit`, so the
   ghost is the *suffix* `"it"`. Guarded by a smart prefix check, and re-synced whenever
   the selection moves (`sync_inline_ghost_to_selection`).
2. **Argument hint** — the `usage()` string (`/model <model> [effort]` in the
   screenshot) shown once a command is recognized. **Not yet traced to its renderer**;
   confirm during implementation rather than assuming it shares the ghost path.

---

## What this implies for us

**Core** (the ADR's scope): a trait-shaped command with `name`/`aliases`/
`description`/`usage`/the two-bit args model/`suggest_args`/`visible`, a registry with
provenance and dynamic visibility, and a result enum that keeps commands side-effect
free — including the variant that invokes a skill, which is what makes ADR-0025's
`user-invocable` mean something.

**UI** (a later plan, built in small increments against the source): nucleo-backed
ranking, run-grouped highlight spans, a themed selected row with a `❯` prefix,
argument submenus fed by `suggest_args`, and the two ghost mechanisms.

**One dependency decision** the ADR must record: **`nucleo`**. Everything visible in
the screenshots — the ranking, and the match indices that drive the coloured letters —
comes from it. Writing our own fuzzy matcher to avoid the dependency would be the same
mistake as hand-rolling the YAML frontmatter reader (ADR-0025 §2): the harness we are
copying uses a real library, and the hand-rolled version silently loses cases.

## Open questions for the ADR

1. **Trait or enum?** grok's trait buys runtime registration (skills, and later plugins
   and server-advertised commands) and per-command argument suggestions. codex's enum
   is far cheaper. Our skills requirement points at the trait.
2. **Where does the registry live** — `locode-tui` (a UI concern, as in both harnesses)
   or the core? Commands that only manipulate the UI argue for the TUI; commands that
   inject a prompt or a skill body touch the engine's seams.
3. **Does an unknown `/foo` pass through as a prompt** (grok's `PassThrough`) or error?
4. **How does a skill become a command** — every `user-invocable` skill registers
   automatically (grok's model), or only on an explicit opt-in?
