# ADR-0023: The fidelity boundary, and shared project-instruction (`AGENTS.md`) loading

## Status
Accepted

## Date
2026-07-23

## Amends
- [ADR-0013](ADR-0013-conversation-protocol.md) — narrows the `Developer` role to
  losslessly native-mapped content and moves injected framing/reminders to `User`
  (see the ADR-0013 amendment dated 2026-07-23).
- [ADR-0012](ADR-0012-harness-packs.md) — states, explicitly, where pack
  faithfulness stops (see the ADR-0012 amendment dated 2026-07-23).

Partially supersedes the *Recommendation* sections of
[`docs/research/harness-study-agents-md.md`](../research/harness-study-agents-md.md)
and [`docs/research/harness-study-skills.md`](../research/harness-study-skills.md):
their per-pack fidelity tables and their `Developer`-role injection are replaced by
the boundary and the `User`-role injection decided here. The seven-axis source
studies themselves remain accurate as *descriptions* of what each harness does.

## Context

Two questions came due together while designing `AGENTS.md`/`CLAUDE.md`
project-instruction loading (a still-unbuilt capability):

1. **How much of a harness do we faithfully reproduce?** ADR-0012 says a pack is a
   "faithful reproduction of one harness's toolset … and its system prompt," and
   the MEMORY note *harness-fidelity-boundary* already draws the line at
   "tools + prompts + preamble; loop-adjacent behavior stays on the shared engine."
   But the `AGENTS.md` and skills studies each ended with a **per-pack fidelity
   table** — codex mimics Codex's `--- project-doc ---` separator and world-state
   banners; grok mimics Grok's exact `<system-reminder>` envelope and vendor dirs;
   claude mimics the 4-tier hierarchy + `@import` + 40k cap; opencode alone injects
   into the system prompt — i.e. **four separate loaders and four injection
   formats.** That is a large, duplicated surface whose variation the A/B does not
   measure. The scope of "faithful" was never pinned; left vague it sprawls.

2. **Which role carries injected project instructions?** The `AGENTS.md` study
   recommended the `Developer` role (ADR-0013), rendered via its portable
   `<system-reminder>` fallback. On review that is the wrong role for this content
   (see Decision §3): `Developer` was introduced to map **1:1 and losslessly** onto
   a provider-native role, and the `<system-reminder>` fallback breaks that
   property in the reverse direction.

