# ADR-0026: Slash commands — the core contract

## Status
Accepted

## Date
2026-07-24

## Scope

**Core only.** What a command *is*, how it is registered and looked up, what it may
return, and how a skill becomes one. The dropdown — fuzzy ranking, matched-letter
highlighting, the selected row, argument submenus, ghost text — is a **TUI**
implementation built later against the source, in small increments, and is
deliberately out of this ADR. Source study:
[`../research/harness-study-slash-commands.md`](../research/harness-study-slash-commands.md).

## Context

ADR-0025 shipped skills with two invocation switches and only one channel. Its own
words: `user-invocable: false` "is parsed and recorded but has **no observable
behavior** until slash-command invocation exists", and its Open Questions leave slash
invocation to "the deferred slash-command design pass". That pass is this ADR, and it
is sequenced **before** background tasks and subagents *(user decision)* — without a
way to say `/commit`, the skills feature that just shipped can only be reached by the
model deciding to read a file, never by the user asking for it directly.

The tracker's existing entry deferred slash commands "pending a *holistic* design pass
(discovery/registry, syntax, pure-UI vs seam- or persistence-backed), not piecemeal".
This is that pass.

The three harnesses split on the central question — whether a command is a **trait
object with behavior** (grok), a **static enum** (codex), or **data plus a handler**
(Claude Code). The study details each; the choice below is the load-bearing decision.

## Decision

### 1. A command is a trait object, not an enum

We take grok's shape (`xai-grok-pager/src/slash/command.rs:132-180`):

```rust
pub trait SlashCommand: Send + Sync {
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&str] { &[] }
    fn description(&self) -> &str;   // the dropdown's right column
    fn usage(&self) -> &str;         // "/model <name>" — the argument hint
    fn takes_args(&self) -> bool { false }
    fn args_required(&self) -> bool { false }
    fn suggest_args(&self, ctx: &CommandCtx<'_>, query: &str) -> Option<Vec<ArgItem>> { None }
    fn visible(&self, ctx: &CommandCtx<'_>) -> bool { true }
    async fn execute(&self, ctx: &CommandExecCtx<'_>) -> CommandResult;  // §6
}
```

**Why not codex's enum**, which is materially cheaper. Two things we need are
impossible in it:

- **Runtime registration.** Every `user-invocable` skill becomes a command (§4), and
  skills are discovered from disk *per run* — they cannot be enum variants. Plugins and
  any future server-advertised commands have the same problem.
- **Per-command argument suggestions.** The second-level menu (`/model` → the model
  list) is `suggest_args` on the command itself. An enum would need a parallel
  `match` that every new command must remember to extend.

Codex's enum-order-is-menu-order trick is genuinely nice and we lose it; ordering
becomes explicit instead (§3).

**The two-bit argument model** is grok's, including its truth table, because the
distinction is real and easy to get wrong:

| `takes_args` | `args_required` | Example | Enter with no args |
|---|---|---|---|
| `false` | `false` | `/exit` | executes |
| `true` | `false` | `/compact [ctx]` | executes |
| `true` | `true` | `/model <id>` | **blocked, with the usage string** |

`ArgItem { display, match_text, insert_text, description }` keeps *shown*, *matched*
and *inserted* separate — that separation is what lets `/model` list "Grok 4.5
(current)" while inserting a model id.

### 2. Commands return a value; they do not act

`execute` returns a `CommandResult` and touches nothing itself. The caller — the TUI
today, anything else later — performs the effect. This is grok's model and the reason
its command set is testable without a terminal.

The variants we need, pared from grok's eight:

| Variant | Meaning |
|---|---|
| `Handled` | done, nothing to show |
| `Message(String)` | user-visible text (a notice block) |
| `Error(String)` | failed, with a reason |
| `Prompt(String)` | send this text as an ordinary prompt |
| `InjectSkill { display_text, blocks }` | a skill body, as structured prompt blocks (§4) |
| `Action(UiAction)` | a UI action the caller interprets (`/new`, `/quit`, …) |

Dropped deliberately: grok's `HandledNoOp` (its own doc says dispatch treats it
identically to `Handled` — it exists for TUI parity we do not owe anyone) and
`QueueCommand` (we have no queued-prompt pipeline yet; `Prompt` covers the case until
one exists).

### 3. Ordering is explicit, and visibility is per-keystroke

Codex's "enum order is presentation order" cannot survive a dynamic registry, so each
command carries an explicit sort key and ties break on name. **Most-recently-used
ordering is deliberately not in v1** — grok needs its 395-line `mru.rs` because
single-letter queries tie many commands at the same fuzzy score (its own test
`query_p_ties_personas_and_pager_headless_at_same_score` records this), which is a
problem worth solving only once we *have* dozens of commands.

`visible(ctx)` is evaluated **on every query**, not at registration — Claude Code's
rule, where availability runs "fresh every call" and gated commands are hidden rather
than shown-and-refused (`commands.ts:413,475`). A command that cannot run should not
be offered.

### 4. Every `user-invocable` skill is a command

Discovery already returns each skill's name, description and path (ADR-0025 §2), and
parses `user-invocable` — which has had no effect until now. On registration, each
discovered skill with `user-invocable: true` (the default) is registered as a command:
`name` from the skill, `description` from its frontmatter, `usage` `/<name> [args]`.

Invoking it reads the `SKILL.md`, appends the arguments block (§8), and returns
`InjectSkill` — the body reaches the model as structured prompt blocks, which is
grok's `CommandResult::InjectSkill` and the *zero-round-trip* path the skills study
describes (the model does not have to go read the file it was just asked to use).

