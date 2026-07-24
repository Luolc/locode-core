# ADR-0025: Agent Skills — shared discovery + a grok-pack `skill` tool

## Status
Accepted

## Date
2026-07-24

## Amends
- [ADR-0023](ADR-0023-fidelity-boundary-and-agents-md-loading.md) — its §1 lists
  "skills discovery and listing/body injection" as shared engine machinery. That
  holds for **discovery and the listing**; this ADR records that the **body
  envelope and its delivery are the `skill` tool's surface**, and the tool is a
  pack tool (grok only). See the ADR-0023 amendment dated 2026-07-24.
- [ADR-0024](ADR-0024-locode-home-settings-and-traces.md) — §3 reserved the skill
  roots and deferred "frontmatter, the two-switch invocation gate, and injection"
  to this ADR; §1.4's scaffold default `harness` flips from `claude` to `grok`
  (Decision §6). See the ADR-0024 amendment dated 2026-07-24.

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
  `<repo>/.locode/skills/<name>/SKILL.md` (project), plus manual `skills.extra`
  entries from settings (§1.4). Explicitly **no `~/.claude/skills` compat root** —
  provenance over convenience. The settings loader already parses and validates
  `skills.extra` into `SkillsExtraEntry`.
- **ADR-0023 §1** fixed the fidelity boundary (a pack is *tools + prompt*;
  loop-adjacent machinery is shared) and **§3** fixed the role for injected
  framing: `User`-role content carrying `<system-reminder>`, never `Developer`.

What was left open — and what this ADR decides — is the on-disk frontmatter
contract, the listing's shape and budget, the tool's shape and body envelope,
which packs carry it, and the safety rules around executing semi-trusted text
found on disk.

This ADR also **closes the ported-harness workstream** (user decision, below):
after skills, `claude` / `codex` / `grok` are done, the `opencode` pack is
cancelled, and every subsequent capability (background tasks, todo, subagents, …)
is built once, on our own `locode` best-of pack.

Empirical grounding is a fresh source read (2026-07-24) against the
`coding-cli-survey` submodules, cited below as `harness: path:line`.

## Decision

### 1. Placement — one shared loader, one pack-owned tool

