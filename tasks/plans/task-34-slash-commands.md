# Task 34 — slash commands: the core contract and grok's dropdown

> Implements [ADR-0026](../../docs/decisions/ADR-0026-slash-commands-core.md) (core) and
> ports grok's command dropdown (UI). Source grounding:
> [`../../docs/research/harness-study-slash-commands.md`](../../docs/research/harness-study-slash-commands.md).
>
> Sequenced **before** background bash/subagents *(user decision)*: it is the missing
> half of skills. ADR-0025 parses `user-invocable` but ships no user-invocation channel,
> so a shipped skill can only be reached by the model choosing to read it.

## Objective

1. A `/name args` typed in the composer resolves against a registry, executes, and its
   `CommandResult` is applied by the caller — commands themselves stay side-effect free.
2. Every `user-invocable` skill is a command; invoking it splices the skill body plus a
   plain-text `**ARGUMENTS:**` block into the turn (ADR-0026 §4, §8).
3. The dropdown reproduces grok's: fuzzy ranking, **blue matched letters**, a grey
   selected row with a `❯` prefix, argument submenus, and ghost completion.

## Design constraints (from ADR-0026 — not re-litigated here)

- **Trait, not enum**: runtime registration (skills are discovered per run) and
  per-command `suggest_args` are both impossible in codex's enum.
- **`execute` is async** from day 0; `/model` hardcodes its list for v1.
- **Two layers** (ADR-0026 §7 amendment 2026-07-25): the trait/result/registry and
  everything visible live in `locode-tui`; `locode-skills` supplies invocation assembly.
  Dependency runs commands → skills, never the reverse.
- **Arguments are plain text appended**, no `$ARGUMENTS`/`${SKILL_DIR}` — the model's
  path cannot substitute, so any template scheme would make one skill behave two ways.
- **Unknown `/foo` is an error**, not a pass-through; a message merely *starting* with
  `/` that is not a command is ordinary text.
- **No MRU ordering** in v1 (ADR-0026 §3 records it as the known next step).

## Slices

### S1 — trait, result, registry (M)

- `locode-tui::commands`. `SlashCommand` (name/aliases/description/usage, `takes_args`/
  `args_required`, `suggest_args`, `visible`, async `execute`), `CommandResult`
  (Handled/Message/Error/Prompt/InjectSkill/Action), `ArgItem`.
- `CommandRegistry`: register, alias resolution, per-query `visible` filtering, ordered
  listing, lookup returning a typed not-found carrying the closest names.
- Parsing: `/name rest` → `(name, args)`; the "starts with `/` but is not a command"
  case is distinguished here, not in the UI.
- Pure and fully unit-tested; nothing renders yet.

### S2 — skill-backed commands (M)

- `locode-skills`: `invocation_text(skill_body, args) -> String` — body verbatim, then
  a blank line and `**ARGUMENTS:** <args>` when args are non-empty. Pure; skills stays
  unaware that commands exist.
- `locode-tui::commands`: register one command per discovered `user-invocable` skill;
  `execute` reads the `SKILL.md`, assembles, returns `InjectSkill`.
- Collisions: builtin wins, skill reachable as `user:name` (ADR-0025 §2's qualifiers,
  not a second scheme).

### S3 — TUI: detection, state, keys, basic dropdown (M)

- Composer text → `SlashState`: open only when the text starts with `/` at position 0
  and the cursor is inside the command token; query, filtered rows, selection.
- Keys while open: ↑/↓ move, Enter accepts (or executes when unambiguous), Esc closes,
  Tab completes. Everything else falls through to the composer.
- Dropdown rendered above the composer: label + description columns, selected row with
  `theme` background + bold + `❯` prefix (two spaces otherwise, so text never shifts),
  selection kept centred while scrolling.

### S4 — dispatch, and the existing commands move into the registry (M)

- `CommandResult` → TUI effects: `Message`/`Error` become notice blocks, `Prompt` and
  `InjectSkill` submit a turn, `Action` maps to the existing app commands.
- `/new`, `/quit`, `/exit` become registry commands instead of the hard-coded `match`.
- **This is the slice that makes the feature real** — everything after it is ranking
  and polish.

### S5 — `nucleo` ranking and blue matched letters (M)

- Rank rows with nucleo (`CaseMatching::Smart`, `Normalization::Smart`); empty query =
  insertion order so the menu is populated before typing.
- `indices()` → highlight spans grouped into **consecutive runs** of matched/unmatched
  (grok's `build_highlighted_spans`), styled accent vs primary. Runs, not per-character.

### S6 — argument submenu + `/model` (M)

- When a command is recognized and takes args, the dropdown switches to its
  `suggest_args` items, matched on `match_text`, showing `display`, inserting
  `insert_text`.
- `/model` ships a hardcoded list (ADR-0026 §6); `args_required` blocks bare Enter with
  the usage string.

### S7 — ghost text (S)

- Name completion: typed `/comm`, best match `commit` ⇒ ghost suffix `it`, re-synced
  when the selection moves.
- Argument hint: the `usage()` string once a command is recognized.
- grok's second ghost renderer was **not traced** in the study — confirm against source
  before implementing rather than assuming it shares the first's path.

## Preset targets (gate per slice + final)

- **S1**: `/model gpt` parses to `("model", "gpt")`; `/nope` returns not-found listing
  near names; a command whose `visible` is false never appears.
- **S2**: a `user-invocable` skill appears as a command; invoking with args yields the
  body plus one `**ARGUMENTS:**` block; `user-invocable: false` does not register.
- **S3**: typing `/` opens the menu; ↑/↓ change selection; Esc closes; the selected row
  renders with its background, bold, and `❯`.
- **S4**: `/quit` exits; `/new` clears; an unknown `/foo` shows an error naming close
  matches; a skill command submits a turn carrying the body.
- **S5**: `/mdl` ranks `model` first; the matched letters are separate accent spans.
- **S6**: `/model ` shows the model list; picking one inserts its id; bare `/model` +
  Enter is refused with the usage string.
- **S7**: `/comm` shows ghost `it`; Tab accepts it.
- Four-part gate (`fmt · clippy · test · doc`) per slice; PR per slice, auto-merge on
  green.

## Result

_(pending)_