The `README.md` was updated in the same period to state the reproduction scope out
loud ("each pack faithfully mirrors only its harness's system prompt and the six
core tools; memory files, skills, and richer engine behaviors are shared, not
reproduced per harness"). This ADR is the decision of record behind that sentence,
and reconciles `SPEC.md` and the affected ADRs to match.

Empirical grounding is the two seven-axis source studies (conducted 2026-07-22
against the `coding-cli-survey` submodules), cited below by `harness: path:line`.

## Decision

### 1. The fidelity boundary — what "faithful" covers, and where it stops

A pack faithfully reproduces exactly two surfaces of its harness:

- its **system prompt / static preamble**, and
- its **tool set** — the six core `ToolKind`s (shell, read, write, edit, list,
  search — `locode-tools/src/tool.rs`) as that harness really ships them: names,
  argument schemas, descriptions, behavior, caps, and guardrails (ADR-0012).

**Everything else is shared, single-implementation engine machinery, identical for
every pack.** In particular, the following are **not** reproduced per harness:

- project-instruction loading (`AGENTS.md`/`CLAUDE.md`, rules dirs, global files);
- skills discovery and listing/body injection;
- reminder / context injection and its wrapping (`<system-reminder>` framing);
- compaction, session continuity, subagent orchestration, background tasks.

A pack chooses *tools + a prompt*; it never forks the loop or the context pipeline.

**Why.** (a) *Honest A/B.* The experiment exists to compare harness **surfaces** on
one engine and one wire (ADR-0012); if each pack also brought its own loader,
injection format, and refresh policy, a trajectory difference could come from the
machinery rather than the surface — the measurement is only clean when the loop is
constant. (b) *Cost without value.* Four loaders × discovery/merge/dedup/cap/refresh
is a large duplicated surface whose variation the A/B does not measure. (c)
*Entanglement.* These behaviors are woven into engine seams that are already
single-owner (dispatch — ADR-0008; continuity — ADR-0016; compaction). A faithful
per-pack copy would have to fork those seams too.

This is the same line the MEMORY note *harness-fidelity-boundary* draws; this ADR
makes it a decision of record and extends it explicitly to `AGENTS.md` loading and
skills. It supersedes the per-pack fidelity tables in the two studies.

**What the studies are still good for.** Their seven-axis descriptions remain the
authoritative catalogue of harness behavior, and they are the **menu we pick the
one shared "best-of" design from** — see §2.

### 2. Project-instruction loading — one shared loader, best-of defaults

A single headless loader in the shared engine produces a neutral value and injects
it once per turn. It is not pack-selectable; every pack gets the same behavior.

**Shape.** A neutral `ProjectInstructions { entries: Vec<Entry> }` where
`Entry { source_path, content }`. The loader routes all filesystem reads through
the `locode-host` seam (ADR-0008) — never `std::fs` in a tool, never the wire.

**Files.** `AGENTS.md` is canonical (this repo already standardizes on it).
`CLAUDE.md` is recognized as a compatibility alias so existing repos load without
renaming. A per-directory local override (`AGENTS.override.md`, à la Codex —
`codex: agents_md.rs:37-40`) and a global `~/.locode/AGENTS.md` are recognized. The
override is **same-directory, first-match-wins**: within one directory, if
`AGENTS.override.md` exists it **replaces** that directory's `AGENTS.md` entirely
(`codex: agents_md.rs:211-217` returns the first candidate found per dir) — it does
**not** override files in other directories, and it is not additive. It is the
conventionally-gitignored "local, uncommitted variant" of a directory's checked-in
`AGENTS.md` (the tool does not gitignore it for you). This contrasts with Claude's
`CLAUDE.local.md`, an *additive* private tier rather than a replacement — we adopt
Codex's replacement semantics.
Rules-dir globbing (`.locode/rules/*.md`) and `@import` (Claude-style —
`claude-code: claudemd.ts:459-486,537`) are **deferred** (see Open Questions).

**Root detection (walk).** Ascend from cwd; **stop at the nearest ancestor that
matches either rule, first match wins**:

1. a **root marker** in the directory — default set `{.git}`, configurable (Codex's
   `project_root_markers` — `codex: agents_md.rs:172-187`); or
2. a configurable **root-path regex** (`root_stop_pattern`, new): if the
   directory's absolute path matches, that directory is the root.

Rule 2 is the deliberate escape hatch for trees with no VCS marker (a monorepo
segment, a `/workspace/<project>` layout). It generalizes the marker stop from "a
sentinel file exists" to "the path looks like a root."

**No marker and no regex match, up to the filesystem root ⇒ fall back to
cwd-only** (scan just the current directory; do **not** walk to the filesystem
root). This matches Codex/Grok/opencode's out-of-repo behavior
(`codex: agents_md.rs:141-143`; `grok: agents_md.rs:89-91,141-143`;
`opencode: instruction-context.ts:48-49`) and rejects Claude Code's walk-to-FS-root
(`claude-code: claudemd.ts:850-878`), which is noisy and occasionally reads
unrelated ancestors. The filesystem root is only ever a hard backstop for the
ascent, never itself treated as the project root.

**`--add-dir`.** Each `--add-dir <dir>` is an **additional root**: it is discovered
by the same walk (ascend from `<dir>` to *its* root, assemble root→dir) and its
entries are appended **after** the primary cwd chain, in CLI order, each labeled by
`source_path`. `--add-dir` additionally widens the path-jail (ADR-0008) — the two
effects share one flag, as in Claude/Codex (`claude-code: main.tsx:1000`;
`codex: shared_options.rs:61`). Auto-discovery may be disabled (below) but an
explicit `--add-dir` is **still honored** — Claude's "skip what I didn't ask for,
not what I asked for" rule (`claude-code: context.ts:162-172`).

**Merge.** Additive; assembled **root→cwd** so the deepest (most specific) file
wins on conflict (universal across all four harnesses). Dedup by **canonical path**
(case-insensitive FS / symlink-resolved — Grok's robustness, `grok:
agents_md.rs:159-168`); `.gitignore`-filtered (`grok: agents_md.rs:156`); YAML
frontmatter stripped from rules files. Every entry is **labeled with its source
path** (all four harnesses do this) so the model can attribute conflicting rules. A
**byte cap with a truncation marker** bounds a runaway file (Codex's discipline —
`codex: agents_md.rs:95-130`; Claude's 40k char cap — `claude-code: claudemd.ts:92`);
the exact cap is an Open Question.

**Injection.** One **`User`-role** meta message wrapping the assembled body in
`<system-reminder>…</system-reminder>`, with (a) a short authority preamble, (b)
per-file `## From: <source_path>` sections in root→cwd order, (c) a "deeper files
take precedence on conflict" note (`grok: agents_md.rs:194-227`), and (d) a
relevance / "do not answer this as a question" disclaimer (Claude's framing —
`claude-code: api.ts:461-473`). Role rationale is §3. This reproduces the
Grok/Claude on-the-wire shape (a user-role `<system-reminder>`) without borrowing
their per-pack variation — one format for every pack.

**Refresh.** Modeled as diff-able state: injected **idempotently** (never
double-injected on fork/resume — `grok` idempotence tests), **re-injected after
compaction**, and re-emitted with a replace/remove banner when the files change
mid-session (Codex/opencode's mature pattern —
`codex: context/world_state/agents_md.rs`; `opencode: instruction-context.ts:36-37`).
This is engine machinery and therefore shared.

**Enable/disable.** An env switch plus a `--bare`-style flag turns auto-discovery
off atomically (Claude's `--bare` — `claude-code: main.tsx:976`; Codex's
`--ignore-*` trio), while still honoring explicit `--add-dir`.

### 3. `Developer` is for native-role mapping only; injected framing is `User`

Project instructions — and injected reminders/framing generally — are authored as
**`User`-role** content blocks carrying `<system-reminder>…</system-reminder>`
text. They are **not** modeled as `Developer` and rendered down to a user turn.

`Role::Developer` (ADR-0013) exists to map **1:1 and losslessly** onto a
provider-native "app-author instructions" role — OpenAI `role:"developer"` and
Anthropic's beta mid-conversation `role:"system"`
(`crates/locode-provider/src/openai/responses/build.rs:78`;
`crates/locode-provider/src/anthropic/config.rs:88-94`). On those wires the mapping
is **bijective**: `Developer ⇄ payload role` round-trips exactly.

The problem is the **portable fallback** — rendering `Developer` as a
`role:"user"` message wrapped in `<system-reminder>`
(`crates/locode-provider/src/anthropic/build.rs:133-146`). The *forward* direction
is fine, but the *reverse* is unrecoverable: given a `role:"user"` payload message,
nothing reliably distinguishes "a `Developer` that was rendered down" from "genuine
user text" (or a plain project-instruction reminder). Recovering the original role
would require hand-maintained tag/format detection that differs per pack and breaks
the moment a user's own message contains the sentinel string. **A value whose only
faithful rendering on a wire is the user-`<system-reminder>` fallback must therefore
be modeled as `User` from the start** — then there is no role to recover and the
conversation ⇄ payload conversion is losslessly bidirectional by construction.

Injected framing (project instructions, reminders) is exactly that class, so it is
`User`. `Developer` is **reserved** for content that has a genuine native role to
ride (the beta system message, or OpenAI `developer`), where the round-trip stays
bijective. The `DeveloperRendering::SystemReminder` portable fallback is retained
in the wire for callers who deliberately emit a `Developer` message on a non-beta
Anthropic wire, but it is **not** the vehicle for reminders and carries the
reverse-lossy caveat recorded in the ADR-0013 amendment.

## Alternatives Considered

- **Per-pack faithful loaders + injection (the studies' original recommendation).**
  Rejected: cost without measurement value, and it contaminates the A/B with
  machinery variation (Decision §1). The studies' descriptions are kept as the menu
  for the one shared design.
- **Inject project instructions into the `System` prompt (opencode's choice —
  `opencode: session/runner/llm.ts:208-210`).** Rejected: mutating the system
  prompt mid-session busts the provider prompt cache and forces the whole
  epoch/baseline machinery opencode built to manage it; our System/Developer split
  already offers a higher-authority path without touching System.
- **Inject as `Developer` with the portable `<system-reminder>` fallback (the
  study's recommendation).** Rejected: reverse-lossy (Decision §3).
- **Walk to the filesystem root when no `.git` is found (Claude Code).** Rejected:
  noisy and occasionally surprising; cwd-only fallback + the `root_stop_pattern`
  escape hatch covers the real cases without reading unrelated ancestors.

## Consequences

- **One loader, one injection format, one refresh policy** for every pack; the
  packs shrink to *tools + prompt*. Less code, and an A/B that varies only the
  surface under test.
- **`Developer` narrows**: it is the native-mapped role only; reminders are `User`.
  No current code path emits `Developer` for reminders (today `Role::Developer` is
  produced only in protocol tests — `crates/locode-protocol/src/lib.rs:476,581`), so
  this is a forward-looking constraint, not a migration. Whether to eventually
  retire `DeveloperRendering::SystemReminder` from the wire is an Open Question, not
  decided here.
- **New shared capability, not yet built.** This ADR records the design; the loader
  is a future task (tracker: *Tier B/C future capability* / *Deferred*). When it
  lands it introduces a neutral `ProjectInstructions` type and a `root_stop_pattern`
  config knob; both are additive.
- **Reconciled in this change** (ADR-first — MEMORY: adr-first-reconcile):
  ADR-0013 amendment (Developer/User), ADR-0012 amendment (boundary), `SPEC.md`
  (Boundaries + a scope line), the two research docs (superseded-recommendation
  notes), the tracker (relabel "pack session-start file context" → shared engine
  context), and the decisions index.

## Open Questions

1. **Loader home** — a module in `locode-host`, a small new `locode-instructions`
   crate, or the engine? The walk + reads must route through the host seam
   (ADR-0008); rules-dir enumeration could reuse the ripgrep/glob machinery
   (ADR-0011). (Adding a crate is an *Ask-first* boundary — SPEC.)
2. **Cap default** — Codex's byte budget vs Claude's 40k-char cap, and the exact
   truncation marker.
3. **`CLAUDE.md` alias scope** — recognize `CLAUDE.md` everywhere `AGENTS.md` is
   scanned, or only at the repo root? And do we scan any `.claude`/`.cursor`
   compatibility dirs, or keep the shared loader to `AGENTS.md`(+`CLAUDE.md`) only?
4. **`@import` and rules dirs** — ship in the shared loader later (Claude-style
   modularity, with the external-approval + cycle-guard complexity), or keep the
   shared surface to single files + a flat rules dir?
5. **Cross-root precedence for `--add-dir`** — appended-after (this ADR) is a
   deterministic default; is any "primary project always wins on conflict" override
   needed, or is source-path labeling enough?
6. **Retire `DeveloperRendering::SystemReminder`?** Now that reminders are `User`
   and `Developer` is native-mapped only, is the reverse-lossy portable fallback
   still worth keeping in the Anthropic wire, or should a non-beta Developer message
   become an error (forcing the caller to choose `User` or the beta)?
7. **Refresh cost** — per-turn `stat` vs watch/invalidate for mid-session reload in
   the headless loop.
