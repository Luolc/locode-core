# Task 32 — Agent Skills: discovery, the `<system-reminder>` listing, and `extends` dotfolders

> Implements [ADR-0025](../../docs/decisions/ADR-0025-agent-skills.md) in full, plus the
> two things it changed elsewhere: `extends` becoming a dotfolder pointer (ADR-0024 §1.2
> amendment) and an extended dotfolder's `AGENTS.md` joining the instruction chain
> (ADR-0023 amendment). Source grounding:
> [`../../docs/research/harness-study-skills.md`](../../docs/research/harness-study-skills.md)
> — the four-harness study **plus its live wire probe**, which is what disproved the
> "grok has a Skill tool" claim this design originally rested on.
>
> Shared **engine machinery** (ADR-0023 §1): identical under every `--harness`, nothing
> pack-visible. **No pack gains a tool** and no pack's toolset changes by a byte.

## Objective

1. Skills on disk are discovered from four roots, parsed against a five-key frontmatter
   contract, and advertised to the model as one budgeted `User` `<system-reminder>`
   listing carrying each `SKILL.md`'s **absolute path**.
2. The model invokes a skill by **reading that path with the pack's ordinary read tool**
   — there is no skill tool, in any pack.
3. The listing is compared as a **whole body**: any change re-sends all of it, "already
   delivered" is decided by finding the marker in the transcript, and the rescan runs
   **after a run finishes**, off the user's critical path.
4. `extends` points at a **locode dotfolder**, contributing its `settings.json`,
   `skills/` and `AGENTS.md` — which makes settings resolution a hard prerequisite of
   both instruction loading and skills discovery.

## Design constraints (from the ADRs — not re-litigated here)

- **No tool, no `ToolKind`, no pack change.** Two of the three shipped CLIs work this
  way; only Claude Code has a tool, and it works under one harness only (ADR-0025 §4).
- **Frontmatter is exactly five keys** — `name`, `description`, `when-to-use`,
  `disable-model-invocation`, `user-invocable`. `allowed-tools`/`model`/`effort`/`paths`
  are not parsed at all, so there is no permission question to answer (§2).
