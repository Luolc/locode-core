# ADR-0025: Agent Skills — shared discovery + a budgeted `<system-reminder>` listing

## Status
Accepted

## Date
2026-07-24

## Amends
- [ADR-0024](ADR-0024-locode-home-settings-and-traces.md) — §3 reserved the skill
  roots and deferred "frontmatter, the two-switch invocation gate, and injection"
  to this ADR; §1.2/§1.4's `extends` **changes from a settings-file pointer to a
  dotfolder pointer** (Decision §6). See the ADR-0024 amendments dated 2026-07-24.
- [ADR-0023](ADR-0023-fidelity-boundary-and-agents-md-loading.md) — §2's global
  instruction file gains one lower-precedence entry per extended dotfolder
  (Decision §6). See the ADR-0023 amendment dated 2026-07-24.
- [ADR-0008](ADR-0008-dispatch-door-and-path-jail.md) — §4.1 designed a read-only
  jail exception for the locode home. It is **deferred, not implemented**: the same
  day's later ADR-0008 amendment makes unrestricted the default, so the exception
  buys nothing until `--restricted` becomes a real mode.

Supersedes the *Recommendation* and *Open questions* sections of
[`docs/research/harness-study-skills.md`](../research/harness-study-skills.md).
That study also carried one factual error about Grok Build, corrected in
Decision §4 and in the study itself.

## Context

Skills are the last capability of the "reproduce four harnesses" workstream and
the first one we intend to **use daily**, not merely measure. All four surveyed
harnesses converged on the same on-disk contract — a per-skill directory holding
`SKILL.md` with YAML frontmatter, advertised to the model as a cheap
**name + description listing**, with the (potentially large) body loaded only
when a skill is actually chosen. That progressive-disclosure split is unanimous
and load-bearing; the harnesses diverge only on **how the body reaches the
model**.

Two prior decisions already fixed part of the answer, so this ADR inherits rather
than re-litigates them:

- **ADR-0024 §3** fixed the roots: `~/.locode/skills/<name>/SKILL.md` (user),
  `<repo>/.agents/skills/<name>/SKILL.md` (project — moved out of `.locode/` by the
  §2 amendment below), plus manual `skills.extra`
  entries from settings (§1.4). Explicitly **no `~/.claude/skills` compat root** —
  provenance over convenience. The settings loader already parses and validates
  `skills.extra` into `SkillsExtraEntry`.
- **ADR-0023 §1** fixed the fidelity boundary (a pack is *tools + prompt*;
  loop-adjacent machinery is shared) and **§3** fixed the role for injected
  framing: `User`-role content carrying `<system-reminder>`, never `Developer`.

What was left open — and what this ADR decides — is the on-disk frontmatter
contract, the listing's shape, budget and update rule, how a skill body reaches
the model, and what has to be reachable for that to work.

This ADR also **closes the ported-harness workstream** (user decision, below):
after skills, `claude` / `codex` / `grok` are done, the `opencode` pack is
cancelled, and every subsequent capability (background tasks, todo, subagents, …)
is built once, on our own `locode` best-of pack.

Empirical grounding is a fresh source read plus a live wire probe of the shipped
binaries (both 2026-07-24), cited below as `harness: path:line` and recorded in
the study's *Live wire probe* section.

## Decision

### 1. Placement — one shared implementation, no new tool

Skills are **entirely shared engine machinery**, exactly as ADR-0023 §1 says: one
loader, one listing, one update rule, identical under `--harness claude`, `codex`
and `grok`. **No pack gains a new tool.** The three ported packs keep the toolsets
they ship today, byte for byte.

