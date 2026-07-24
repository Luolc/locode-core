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
`Entry { source_path, content }`. The loader **lives in `locode-host`**, reusing
its existing path/query/read machinery (ADR-0008) rather than a new crate — every
filesystem read goes through the host seam, never `std::fs` in a tool, never the
wire. The engine calls the loader and injects the neutral value.

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
Rules-dir globbing (`.locode/rules/*.md`) is deferred (see Open Questions).
**`@import` is out of scope** — we do not adopt Claude's `@path` include mechanism
(`claude-code: claudemd.ts:459-486,537`); its external-approval prompts and
cycle-guard machinery are not worth it for our headless core. Instructions are
single files assembled by the directory walk, nothing more.

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

Multi-root (`--add-dir`) is a **core/engine capability, supported in the headless
path too** — not a TUI-only concern (a headless eval/pipeline run must be able to
span extra roots). Cross-root conflicts are resolved by **append order + source-path
labeling only**; there is no "primary project always wins" override — the label
lets the model attribute a rule to its root, which is enough.

**Merge.** Additive; assembled **root→cwd** so the deepest (most specific) file
wins on conflict (universal across all four harnesses). Dedup by **canonical path**
(case-insensitive FS / symlink-resolved — Grok's robustness, `grok:
agents_md.rs:159-168`); `.gitignore`-filtered (`grok: agents_md.rs:156`); YAML
frontmatter stripped from rules files. Every entry is **labeled with its source
path** (all four harnesses do this) so the model can attribute conflicting rules. A
**byte budget with a truncation marker** bounds a runaway file: default **64 KiB**
for the whole assembled body, files exceeding the remaining budget are truncated
with a marker, and `0` disables loading — Codex's `project_doc_max_bytes` semantics
(`codex: agents_md.rs:95-130`), sized a little above Claude's 40k-char cap
(`claude-code: claudemd.ts:92`). The budget is configurable.

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
Change detection is a **per-turn rescan**, not a filesystem watcher: the cwd→root
walk is bounded (at most root-deep) and cheap, so re-scanning each turn and diffing
against the last-injected set is simpler than watch/invalidate and fast enough for
the headless loop. This is engine machinery and therefore shared.

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
  this is a forward-looking constraint, not a migration. `DeveloperRendering::SystemReminder`
  stays in the wire as a caveated escape hatch (Resolved, below) — no wire change.
- **New shared capability, not yet built.** This ADR records the design; the loader
  is a future task (tracker: *Tier B/C future capability* / *Deferred*). When it
  lands it introduces a neutral `ProjectInstructions` type and a `root_stop_pattern`
  config knob; both are additive.
- **Reconciled in this change** (ADR-first — MEMORY: adr-first-reconcile):
  ADR-0013 amendment (Developer/User), ADR-0012 amendment (boundary), `SPEC.md`
  (Boundaries + a scope line), the two research docs (superseded-recommendation
  notes), the tracker (relabel "pack session-start file context" → shared engine
  context), and the decisions index.

## Resolved (user review, 2026-07-23)

- **Loader home → `locode-host`.** Reuse its existing path/query/read machinery; no
  new crate. (Decision §2, *Shape*.)
- **Cap default → a 64 KiB byte budget** with a truncation marker, `0` to disable,
  configurable. (Decision §2, *Merge*.)
- **`@import` → out of scope.** Not built — single files + the directory walk only.
  (Decision §2, *Files*.)
- **Multi-root / `--add-dir` → in the core, headless included.** Not TUI-only; a
  headless run must be able to span extra roots. Cross-root conflicts are handled by
  append order + source-path labeling, no "primary wins" override. (Decision §2,
  *`--add-dir`*.)
- **Refresh → per-turn rescan.** The bounded cwd→root walk is cheap; re-scan each
  turn and diff, no filesystem watcher. (Decision §2, *Refresh*.)
- **Keep `DeveloperRendering::SystemReminder`.** Retain it as the caveated,
  reverse-lossy escape hatch for a deliberately-emitted non-beta `Developer`
  message; do **not** turn a non-beta `Developer` into an error. Reminders are
  `User` regardless (Decision §3), so this fallback is out of the reminder path.

## Open Questions

1. **`CLAUDE.md` alias scope** — recognize `CLAUDE.md` everywhere `AGENTS.md` is
   scanned, or only at the repo root? And do we scan any `.claude`/`.cursor`
   compatibility dirs, or keep the shared loader to `AGENTS.md`(+`CLAUDE.md`) only?
2. **Rules dirs** — whether/when to add flat rules-dir globbing
   (`.locode/rules/*.md`) to the shared loader (deferred, not rejected; `@import`
   is rejected).