- **Listing format is grok's, verbatim** — header `The following skills are available
  for use:`, entries `- <name>: <desc>` + optional `  Use when:` + `  Absolute path:`;
  50 % context-window char budget, 400-byte per-entry cap, three-tier degrade (§3).
- **Whole-body diff, never a per-skill delta** — both harnesses that do deltas carry two
  defects because of it (misleading partial header; announced-before-truncation), and
  comparing bodies has no ledger to get wrong (§3.1).
- **Removal is stated**, not silent: going to zero emits `No skills are currently
  available.` (§3.1).
- **New crate `locode-skills`**, sibling to `locode-instructions` (ADR-0002 amendment
  2026-07-24). Not a shared "context" crate — the two features share only an envelope.
- **Deferred, deliberately:** the ADR-0008 read-only jail exception (ADR-0025 §4.1). With
  unrestricted the default (ADR-0008 amendment 2026-07-24) it buys nothing today; under
  `--restricted`, skills reach only `<repo>/.locode/skills` and that is accepted.

## Slices

### S1 — `extends` becomes a dotfolder, and load order becomes an invariant (M)

Prerequisite for everything else: until `extends` resolves to a directory, the skills
roots and the instruction chain cannot know what to include.

- `locode-host/settings.rs`: an `extends` entry resolves to a **directory**; its
  `settings.json` merges exactly where the file used to (between user and project, list
  order, non-recursive, denylist applied). A **file**-valued entry is a config error with
  a message naming the fix — never silently reinterpreted (ADR-0024 §1.5).
- `SettingsLoad` gains the resolved dotfolder list, so downstream consumers do not
  re-resolve it.
- `locode-instructions`: each extended dotfolder's `AGENTS.md`, when present, becomes an
  entry **below** the global `~/.locode/AGENTS.md`, labeled with its `source_path`.
- **Load order** (ADR-0025 §6.1): settings + `extends` resolve first; instructions and
  skills are consumers. Encode it in the types rather than in a comment — the discovery
  entry points take the resolved settings, so calling them early does not compile.

### S2 — `locode-skills`: discovery, frontmatter, precedence, collisions (M)

- New crate; depends on `locode-host` (`find_root_from_markers`, home resolution) and
  nothing else. Pure: no rendering, no injection, no engine wiring yet.
- Roots, highest precedence first: `<repo>/.locode/skills` → `~/.locode/skills` →
  each `extends` dotfolder's `skills/` → each `skills.extra` entry.
- Frontmatter: the five keys, **lenient** parsing (unknown keys ignored). A file that
  fails to parse or has an unusable name is **skipped with a stderr diagnostic** — never
  fatal, never surfaced to the model.
- Name = frontmatter `name` slug-normalized (lowercase, non-`[a-z0-9]` → `-`, collapse,
  trim, ≤64), else the directory name — grok's `normalize_skill_name` verbatim.
- Collisions: **three** qualifier scopes — `project:`, `user:`, `extra:`; an `extends`
  dotfolder's skills are `user:`. Same qualifier ⇒ precedence wins and the loser is
  dropped; different qualifier ⇒ both kept, addressable qualified.
- `disable-model-invocation: true` ⇒ excluded from what discovery returns as listable.

### S3 — the listing body: format, budget, three-tier degrade (S)

- Render the exact block in ADR-0025 §3, including the omit-the-line rule for a missing
  `when-to-use` and the two-space continuation indent.
- Budget: 50 % of the context window in chars (400 000 fallback), 400-byte per-entry cap
  split proportionally with a 20-char floor; degrade full → shortened → names-only with
  the `... and N more skills in <dir>` overflow marker.
- Pure function of `(skills, context_window)`; snapshot-tested.

### S4 — injection: whole-body diff, transcript-grounded previous state (M)

- `locode-engine`: render the body, compare it to what the conversation already carries,
  inject a `User` `<system-reminder>` only when it differs.
- "Already delivered" = **the marker is present in the transcript being sent**, not a
  side ledger — which is what makes compaction self-healing and resume correct for free
  (ADR-0025 §3.1). Test both: a transcript with the marker stripped re-injects; a
  replayed (resumed) transcript does not.
- Zero skills ⇒ no message at all; a transition from some to none ⇒ the removal notice.

### S5 — the post-run rescan seam (S)

- An engine hook that fires **after a run reaches its terminal state**; the TUI invokes
  it *after* its final render, so the filesystem work hides behind the user reading the
  reply (ADR-0025 §3.2).
- Session start is the one synchronous scan; a headless one-shot therefore scans once.
- No filesystem watcher (would be a new dependency, and ADR-0023 already chose
  rescan-over-watch).

## Explicitly out of scope (tracked elsewhere)

The read-only jail exception (ADR-0025 §4.1 — deferred behind ADR-0008's default flip);
any skill **tool** (waits for the `locode` best-of pack); slash/user invocation, so
`user-invocable` stays parsed-but-inert; `allowed-tools` (returns narrowing-only with the
permission rules); conditional `paths:` activation (rejected twice now); `--bare`;
vendor-compat roots.

## Preset targets (gate for each slice + final)

- **S1**: a `~/team-locode/` containing `settings.json` (`{"model":"x"}`) and `AGENTS.md`,
  referenced by `"extends": ["~/team-locode"]` → `locode -p --api-schema mock "hi"` uses
  the model and the trace shows the team `AGENTS.md` as an instruction entry below the
  global one. An `extends` entry pointing at a *file* errors with a message naming the fix.
- **S2**: `~/.locode/skills/commit/SKILL.md` and `<repo>/.locode/skills/commit/SKILL.md`
  both discovered, addressable as `user:commit` / `project:commit`; a `SKILL.md` with
  broken frontmatter is skipped with a stderr line and does not abort the run.
- **S3**: 30 synthetic skills against a small context window render names-only plus the
  overflow marker; one skill with no `when-to-use` renders two lines, not three.
- **S4**: `locode -p --api-schema mock "hi"` with one skill present → the trace contains
  exactly one `<system-reminder>` listing; a second turn adds none; adding a second skill
  re-sends **both**.
- **S5**: creating a skill mid-session makes it appear in the next turn's listing with no
  restart.
- Four-part gate (`fmt · clippy · test · doc`) green per slice; PR per slice, auto-merge
  on green.

## Result

_(pending)_