The loader lives in **`locode-host`**, like the `AGENTS.md` loader: it is the
trusted OS seam, and skills legitimately live *outside* the tool path-jail
(`~/.locode/skills`, `skills.extra` paths), which a jailed read would reject
(ADR-0023's Implementation note makes the same call for the same reason). It
produces a neutral `Vec<Skill>`; the engine renders and injects the listing.

That skills need no tool is the finding of §4: two of the three surveyed CLIs
work exactly this way, and the third's tool buys a property we can get more
cheaply.

### 2. The on-disk contract

**Unit.** A skill is a directory containing `SKILL.md`. A bare `*.md` file under a
skills root is **not** a skill (Claude's rule — `claude-code:
loadSkillsDir.ts:424-428`); the directory is what lets a skill ship
`scripts/`/`references/` alongside its instructions.

**Roots and precedence**, highest first:

1. **project** — `<repo>/.agents/skills/` *(amended 2026-07-24 — see below)*
2. **user** — `~/.locode/skills/` (`$LOCODE_HOME` honored, per ADR-0024)
3. **user, via `extends`** — `<extended dotfolder>/skills/`, in `extends` list order (§6)
4. **extra** — `skills.extra` entries from settings, in list order

1–3 mirror the settings layers 1:1 (ADR-0024 §3, and §1.2's user-beats-extends
ordering). Tiers 2 and 3 are distinct in *precedence* but share the **`user`**
qualifier for collisions (below), because an extended dotfolder is part of how the
user composes their own configuration, not a separate authority. Project wins because it is the most specific to the work at hand — the
same root→cwd "deepest wins" rule ADR-0023 uses for instructions, and grok's own
ordering (cwd → repo → home, `08-skills.md:15-35`). `extra` sits last because it
is a manually-pointed grab-bag, not a tier of anything.

> **Amendment (2026-07-24): project skills live in `<repo>/.agents/skills`, not
> `<repo>/.agents/skills`.** *(User decision.)* This is the one project-scoped path we
> do **not** put under `.locode/`, and the asymmetry is the point: a skill is a
> **portable artifact** — `SKILL.md` with `name`/`description` frontmatter is the one
> thing all four surveyed harnesses independently converged on — so a repo's skills
> should be findable by whichever agent a contributor happens to run. Settings are
> ours alone and stay under `.locode/`.
>
> `.agents/` is the established cross-agent location, verified in source rather than
> assumed: **codex** scans `<root>/.agents/skills` through its own `AGENTS_DIR_NAME` +
> `SKILLS_DIR_NAME` constants (`core-skills/src/loader_tests.rs:119,2217`), and
> **grok** hard-codes `vec![".grok", ".agents"]` as its always-scanned config roots,
> with `.claude` merely opt-in compat (`discovery.rs:829-854`). The live wire probe
> corroborates it: grok's listing carried a skill resolved from
> `~/.agents/skills/find-skills/SKILL.md`.
>
> Only the **project** root moves. `~/.locode/skills` (user), each `extends`
> dotfolder's `skills/`, and `skills.extra` are unchanged — those are locode's own
> configuration home, not a shared repo surface. This does **not** reopen the rejected
> `~/.claude/skills` compat root (ADR-0024 §3): `.agents/` is a neutral convention, not
> another vendor's tree.

**Name identity.** The name is the frontmatter `name`, **normalized to a slug**
(lowercase; every character outside `[a-z0-9]` → `-`; consecutive hyphens
collapsed; leading/trailing hyphens trimmed; ≤ 64 chars), falling back to the
**directory name** when frontmatter has no `name`. This is grok's rule verbatim
(`normalize_skill_name`, `discovery.rs:333-348`; `MAX_NAME_LEN`, `:16`). We
deliberately reject Claude's "directory name is identity, frontmatter `name` is
display-only" (`loadSkillsDir.ts:452,238`): two names for one thing is a
documented foot-gun, and the slug rule keeps a human-written `name: Review PR`
usable (`review-pr`) instead of dropping the skill.

**Collisions.** There are **three** qualifier scopes, not four *(user decision)*:
`project:`, `user:` and `extra:`. An `extends` dotfolder's skills carry the **`user`**
qualifier — `extends` is a way of composing the user's own configuration, not a
separate authority, exactly as its settings merge into the user side of the layer
stack (ADR-0024 §1.2).

So the rule is:

- **Same name, same qualifier** (user vs. an extended dotfolder) → the
  higher-precedence one wins and the other is **dropped**; there is no way to
  address the shadowed one, and none is needed — that is what precedence means.
- **Same name, different qualifier** → both are kept and addressable as
  `<scope>:<name>` (`project:commit` vs `user:commit`), grok's `format_skill_name`
  (`skill.rs`). The listing renders the qualified form whenever a short name is
  ambiguous.

**Frontmatter — exactly five recognized keys** *(user decision)*:

| Key | Effect |
|---|---|
| `name` | Identity (slug-normalized; falls back to the directory name). |
| `description` | The router. Shown in the listing; this is what the model matches a task against. |
| `when-to-use` | Optional trigger phrasing, shown as a separate `Use when:` line. |
| `disable-model-invocation` | `true` → excluded from the listing, so the model never learns the skill exists. |
| `user-invocable` | Parsed and carried; **no observable effect in v1** (see below). |

Everything else — `allowed-tools`, `model`, `effort`, `paths`, `argument-hint`,
`license`, `metadata`, … — is **not parsed and not honored**. Each of the three
interesting ones is refused for its own reason, and all three land in the same
place: a file on disk in an **attacker-controlled layer** (ADR-0024 §1.3 — a
cloned repo ships `.locode/`) must not reconfigure the run.

- **`allowed-tools`** — we have no permission-rules system yet (ADR-0024 lists
  `permissions` as reserved), so there is no "narrow the toolset" semantics to
  attach it to; the only implementable meaning today is *widening*, i.e. letting a
  cloned repo move our trust boundary. It can return, **narrowing-only**, with the
  permissions work.
- **`model` / `effort`** — model selection has no seam yet (the tracker's `/model`
  item is blocked on the ADR-0015 `ProviderRegistry` public surface, an ask-first
  change). Honoring it now would route around that seam from a config file.
- **`paths`** — glob-conditional activation. ADR-0023 §2 already rejected exactly
  this machinery for rules directories as "disproportionate for this core"; there
  is no reason to build it a second time under a different name.

**Parsing uses a real YAML crate**, as both reference harnesses do — codex runs
`serde_yaml::from_str::<SkillFrontmatter>` (`core-skills/src/loader.rs:747`) and grok
coerces from `serde_yaml::Value` (`skills/discovery.rs:150-176`). An earlier
implementation hand-rolled a scalar-only scanner on the theory that five scalar keys
need no parser; that was wrong in a way that loses skills, because real frontmatter
uses folded (`description: >`) and literal (`|`) block scalars, and a skill whose
description is lost cannot be routed to at all. We take `serde_yaml_ng`, the maintained
drop-in — upstream `serde_yaml` is archived and publishes as `0.9.34+deprecated`.

**Parsing is lenient**: unknown keys are ignored, not errors — a skill authored
for another harness must still load, and this matches ADR-0024 §1.5's
"unknown keys are preserved" posture for settings. This does **not** conflict with
the standing type-strict rule for tool arguments: that governs **model-supplied**
input, this governs a **user-authored** file. A `SKILL.md` whose frontmatter fails
to parse, or whose name is unusable, is **skipped with a stderr diagnostic** — it
never aborts the run and never reaches the model.

**The two-switch gate.** `disable-model-invocation` and `user-invocable` are the
two halves ADR-0024 §3 named. Only the first has a channel in v1: the listing.
`user-invocable: false` is parsed and recorded but has **no observable behavior**
until slash-command invocation exists (tracker: deferred pending a holistic
design pass). We record it now so the on-disk contract is stable, and we will not
describe it in `--help`/README as if it worked.

### 3. The listing — `User` `<system-reminder>`, full-body diff, rescanned after each run

**Surface and role.** One `User`-role message wrapping the catalog in
`<system-reminder>…</system-reminder>` — ADR-0023 §3's rule, and the shape both
Claude Code (`messages.ts:3728-3738`) and grok (`listing.rs`) put on the wire
(confirmed on both wires by the live probe). The system prompt is **never**
mutated for skills (mutating it mid-session busts the provider prompt cache — the
same reason ADR-0023 rejected opencode's approach).

**The rendered message, verbatim.** The header and per-entry shape are grok's
(`listing.rs`, `SkillEntry::format` + `listing_header`):

```text
<system-reminder>
The following skills are available for use:

- <name>: <description>
  Use when: <when-to-use>
  Absolute path: <absolute path to SKILL.md>
- <name>: <description>
  Absolute path: <absolute path to SKILL.md>
</system-reminder>
```

The `  Use when:` line is **omitted entirely** when a skill has no `when-to-use`
(second entry above); the two continuation lines are indented two spaces. Entries
are ordered by scope then name, and a name that is ambiguous across scopes is
rendered qualified (`project:commit`, §2).

The **absolute path** is not decoration — it is the whole invocation mechanism
(§4). grok's header, which names no tool, is therefore exactly right for us: there
is no tool to name.

**Budget and degrade** — grok's numbers verbatim: a char budget of **50 % of the
context window** (falling back to 400 000 chars when the window is unknown), a
per-entry **400-byte** cap on description + `when-to-use` combined (split
proportionally, with a 20-char floor), and the three-tier degrade every harness
implements: full → shortened descriptions → names-only with an
`... and N more skills in <dir>` overflow marker. We take grok's 50 % rather than
Claude's 1 % or a freshly-invented conservative number because this is a **cap,
not a reservation**: at any realistic skill count it never binds, so a tighter
number can only ever silently truncate the text that does the routing, while
saving nothing in the common case. A configurable knob is a reserved `skills.*`
settings sibling (ADR-0024 §1.4 shaped `skills` as an object for exactly this),
not a v1 field.

**Zero skills → no listing message at all**, and no other trace: a run with no
skills on disk is byte-identical to today's.

#### 3.1 Update rule — whole-body comparison, never a per-skill delta

The unit of comparison is **the entire rendered listing body**, exactly as codex
compares its `<skills_instructions>` world-state section
(`ext/skills/src/world_state.rs`: the snapshot is `{body, includeInstructions}`,
and `render_diff` returns `None` when the previous body is byte-identical). If the
body is unchanged, **nothing is injected**. If it changed, the **whole listing is
re-sent** — all skills, not just the new ones.

Concretely: a session that started with skills `A` and `B` and gains `C`
mid-session re-sends **A, B and C** in one message. It never sends "`- C: …`"
alone.

We reject the per-skill delta that Claude Code and grok both implement
(Claude's `sentSkillNames` filter, `attachments.ts:2719-2723`; grok's
`announced.insert(...)` inside `format_announcement`'s filter chain) for two
concrete defects, both visible in their source:

- **A partial list under a total header is misleading.** Both harnesses emit the
  delta under the same "The following skills are available for use…" header, so a
  mid-session announcement of one skill reads as "there is now exactly one skill."
  They are relying on the model to union several partial listings.
- **Truncated-but-announced is unrecoverable.** In both, the announced key is
  inserted **before** budget truncation — grok says so in a comment on
  `render_listing`: *"Keys are inserted before budget truncation, so `announced`
  tracks skills that qualified, not skills whose text survived."* A skill dropped
  by the names-only tier is therefore marked announced and **never described
  again**. Comparing whole bodies has no per-skill ledger, so the bug cannot exist.

**Removal is stated, not silent.** When the last skill disappears (all deleted, or
all `disable-model-invocation`), we emit an explicit notice rather than simply
falling quiet:

```text
<system-reminder>
No skills are currently available.
</system-reminder>
```

Codex is the only surveyed harness that does this (`NO_EXECUTOR_SKILLS_BODY` /
`NO_HOST_SKILLS_BODY`, worded "`## Skills update` / No … skills are currently
available."). Claude Code and grok both go silent — grok explicitly rejected a
removal footer in a `take_pending` comment ("*it wastes tokens and looks like
skills with no description*"). We take codex's side: the model was told these
skills exist, and leaving that standing after they are gone is a stale
instruction. Codex's guard applies too — if there were never any skills, we say
nothing.

**The previous state is the transcript, not a side ledger.** Whether a listing
counts as "already delivered" is decided by **looking for its marker in the
conversation actually being sent**, the way codex resolves
`PreviousSectionState` (`world_state/mod.rs:342-362`: if
`has_retained_fragment_matcher()` and the fragment is not found in `items`, the
previous state is `Absent` and the section re-renders). Two properties fall out
for free, which the other two harnesses each had to hand-manage:

- **Compaction self-heals.** If a future compaction step drops the listing message,
  the next turn finds no marker and re-injects. No compaction hook is needed —
  contrast Claude Code, which deliberately does **not** re-inject post-compact
  (`compact.ts:524`, `:922`, `postCompactCleanup.ts:65` — "*~4K tokens … marginal
  benefit*"), and grok, which manually clears `announced_names` in
  `on_skill_discovery_compaction()`.
- **Resume is correct by construction.** A resumed session replays the transcript,
  the marker is present, the body matches → nothing is re-sent. No process-local
  latch (Claude's `suppressNextSkillListing`) and no persisted announced-set
  (grok's `restore_announced_skill_names`).

#### 3.2 When the scan runs — after the run finishes, off the user's critical path

*(User decision.)* The skills roots are rescanned **immediately after a run reaches
its terminal state and the UI has rendered** — not at the top of the next user
turn. The freshly rendered body is compared and stored then, so the next turn
injects a value that is already computed.

The reason is latency placement, not correctness: after a reply lands the user is
reading it, so several seconds of filesystem work is invisible; doing the same work
when the next prompt arrives puts it directly on the user's critical path and
reads as a stall. The **first** scan of a session is the one exception — there is
no preceding turn, so it runs synchronously at session start, before the first
sample. A headless one-shot therefore scans exactly once.

We deliberately do **not** adopt a filesystem watcher. Claude Code runs chokidar
over the skill roots (`skillChangeDetector.ts`, `depth: 2`, 1 s `awaitWriteFinish`,
300 ms debounce) and codex runs one in **app-server only** — `codex-file-watcher`
appears in no other crate's manifest, and `SkillsWatcher::new` is constructed once,
in `app-server/src/message_processor.rs:312`, so `codex` TUI and `codex exec` never
watch at all and a skill added mid-session is invisible to them until restart
(`SkillsService` caches by cwd and by config, invalidated only by `clear_cache()`).
A watcher would mean a new dependency — an ask-first item under AGENTS.md — to buy
what a bounded rescan already gives us. ADR-0023 made the same call for
`AGENTS.md`, for the same reason: the walk is small and cheap, so rescan-and-diff
beats watch-and-invalidate.

Net effect: a skill written **while a run is in flight** is usable on the very next
turn, with no restart.

> **Amendment (2026-07-24, implementation): the one-turn lag this timing implies.**
> The scan runs at the *end* of a run, so a skill created **after** that scan — while
> the user is typing their next prompt — is not seen by the turn they are about to
> send. The next run's post-run scan picks it up, and it appears on the turn after.
> This is inherent to scanning post-run rather than per-turn, and it is the accepted
> cost of keeping filesystem work off the user's critical path; the case the design
> optimizes for (the agent or the user writes a skill *during* a run) has no lag at
> all. Pinned by a test so it cannot drift silently into "sometimes one turn, sometimes
> two". Among the surveyed harnesses only Claude Code manages that in an ordinary
CLI session; grok notices a new skill only if a tool call happens to touch its
directory (it has no watcher at all), and codex's CLI does not notice it.

### 4. Invocation — the model reads `SKILL.md` itself

*(User decision.)* There is **no `Skill` tool**. The listing gives the model a
name, a description, and an **absolute path**; when a task matches, the model
opens that path with the pack's ordinary read tool and follows what it finds.

This is the majority behavior among the shipped CLIs, not a shortcut:

- **Grok Build has no skill tool at all** — see the correction below. Its listing
  plus the absolute path *is* the mechanism.
- **Codex's default path is the same**, and states it outright in the prompt text
  it ships (`render.rs:30-46`): *"After deciding to use a skill, the main agent must
  read its `SKILL.md` completely before taking task actions … The main agent must
  read each required instruction or reference file itself before acting."*
- **Only Claude Code has a real tool**, and the live probe caught its exact shape
  (`{skill: string, args?: string}`, `skill` required). Reproducing it faithfully
  means reproducing its two-step delivery: the `tool_result` is only
  `"Launching skill: <name>"` and the body arrives as a **separate injected user
  message** (`SkillTool.ts:634-774`, `:856-861`) — a second injection path in the
  loop, and a turn where one `tool_use` yields both a `tool_result` and an extra
  message.

What a tool buys is a durable `tool_use`/`tool_result` record of which skill fired.
What it costs here is a tool no ported pack legitimately owns: grok and codex do
not have one, so adding it to their packs breaks the ADR-0023 fidelity boundary,
and putting Claude's in the claude pack alone would make skills work under one
harness only. Our own `locode` pack is where a first-class skill tool belongs, and
it does not exist yet. Until then, reading the file is the honest mechanism — and
the read itself still leaves a `tool_use`/`tool_result` pair naming the path, so
the trace is not silent about it either.

> **Correction of record — Grok Build has no `Skill` tool, in source *or* on the
> wire.** The skills study described the `<skill name description path>` envelope
> as grok's *live* `Skill` tool. It is not, and this was confirmed twice.
>
> *In source:* the grok-native skill tool was **deleted**
> (`implementations/skills/skill.rs:35-37` — "Old `SkillToolImpl` + `impl Tool`
> deleted", pointing at a `grok_build/skill/` directory absent from the published
> tree), and **no `grok_build` toolset registers any skill tool**
> (`config.rs:440-517`). The only registered one is `opencode::OpenCodeSkillTool`,
> used solely by `opencode_toolset()` (`registry/types.rs:707`, `config.rs:528`).
> Corroboration: the listing header function takes its tool-name parameter as
> `_tool_name` and hardcodes "The following skills are available for use:" — the
> no-tool phrasing, against Claude's "…for use with the Skill tool".
>
> *On the wire:* a live probe of **Grok Build 0.2.111** through a local recording
> proxy (2026-07-24, [`../research/harness-study-skills.md`](../research/harness-study-skills.md)
> § *Live wire probe*) captured its request payload: **26 tools, none of them a
> skill tool**, alongside a `<system-reminder>` skills listing in exactly the
> format §3 reproduces.
>
> This correction is why this ADR ships no tool. An earlier draft specified a
> grok-shaped `skill` tool on the strength of the study's claim; with the claim
> disproven, building it would have meant inventing a surface and attributing it
> to a harness that does not have it.

#### 4.1 Reachability — deferred

> **Amendment (2026-07-24, later the same day): this section is deferred, not
> implemented.** *(User decision.)* ADR-0008's later amendment the same day makes
> **unrestricted the default** until the permission rules land, so a normal run has
> no jail for skill reads to trip over and the exception below buys nothing today.
> It is retained as the recorded design for the moment `--restricted` becomes a
> real mode; **Task 32 does not implement it**, and until it does, skills under
> `--restricted` reach only the project-local `<repo>/.agents/skills`.

The design, for when it is needed:


Reading `SKILL.md` only works if the model is allowed to. Project skills sit under
the workspace root and are already reachable; **user, `extends` and `extra` skills
are not** — `~/.locode/skills/x/SKILL.md` is outside the root, so ADR-0008's jail
rejects it as an escape. Without an exception, §4 would work for project skills
only, which is not a feature.

The exception, as decided *(user decision)*:

- **The whole locode home is readable** — `$LOCODE_HOME`, else `~/.locode` — not
  merely the skill subdirectories, plus every **skill root contributed from
  outside it**: `extends` dotfolders and `skills.extra` entries.
- **Read only, everywhere.** No write, create, delete, or edit through this
  exception; those paths keep the unmodified jail and are rejected exactly as they
  are today. The relaxation is on the read path alone.

A narrower first draft admitted only the directories discovery actually returned,
so that `~/.locode/skills/commit/` became readable while `~/.locode/` did not. That
was raised together with its cost and **overruled**: the simpler rule is easier to
reason about and to explain, and read-only is judged a sufficient safeguard.

**What this means, stated plainly:** `~/.locode/sessions/` holds full JSONL
transcripts of previous runs in this and every other project. Under this decision
the model can read them — it will not stumble in (nothing advertises those paths;
the listing names only `SKILL.md` files), but a prompt that asks for them will
succeed. That is the accepted trade, and it is bounded by read-only: no run can
rewrite or delete another run's history. Note also that this is moot in the mode
the author usually runs — `--dangerously-skip-permissions` already lifts the jail
entirely (ADR-0008 amendment 2026-07-18); the exception exists so that skills also
work in a *jailed* session.

The coherence argument stands behind all of it: the listing hands the model these
exact paths and says they are available. Advertising a path and then refusing to
read it is a contradiction that would surface as a confusing escape error on the
happy path.

### 5. Safety — skill text is semi-trusted

A `SKILL.md` can arrive with a cloned repo, or from an `extends` dotfolder someone
else maintains. Not injecting bodies removes most of the attack surface — nothing
from a skill file enters the conversation unless the model chooses to read it, and
that read is an ordinary, jailed, approval-gated tool call. What remains:

- **No execution of skill text.** Claude Code expands inline `` !`cmd` `` in a
  skill body at load time (`loadSkillsDir.ts:374-396`); we do **not** implement
  that in any form, and with no body-expansion step there is nowhere for it to
  live. A skill that wants to run something instructs the model to run it through
  the normal Bash tool. A file on disk must not be a silent RCE.
- **The listing is data, not instructions.** Descriptions come from disk and are
  rendered into a `<system-reminder>`; they are advertising copy for a router, and
  the enclosing text says so. No skill text is presented as a user message.
- **Read-only reachability** (§4.1) is the only jail relaxation, and it is scoped
  to the skill directories themselves.

**No separate approval gate for reading a skill.** The read already passes the
ADR-0017 seam like any other read, and adding a second prompt for "may I read the
file you put there yourself" trains the user to click through prompts, which makes
the gate that matters weaker.

### 6. `extends` points at a whole dotfolder, not at a settings file

*(User decision.)* ADR-0024's `extends` currently accepts **settings files only** —
each entry "an ordinary settings JSON file merged between the user and project
layers" (§1.2 amendment). That was too narrow: the point of the field is to inherit
a shared *configuration home* — a team's `~/team-locode/` — and a home is more than
its JSON.

**An `extends` entry is now a locode dotfolder directory**, and all three of its
contributions merge:

| Path in the extended dotfolder | Merges as |
|---|---|
| `<dir>/settings.json` | a settings layer, exactly where `extends` sits today — between the user and project layers, in list order, non-recursive, §1.3 denylist applied |
| `<dir>/skills/` | a skill root at precedence tier 3 (§2), below `user`, above `extra` — mirroring where its settings sit |
| `<dir>/AGENTS.md` | an instruction entry **below** the global `~/.locode/AGENTS.md` (ADR-0023 §2), so ours wins on conflict and the repo chain wins over both |

Multiple entries apply in list order, each labeled with its `source_path` like
every other instruction entry. **A missing sub-path is simply absent, never an
error** — extending a dotfolder that ships only skills is a normal thing to do. A
missing *entry* keeps today's loud warning: the user pointed at it.

**This replaces the file form; it does not sit beside it.** Pointing `extends` at a
`settings.json` is now a **config error with an explicit message**, not a silently
reinterpreted path — ADR-0024 §1.5 permits a structural change but forbids silent
reinterpretation, and a file that used to be read as settings must not quietly
become "a dotfolder that happens to have no `skills/`". The blast radius is small:
the field landed in the same release cycle, the first-run scaffold writes
`extends: []`, and the error text names the fix.

#### 6.1 Load order is now an invariant, not an accident

*(User decision.)* Because `extends` lives **in** settings and decides **where else**
instructions and skills come from, startup has a required order:

1. Resolve the **user** `settings.json`.
2. Resolve its **`extends`** entries — the dotfolder list is complete after this
   one pass, since `extends` does not recurse (ADR-0024 §1.2).
3. Finish the layer stack (project, local, flag) and produce the merged settings.
4. **Then** load project instructions — the chain now includes each extended
   dotfolder's `AGENTS.md` (§6) and honors `instructions.root_stop_pattern`.
5. **Then** discover skills — the roots now include each extended dotfolder's
   `skills/` (§2 tier 3) and every `skills.extra` entry contributed by *any*
   settings layer, including the extended ones.

Steps 4 and 5 are consumers of resolved settings and must never run before step 3.
The dependency already existed in weaker form — `instructions.root_stop_pattern`
is a settings key (ADR-0024 §1.4) — but `extends` makes it structural: run
discovery too early and an extended dotfolder's skills and instructions are
silently missing, with no error to explain it. Any future settings key that
contributes a *root* inherits the same rule.

## Alternatives Considered

- **A grok-shaped `skill` tool returning the body as the tool result** (this ADR's
  own earlier draft). Rejected once the study's claim was checked: grok ships no
  such tool, so building it would invent a surface and attribute it to a harness
  that does not have it, and hosting it in the grok pack would break ADR-0023's
  fidelity boundary. The design's *merits* (one round-trip, a self-describing
  transcript) are real and belong to the future `locode` pack.
- **Claude Code's `Skill` tool, faithfully ported into the claude pack.** The only
  option where the tool reproduces something that exists. Rejected for now: it
  makes skills work under one harness only, and its two-step delivery needs a new
  engine path for a tool that emits both a `tool_result` and an extra injected
  message. Revisit when the `locode` pack lands.
- **Per-skill incremental announcement** (Claude Code and grok both do this).
  Rejected in §3.1 on two source-visible defects: a delta is emitted under the same
  "the following skills are available" header, so a one-skill announcement reads as
  a total list; and the announced key is recorded *before* budget truncation, so a
  skill dropped by the names-only tier is marked announced and never described
  again. Whole-body comparison has no ledger and therefore neither defect. It costs
  a re-send of the full listing whenever anything changes — 1–3 KB at realistic
  skill counts, and only on change.
- **A filesystem watcher** (Claude Code's chokidar; codex's `FileWatcher`).
  Rejected in §3.2: it needs a new dependency (ask-first) to buy what a bounded
  post-run rescan already gives, and ADR-0023 already chose rescan-over-watch for
  `AGENTS.md`. Worth recording what the watcher route actually buys the harnesses:
  Claude Code gets "write a skill, use it immediately"; codex's watcher lives in
  app-server only, so its own CLI does *not* get that.
- **Rescanning at the start of the next user turn** (the obvious placement, and
  where Claude Code's per-turn attachment pass effectively sits). Rejected *(user
  decision)*: it puts filesystem latency on the user's critical path. Scanning right
  after the run finishes hides the same work behind the user reading the reply.
- **Silence when the last skill disappears** (Claude Code and grok both go quiet;
  grok explicitly rejected a removal footer). Rejected in §3.1: an instruction the
  model was given should be withdrawn explicitly, not left standing.
- **Making all of `~/.locode` readable from the jail** (the simple version of
  §4.1). Rejected: `~/.locode/sessions/` holds full transcripts of previous runs.
  The exception is scoped to discovered skill directories.
- **A conservative listing budget (1–2 %) plus a config knob** (the study's
  recommendation). Rejected in §3: the budget is a cap that never binds at
  realistic skill counts, so tightening it can only truncate routing text.
- **`~/.claude/skills` vendor-compat root** (grok reads Claude's and Cursor's
  trees). Already rejected by ADR-0024 §3; unchanged here.
- **Keeping the `extends` file form beside the new directory form** (a dual shape,
  as `skills.extra` uses). Rejected in §6: two meanings for one field is exactly the
  silent reinterpretation ADR-0024 §1.5 forbids, and the field is new enough that a
  clear error costs less than a permanent ambiguity.

## Consequences

- **New shared capability, no new tool.** `locode-host` gains a skills loader
  alongside the instructions loader and an extra read-only root set for the jail;
  `locode-engine` gains listing rendering plus the whole-body diff (structurally
  the instructions hash-diff shipped in Task 30, with the comparison widened to the
  rendered body and the "already delivered" test moved onto the transcript). **No
  crate gains a tool, no pack changes, no `Pack`/`Tool` trait signature changes,
  and no new `ToolKind`.**
- **Skills work under every harness.** `--harness claude`, `codex` and `grok` all
  discover, list and read skills identically. The default harness is unchanged
  (`claude`) — an earlier draft flipped it to `grok` only because the tool was to
  live there; with no tool, the reason is gone.
- **No jail change ships** — §4.1's read-only exception is deferred behind
  ADR-0008's default flip. Under `--restricted`, skills reach only
  `<repo>/.agents/skills`; that limitation is the accepted cost of not building an
  exception no one currently needs.
- **A new engine seam: post-run work.** §3.2 needs a hook that fires after a run
  reaches its terminal state, so the rescan lands off the user's critical path.
  Nothing like it exists today (the loop ends and returns), and the TUI must invoke
  it *after* its final render.
- **`extends` changes meaning** (§6): a settings-file pointer becomes a dotfolder
  pointer, and with it the first mechanism by which an external directory
  contributes settings, skills *and* project instructions at once. The settings
  merge already ships and keeps its position in the layer stack; the directory
  resolution, the skills root and the `AGENTS.md` entry land with this task. A
  file-valued entry becomes a config error with an explicit message.
- **The ported-harness workstream closes.** `opencode` (Task 15's faithful half)
  is **cancelled**, not deferred; its plan document stays as a record. Everything
  after this — background tasks, todo, subagents — targets the `locode` pack only.
- **Reconciled in this change** (ADR-first): amendments to ADR-0008 (jail
  exception), ADR-0023 (`extends` instructions), ADR-0024 (`extends` dotfolders;
  §3 skills resolved; the same-day default-harness amendment withdrawn), the skills
  study, the decisions index, and the tracker.

## Resolved (user decisions, 2026-07-24)

- **No skill tool** — the listing carries absolute paths and the model reads
  `SKILL.md` with the pack's ordinary read tool. A first-class tool waits for the
  `locode` pack.
- **Exactly five frontmatter keys** — `name`, `description`, `when-to-use`,
  `disable-model-invocation`, `user-invocable`. `allowed-tools` / `model` /
  `effort` / `paths` are not parsed, which removes the permission question
  entirely.
- **Listing rides the `User` `<system-reminder>`** channel in grok's verbatim
  format; codex's `developer` + `<skills_instructions>` shape is not used.
- **Whole-body comparison, never a per-skill delta** — if the rendered listing
  changed at all, the entire listing is re-sent; two skills becoming three re-sends
  all three (§3.1).
- **The rescan runs after the run finishes and the UI has rendered**, never at the
  start of the next user turn (§3.2). Session start is the one synchronous
  exception. No filesystem watcher.
- **`extends` points at a dotfolder directory**, inheriting its `settings.json`,
  `skills/` and `AGENTS.md` (§6). This *replaces* the settings-file form; a
  file-valued entry is now a config error.
- **The ported-harness reproduction workstream ends with this task**; the
  `opencode` pack is cancelled and all later capability goes to the `locode` pack.

## Open Questions

- **The read-only locode-home exception** (§4.1) is designed and deferred. It
  becomes live work when the permission rules land and `--restricted` stops being a
  preview; the cost recorded there (past-session transcripts readable) is what a
  reviewer should weigh at that point, not now.
- **`--bare`.** Referenced as the switch that turns skills (and, later, subagents)
  off wholesale for clean A/B runs. It does not exist yet; when it lands it must
  disable discovery, the listing and the §4.1 exception together, and it subsumes
  `--no-project-instructions`.
- **Slash invocation.** `user-invocable` stays inert until the deferred
  slash-command design pass; that pass should also decide whether user-invoked
  skills splice the body in at prompt-assembly time (grok's
  `build_skill_information`) rather than making the model read it.