Two ADR-0025 statements become live, and both should be read as amended: the
`user-invocable` switch now has observable behavior, and its §4 note that "user-invoked
skills [might] splice the body in at prompt-assembly time rather than making the model
read it" is **decided in favor of splicing**.

Name collisions between a skill and a builtin resolve **builtin-wins**, with the skill
reachable by its qualified name (`user:commit`), reusing ADR-0025 §2's qualifier
scheme rather than inventing a second one.

### 5. An unknown `/foo` is an error, not a prompt

grok passes unknown commands through to its server as an ordinary prompt
(`PassThrough`), because a server it does not control may know commands it does not.
We have no such server: our registry is the whole world. So an unknown command is a
plain error naming the closest matches, and a message that merely *starts* with `/`
but is not a recognized command (`/usr/bin/env`, `/dev/null` at the start of a
sentence) is sent as ordinary text.

## Out of scope

The entire dropdown — nucleo-backed ranking, run-grouped highlight spans, the themed
selected row and `❯` prefix, argument submenus, and the two ghost-text mechanisms —
is a TUI plan, built in increments against the source. This ADR only guarantees the
core exposes what that UI needs: `suggest_args` for the submenu, `usage` for the
argument hint, `description` for the second column, and a ranked, visibility-filtered
lookup.

Also out: MRU ordering (§3), plugin- and server-provided commands (the registry's
provenance field is designed for them, but nothing registers them yet), and command
*history*.

## Consequences

- **`user-invocable` stops being inert**, closing the ADR-0025 gap that made skills
  model-only. This is the reason the task is sequenced ahead of background work.
- **A new registry with runtime registration** — the first place where a disk artifact
  (a skill) becomes an interactive surface. Its placement is an open question below.
- **The TUI gains a real input mode**: `/` at position 0 opens a menu that intercepts
  keys. That interacts with the composer and the approval overlay, and is the largest
  part of the UI work.
- **Ordering is explicit and will need revisiting** once the command count grows; §3
  records MRU as the known next step rather than pretending the problem is solved.

## Resolved (user decisions, 2026-07-25)

### 6. `execute` is async from day 0

*(User decision.)* `async_trait` is already a workspace dependency — `Tool` and
`Provider` both use it — so an async trait costs essentially nothing now, while
turning a sync trait async later is a breaking change to every implementation. The
motivating case is a future `/model` that lists models from a provider rather than a
hardcoded set; **v1 hardcodes the list**, and nothing in the command set needs `.await`
yet.

### 7. Where the code lives — three layers, most of it in the TUI

*(User decision.)* The registry belongs in the core; the skill-specific parts belong
with skills; everything else is UI and should stay there.

| Layer | Contents |
|---|---|
| **`locode-commands`** (new crate) | the `SlashCommand` trait, `CommandResult`, `ArgItem`, and the registry (registration, provenance, visibility filtering, ordered lookup) |
| **`locode-skills`** | building a skill *invocation*: read the `SKILL.md`, assemble the body + arguments block (§8). A pure function — skills stay unaware that commands exist |
| **`locode-tui`** | everything visible: the dropdown, ranking and its highlight spans, ghost text, key handling, and the UI-only commands (`/new`, `/quit`) |

The dependency runs `locode-commands → locode-skills`, not the reverse: the registry
needs to enumerate skill-backed commands, while skills has no reason to know the
command trait. Both harnesses put the whole thing in their TUI; we split only because
`InjectSkill` genuinely needs skill loading, and a UI crate is the wrong home for it.

**Release consequence:** an eleventh crate, published after `skills` in dependency
order (`… → host → instructions → skills → commands → provider → …`).

### 8. Arguments are plain text appended after the body

*(User decision.)* `/commit fix the typo` injects the skill body **verbatim**, followed
by a marker block carrying the argument text:

```text
<the SKILL.md body, unchanged>

**ARGUMENTS:** fix the typo
```

No template syntax: no `$ARGUMENTS`, no `$1`, no `${SKILL_DIR}`. A skill that wants
arguments refers to them in **ordinary prose** ("the user's request is in ARGUMENTS
below"). With no arguments, no block is appended.

**The reason is consistency, not simplicity.** The model's path physically cannot
substitute: it opens `SKILL.md` with its read tool (ADR-0025 §4), and intercepting
reads of skill files is exactly the machinery that ADR rejected. So any substitution
scheme would apply on the slash path only — and the *same skill* would behave
differently depending on who invoked it. A body containing `${SKILL_DIR}/scripts/x.sh`
would work when the user types `/x` and break when the model reads it. Appending plain
text is the only option where both paths see identical body text.

Grok does substitution as its primary path with this append as its **fallback**
(`apply_substitutions`, the `**ARGUMENTS:**` suffix); we take the fallback as the whole
contract.

**Known cost:** a skill imported from Claude Code or grok may contain a literal
`$ARGUMENTS`, which will reach the model unexpanded. Not fatal — the argument text is
right there under the marker — but visibly untidy. `${SKILL_DIR}` matters more, and is
already covered without tokens: the listing carries each `SKILL.md`'s absolute path, so
a skill can say "the scripts are in `scripts/` next to this file". **Revisit when
importing other harnesses' skills becomes a real need** — and revisit it for *both*
paths, rather than giving the slash path a private capability.

## Open Questions

None outstanding. The `nucleo` dependency (needed by the UI work, not the core) is
**approved** *(user decision, 2026-07-25)*.