**Discovery and the listing are shared engine machinery** (ADR-0023 §1). The
loader lives in **`locode-host`**, exactly like the `AGENTS.md` loader: it is the
trusted OS seam, and skills legitimately live *outside* the tool path-jail
(`~/.locode/skills`, `skills.extra` paths), which a jailed read would reject
(ADR-0023's Implementation note makes the same call for the same reason). It
produces a neutral `Vec<Skill>`; the engine renders and injects the listing.

**The `skill` tool is a pack tool, registered only by the grok pack.** Skills are
a capability we want daily, not a fidelity surface we are measuring; concentrating
the tool in one pack keeps the other two packs' toolsets untouched and matches how
we will use the thing (Decision §6 makes `grok` the default harness for exactly
this reason). `claude` and `codex` packs get **no `skill` tool and no listing** —
under those harnesses skills simply do not exist for now.

The engine decides whether to inject a listing by asking the registry whether a
tool of the new kind **`ToolKind::Skill`** is registered — not by a new `Pack`
method. `ToolKind` was designed to grow this way ("we start small and grow the
canonical set as packs need it", `locode-tools/src/tool.rs`), so this is the
additive path and leaves the `Pack` trait signature untouched.

### 2. The on-disk contract

**Unit.** A skill is a directory containing `SKILL.md`. A bare `*.md` file under a
skills root is **not** a skill (Claude's rule — `claude-code:
loadSkillsDir.ts:424-428`); the directory is what lets a skill ship
`scripts/`/`references/` alongside its instructions.

**Roots and precedence**, highest first:

1. **project** — `<repo>/.locode/skills/`
2. **user** — `~/.locode/skills/` (`$LOCODE_HOME` honored, per ADR-0024)
3. **extra** — `skills.extra` entries from settings, in list order

Project wins because it is the most specific to the work at hand — the same
root→cwd "deepest wins" rule ADR-0023 uses for instructions, and grok's own
ordering (cwd → repo → home, `08-skills.md:15-35`). `extra` sits last because it
is a manually-pointed grab-bag, not a tier of the project.

**Name identity.** The name is the frontmatter `name`, **normalized to a slug**
(lowercase; every character outside `[a-z0-9]` → `-`; consecutive hyphens
collapsed; leading/trailing hyphens trimmed; ≤ 64 chars), falling back to the
**directory name** when frontmatter has no `name`. This is grok's rule verbatim
(`normalize_skill_name`, `discovery.rs:333-348`; `MAX_NAME_LEN`, `:16`). We
deliberately reject Claude's "directory name is identity, frontmatter `name` is
display-only" (`loadSkillsDir.ts:452,238`): two names for one thing is a
documented foot-gun, and the slug rule keeps a human-written `name: Review PR`
usable (`review-pr`) instead of dropping the skill.

**Collisions.** Same name in two scopes → both are kept, addressable as
`<scope>:<name>` (`project:commit`, `user:commit`, `extra:commit`) — grok's
`format_skill_name` (`skill.rs`). The listing shows the qualified name whenever a
short name is ambiguous; an ambiguous short name passed to the tool returns the
qualified candidates rather than silently picking first-match (grok's
`FindSkillResult::Ambiguous`).

**Frontmatter — exactly five recognized keys** *(user decision)*:

| Key | Effect |
|---|---|
| `name` | Identity (slug-normalized; falls back to the directory name). |
| `description` | The router. Shown in the listing; this is what automatic invocation keys off. |
| `when-to-use` | Optional trigger phrasing, shown as a separate `Use when:` line. |
| `disable-model-invocation` | `true` → excluded from the listing, and the tool refuses it. |
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

**Parsing is lenient**: unknown keys are ignored, not errors — a skill authored
for another harness must still load, and this matches ADR-0024 §1.5's
"unknown keys are preserved" posture for settings. This does **not** conflict with
the standing type-strict rule for tool arguments: that governs **model-supplied**
input, this governs a **user-authored** file. A `SKILL.md` whose frontmatter fails
to parse, or whose name is unusable, is **skipped with a stderr diagnostic** — it
never aborts the run and never reaches the model.

**The two-switch gate.** `disable-model-invocation` and `user-invocable` are the
two halves ADR-0024 §3 named. Only the first has a channel in v1: the model's.
`user-invocable: false` is parsed and recorded but has **no observable behavior**
until slash-command invocation exists (tracker: deferred pending a holistic
design pass). We record it now so the on-disk contract is stable, and we will not
describe it in `--help`/README as if it worked.

### 3. The listing — `User` `<system-reminder>`, full-body diff, rescanned after each run

**Surface and role.** One `User`-role message wrapping the catalog in
`<system-reminder>…</system-reminder>` — ADR-0023 §3's rule, and the shape both
Claude Code (`messages.ts:3728-3738`) and grok (`listing.rs`) put on the wire. The
system prompt is **never** mutated for skills (mutating it mid-session busts the
provider prompt cache — the same reason ADR-0023 rejected opencode's approach).

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
are ordered by scope then name (project → user → extra), and a name that is
ambiguous across scopes is rendered qualified (`project:commit`, §2). The
**absolute path** is carried because a skill's real payload is often its sibling
files (`scripts/`, `references/`) and the model needs the base directory to reach
them — and because it keeps the listing useful even if a future pack ships no
`skill` tool. We keep grok's header (which does **not** name a tool) rather than
Claude's "…for use with the Skill tool:" — it stays true under every pack, and the
tool's own description already says how to load a skill.

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
skills exist, and leaving that standing after they are gone is a stale instruction.
Codex's guard applies too — if there were never any skills, we say nothing.

**The previous state is the transcript, not a side ledger.** Whether a listing
counts as "already delivered" is decided by **looking for its marker in the
conversation actually being sent**, the way codex resolves
`PreviousSectionState` (`world_state/mod.rs:342-362`: if
`has_retained_fragment_matcher()` and the fragment is not found in `items`, the
previous state is `Absent` and the section re-renders). Two properties fall out for
free, which the other two harnesses each had to hand-manage:

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

Net effect: a skill written mid-session is usable on the very next turn, with no
restart. Among the surveyed harnesses only Claude Code manages that in an ordinary
CLI session; grok notices a new skill only if a tool call happens to touch its
directory (it has no watcher at all), and codex's CLI does not notice it.

**Zero skills → no listing message at all.** The tool is still registered (below).

### 4. The `skill` tool — one call, body as the tool result

**Shape.** Registered under the wire name **`skill`** — lower-case, matching both
the grok pack's snake_case tools (`read_file`, `list_dir`, `search_replace`) and
grok's own kind→name mapping (`xai-grok-agent/src/prompt/template.rs:105`,
`(ToolKind::Skill, "skill")`). Input `{ skill: string, args?: string }` (grok's
`SkillInput`) → the tool result **is** the skill body, wrapped in grok's canonical
envelope:

```
<skill name="…" description="…" path="…">
{body}
</skill>
```

The envelope is not decoration: it gives the model an explicit identity and
boundary so it reads the contents as *instructions to follow*, and knows exactly
where semi-trusted text starts and stops (grok's own rationale, `skill.rs:46-55`).

**Why body-as-tool-result, and not Claude's two-step** *(user decision)*. Claude
Code's `Skill` tool returns only `"Launching skill: <name>"` and delivers the body
as a **separate injected user message** in the same turn (`SkillTool.ts:634-774`,
`:856-861`). That buys a distinction we do not need and costs machinery we would
have to build: a second injection path in the loop, and a turn where one
`tool_use` produces both a `tool_result` and an extra message. Returning the body
as the tool result keeps the pairing invariant trivially intact (one `tool_use` →
exactly one `tool_result`, ADR-0004/0008), reuses the one dispatch door, and makes
the transcript self-describing — the body is *in* the tool result, so a resumed or
replayed session needs no special case.

> **Correction of record — Grok Build has no `Skill` tool, in source *or* on the
> wire.** The skills study described this envelope as grok's *live* `Skill` tool.
> It is not, and this was confirmed twice.
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
> format §3 reproduces. So grok's only routes are that listing (whose absolute
> paths let the model read `SKILL.md` itself) and user-invoked prompt-assembly
> injection.
>
> What survives in grok is the **formatter** (`build_skill_message`,
> `skill.rs:39-64`), still used by slash expansion, the pager, and agent
> preloading. We adopt that format knowingly, **as a format** — the tool built
> here is our own design, not a reproduction of a live grok surface.

**Argument substitution**, applied to the body before wrapping — grok's set minus
the vendor aliases: `$ARGUMENTS` (whole string), `$ARGUMENTS[N]` and `$N`
(0-indexed, whitespace-split), `${SKILL_DIR}` (the directory holding `SKILL.md`),
`${SESSION_ID}`. When `args` is given but the body contains no argument token, the
arguments are appended as an `**ARGUMENTS:** …` suffix rather than dropped
(`apply_substitutions`). We do **not** ship the `${CLAUDE_SKILL_DIR}` /
`${CLAUDE_SESSION_ID}` aliases: they exist in grok to ingest skills written for
another vendor, which ADR-0024 §3 already declined ("provenance over
convenience").

**Failure modes** are ordinary `ToolError`s, never a silent empty result: unknown
name (with the closest available names listed), ambiguous short name (with the
qualified candidates), a skill marked `disable-model-invocation`, and an
unreadable/oversized body.

**Body size.** The body is capped at a generous byte budget with a **visible**
truncation marker; it is exempt from the ordinary tool-result truncation path,
because silently clipping instructions produces a skill that half-runs, which is
worse than one that says it was cut.

### 5. Safety — skill text is semi-trusted

A `SKILL.md` can arrive with a cloned repo. Three rules, all from the study's
cross-harness observations:

- **No execution of skill text.** Claude Code expands inline `` !`cmd` `` in a
  skill body at load time (`loadSkillsDir.ts:374-396`); we do **not** implement
  that in any form. A skill that wants to run something instructs the model to run
  it through the normal, already-gated Bash tool, with `${SKILL_DIR}` to locate
  its scripts. A file on disk must not be a silent RCE.
- **Path-traversal protection on link resolution.** Relative markdown links in a
  body are resolved to absolute paths for the model's convenience, but a link
  escaping the skill directory (`../../secret.md`) is **left untouched** rather
  than resolved (grok's rule and its regression test, `skill.rs:364-453`, test
  `:1269-1278`).
- **Envelope always.** The body is never spliced into the conversation bare; it
  always arrives inside the `<skill …>` envelope of §4.

**No approval gate on invocation.** Loading instructions is not a side effect;
every side effect a skill causes happens through tools that are already behind the
ADR-0017 approval seam. Adding a second prompt for "may I read this file you put
there yourself" trains the user to click through prompts, which makes the gate
that matters weaker.

### 6. The default harness becomes `grok`

`--harness`'s durable default flips from `claude` back to **`grok`** *(user
decision)* — the scaffolded `settings.json` default and the built-in fallback,
both currently `"claude"` (`locode-host/src/settings.rs:237`; ADR-0024 §1.4
amendment 2026-07-24). `api_schema` (`anthropic`) and `model` (`claude-sonnet-5`)
are **unchanged**: the daily configuration is deliberately *grok's toolset on a
Claude model*, which is exactly the one-engine-one-wire-swap-the-surface usage
ADR-0012 is built for, and is legal because the grok pack is wire-agnostic (only
the codex pack pins a schema).

The reason is skills: the `skill` tool lives in the grok pack (§1), and the whole
point of building this now is to use it every day.

## Alternatives Considered

- **Claude's two-step delivery (ack + injected user message).** Rejected in §4:
  extra loop machinery and a turn shape our pairing invariant would have to make an
  exception for, buying a legibility distinction the enveloped tool result already
  provides.
- **Codex's model-reads-the-file (no tool at all).** Rejected: it needs zero new
  tool surface but leans entirely on the model choosing to read and re-read
  (`render.rs:30-46`), and it leaves no durable record of which skill fired. Our
  listing carries absolute paths anyway, so this remains available as a fallback
  behavior without being the design.
- **Register the `skill` tool only when ≥ 1 skill is discovered** (considered at
  length, and initially chosen). Rejected once the source was checked: **neither**
  harness gates tool registration on skill count — Claude's `SkillTool` sits
  unconditionally in the static tool array (`tools.ts:212`, unlike its neighbors
  which are conditionally spread), and gates only the *listing*, on the reverse
  test ("does this agent have the Skill tool", `attachments.ts:2668-2673`). A
  count-gated registration decided at session start means the **first** skill a
  user ever writes requires a restart; deciding it per turn instead busts the
  prompt-cache prefix on the turn it flips. Gating only the listing has neither
  problem, and A/B cleanliness — the gate's original motivation — is covered by the
  future `--bare` switch, which turns skills off wholesale.
- **A per-pack skill tool for every pack** (the study's original per-pack fidelity
  table). Rejected by ADR-0023 §1 and by the workstream closing: the ported packs
  are done, and further capability goes into the `locode` pack.
- **A conservative listing budget (1–2 %) plus a config knob** (the study's
  recommendation). Rejected in §3: the budget is a cap that never binds at
  realistic skill counts, so tightening it can only truncate routing text.
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
- **`~/.claude/skills` vendor-compat root** (grok reads Claude's and Cursor's
  trees). Already rejected by ADR-0024 §3; restated here because the substitution
  aliases raised it again.

## Consequences

- **New shared capability + one new pack tool.** `locode-host` gains a skills
  loader alongside the instructions loader; `locode-tools` gains
  `ToolKind::Skill`; `locode-engine` gains listing rendering plus the whole-body
  diff (structurally the instructions hash-diff shipped in Task 30, with the
  comparison widened to the rendered body and the "already delivered" test moved
  onto the transcript); `locode-packs`'s grok pack gains the `skill` tool. No
  `Pack` or `Tool` trait signature changes.
- **A new engine seam: post-run work.** §3.2 needs a hook that fires after a run
  reaches its terminal state, so the rescan lands off the user's critical path.
  Nothing like it exists today (the loop ends and returns), and the TUI must invoke
  it *after* its final render. It is small, but it is a genuine addition to the
  engine's lifecycle and the first consumer of a seam later work (prefetch,
  background summarization) will likely reuse.
- **`ToolKind` grows a variant.** Additive by design, but it is a public enum: any
  exhaustive match downstream must add an arm.
- **A user-visible default changes.** `--harness` defaults to `grok`; the settings
  scaffold, the built-in fallback, `README.md`, and the `--help` text must move
  together, and ADR-0024 §1.4 gets a dated amendment (done in this change).
- **Skills are grok-only for now.** Under `--harness claude` or `codex`, a
  `SKILL.md` on disk is inert and nothing is injected. This is deliberate and must
  be stated plainly in the README rather than left to be discovered.
- **The ported-harness workstream closes.** `opencode` (Task 15's faithful half)
  is **cancelled**, not deferred; its plan document stays as a record. Everything
  after this — background tasks, todo, subagents — targets the `locode` pack only.
- **Reconciled in this change** (ADR-first): the ADR-0023 amendment (§1's
  skills line), the ADR-0024 amendment (§1.4 default `harness`), the skills study
  (superseded recommendation + the grok correction), the decisions index, and the
  tracker (Task 32 pointed at this ADR; Task 15's opencode half cancelled).

## Resolved (user decisions, 2026-07-24)

- **One tool, grok-shaped, grok pack only** — body returned as the tool result
  inside `<skill name description path>`; Claude's ack + second-message delivery is
  **not** implemented.
- **Exactly five frontmatter keys** — `name`, `description`, `when-to-use`,
  `disable-model-invocation`, `user-invocable`. `allowed-tools` / `model` /
  `effort` / `paths` are not parsed, which removes the permission question
  entirely.
- **Default `--harness` → `grok`**, with `api_schema` / `model` untouched.
- **Listing rides the `User` `<system-reminder>`** channel; codex's
  `developer` + `<skills_instructions>` shape is not used.
- **The ported-harness reproduction workstream ends with this task**; the
  `opencode` pack is cancelled and all later capability goes to the `locode` pack.
- **Registration is not gated on skill count** — only the listing is (decided
  against the source, see Alternatives).
- **Whole-body comparison, never a per-skill delta** — if the rendered listing
  changed at all, the entire listing is re-sent; two skills becoming three re-sends
  all three (§3.1).
- **The rescan runs after the run finishes and the UI has rendered**, never at the
  start of the next user turn — latency hides behind the user reading the reply
  (§3.2). Session start is the one synchronous exception.
- **No filesystem watcher** — a bounded rescan instead, consistent with ADR-0023
  and avoiding an ask-first dependency (§3.2).

## Open Questions

- **`--bare`.** Referenced as the switch that turns skills (and, later, subagents)
  off wholesale for clean A/B runs. It does not exist yet; when it lands it must
  disable skill discovery, the listing, and tool registration together, and it
  subsumes `--no-project-instructions`. Tracked separately.
- **Slash invocation.** `user-invocable` stays inert until the deferred
  slash-command design pass; that pass should also decide whether user-invoked
  skills use grok's zero-round-trip prompt-assembly injection
  (`build_skill_information`) instead of a tool call.
- **`allowed-tools`, narrowing-only.** Revisit when the permissions work lands.
