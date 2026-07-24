# ADR-0024: `~/.locode` — settings layering and the resumable session trace

## Status
Accepted (user review 2026-07-24)

## Date
2026-07-24

## Relates to
- [ADR-0023](ADR-0023-fidelity-boundary-and-agents-md-loading.md) — everything here is
  **loop/engine machinery** on ADR-0023's shared side of the fidelity boundary: identical
  for every `--harness`; a pack varies tools + prompt, never where settings or traces
  live. The global `AGENTS.md` (+ `$LOCODE_HOME`, amendment 2026-07-24) already ships.
- [ADR-0009](ADR-0009-headless-first.md) / [ADR-0014](ADR-0014-stdout-artifacts.md) — the
  trace complements (does not replace) the stdout report/stream artifacts.
- Source study: [`../research/harness-study-home-dotfolders.md`](../research/harness-study-home-dotfolders.md)
  (the four-harness dossier all rationale below cites). User decisions resolved
  2026-07-24: JSON not TOML; single directory; Claude's cwd-in-path session structure
  with a bijective encoding; no `auth.json`; no `~/.claude/skills` compat root.

## Context

Two P0s need a durable home for user data: **skills** (discovery roots) and
**settings + trace persistence** (enabling `--continue`/`--resume`). All four studied
harnesses converge on a home dotfolder (`~/.claude`, `~/.codex`, `~/.grok`, opencode's
XDG dirs) holding layered config, per-session traces, and skills — but they diverge on
format (JSON vs TOML), trace shape (single JSONL vs dir-per-session vs SQLite), and
cwd scoping (cwd-in-path vs date-buckets + index). This ADR fixes our choices, and —
the binding requirement — fixes the **extension contract**: subagents and workflows
(Claude Code's current features) must be addable later **without any
non-backward-compatible change** to what this ADR ships.

## Decision — overview

One directory, `~/.locode` (override: `$LOCODE_HOME`, same variable for every consumer;
auto-created on first use):

```
~/.locode/
├── settings.json                       # user settings layer (JSON)
├── AGENTS.md (+ AGENTS.override.md)    # global instructions — already shipped (ADR-0023)
├── sessions/
│   └── <encoded-cwd>/                  # bijective cwd encoding (§2.1)
│       ├── rollout-<timestamp>-<session_id>.jsonl   # one trace per session (§2)
│       └── .cwd                        # sidecar; only for >255-byte hash-fallback dirs
└── skills/
    └── <name>/SKILL.md                 # user skills (§3)
```

Per repo (the project layers):

```
<repo>/.locode/
├── settings.json                       # project settings (committed)
├── settings.local.json                 # personal overrides (gitignored)
└── skills/<name>/SKILL.md              # project skills
```

Reserved names, defined by this ADR but **not shipped yet** (they slot in additively):
`sessions/history.jsonl` (cross-session input recall, when the TUI composer does
history), a rebuildable listing index (only if scale ever demands it, §2.5), and
`.lock` siblings on mutable files.

## 1. Settings

### 1.1 Format: JSON (not TOML)

The report envelope, tool schemas, and trace are already JSON with one serializer
(`serde_json`); TOML (codex/grok) would add a second format and parser for no
capability. `settings.json` matches Claude Code and opencode. *(User decision.)*

### 1.2 Layers and merge

Lowest → highest precedence:

1. `~/.locode/settings.json` — user
2. **`extends` files** — external settings files the *user layer* points at
   (amendment 2026-07-24; e.g. a team-shared `team-settings.json`), in list order
   (later wins within the layer)
3. `<repo>/.locode/settings.json` — project, committed
4. `<repo>/.locode/settings.local.json` — project-local, gitignored
5. `--settings <file-or-inline-json>` — flag

**The `extends` layer** (amendment 2026-07-24): the user file may carry
`"extends": ["<path>", …]` — each entry an ordinary settings JSON file merged
*above* the user layer and *below* the project layers, so a shared team file can
override personal defaults while any repo still overrides the team. Rules:

- **Only the user layer honors `extends`.** A project-layer `extends` is ignored
  with a warning — repo-controlled content must not pull arbitrary external files
  into the config tree (the §1.3 asymmetry, applied to file inclusion).
- **No recursion**: an extended file's own `extends` is ignored with a warning
  (cycle-proof by construction; revisit only if a real need appears).
- **Trust follows the pointer**: the user explicitly opted into the file, so it
  merges with user-level trust (no denylist) — a team file legitimately sets
  `model`/`api_schema`; that is its purpose. The denylist continues to bind the
  *repo*-controlled layers only.
- Relative entries resolve against the referencing file's directory; `~` expands;
  a missing/malformed entry degrades to skipped-with-warning (§1.2's rule).

Merge semantics (Claude's, `settings.ts:529-547`): objects **deep-merge**; scalars
**overwrite**; arrays **concatenate + dedupe** — so permission `allow`/`deny` lists
*accumulate* across layers instead of a deeper layer silently discarding the user's
rules. Unknown keys are **preserved** (serde round-trips them; no
`deny_unknown_fields`) so an older binary never destroys a newer config.

### 1.3 The security asymmetry (project layer is attacker-controlled)

A cloned repo ships `.locode/settings.json`, so the project layers get a **denylist**:
keys that redirect where credentials go or what executes may only come from the user
layer or the flag — at minimum the provider/endpoint selection (`api_schema`, any
future `base_url`/provider tables) and any future "skip permission prompt"-class
trust switch. This is codex's `PROJECT_LOCAL_CONFIG_DENYLIST` (`loader/mod.rs:64-76`)
+ Claude's skip-projectSettings-for-trust-reads pattern; the study calls it the
load-bearing idea of settings layering, and it is a *reviewed list* — extending the
denylist is a normal change, shrinking it needs an ADR amendment.

### 1.4 Fields (v1)

The starter set, chosen important-and-cheap (each is a durable default for an
existing or imminent run parameter; a flag always wins):

| Key | Type | Notes |
|---|---|---|
| `model` | string | Default model. Resolution is `--model` flag > settings > the wire's built-in default; there is deliberately **no model env var** (amendment 2026-07-24 — `LOCODE_MODEL` was removed from the provider factories so the model's precedence chain matches every other knob). |
| `api_schema` | string | Default wire (persists today's flag/env). **Project-denylisted** (§1.3). |
| `harness` | string | Default pack (`--harness`'s durable default). |
| `instructions.root_stop_pattern` | string (regex) | Activates ADR-0023's dormant root-detection seam: a directory whose absolute path matches is the project root (the escape hatch for VCS-less trees — monorepo segments, `/workspace/<project>`). Activation requires the `regex` dependency — an ask-first item, approved by accepting this ADR. |
| `skills.extra` | list of paths | Manual skill entries beyond the standard roots (§3). Entry semantics below. |
| `extends` | list of paths | User-layer-only pointer(s) to external settings files merged between the user and project layers (§1.2, amendment 2026-07-24) — the team-shared-settings hook. |

**`skills.extra` semantics** *(user decision)*:

```json
"skills": { "extra": ["~/dev/one-off-skill", "~/team/shared-skills"] }
```

- An entry whose directory **directly contains `SKILL.md`** is a **single skill**.
- Otherwise it is a **skills folder** — each child directory containing `SKILL.md`
  is a skill — and its path **must end in `skills`**; a folder entry that doesn't is
  a config error (the guard against accidentally pointing discovery at some huge
  unrelated tree). `~` is expanded.
- The value is a plain list, but it sits under a `skills` **object** so future
  siblings (`disabled`, `ignore` — grok ships exactly that trio,
  `[skills] paths/ignore/disabled`) are additive; a bare `skills: [...]` would force
  a breaking list→object reshape later (§1.5's own rules).

**Deliberately CLI-only — not settings** *(user decision)*:
`--no-project-instructions`, `--max-turns`, `--no-session-persistence` (§2.2) — per-
invocation switches the user will essentially never want as durable state; keeping them
out of settings avoids a forgotten config permanently distorting runs. `--stream`,
`--strip-identity`, and `--dangerously-skip-permissions` follow the same principle (the
last is additionally a trust-class switch that must never be durable).

**First-run scaffold** *(amendment 2026-07-24, user decision)*: when the **user**
`settings.json` is absent, the loader writes it with every v1 key at its **current
default**, then proceeds normally. This freezes today's defaults as explicit config
(a later change to a built-in default cannot silently move an existing user's runs)
and doubles as a discoverable template. Written with `create_new` (a concurrent
first run is race-safe), keys in **lexicographic order** (byte-stable output), and
any failure is silent — the loader behaves identically without the file. Only the
user layer is ever scaffolded; project layers stay opt-in.

The scaffolded defaults are `harness: "claude"`, `api_schema: "anthropic"`,
`model: "claude-sonnet-5"` (amendment 2026-07-24), with `extends`/`skills.extra`
empty and `instructions.root_stop_pattern` null. The **built-in** fallbacks (used
when no settings file exists at all) match: `claude` / `anthropic` / the wire's
default model.

**Reserved (shapes defined by their own features, later)**: `env` (session
environment variables), `permissions` `{allow, deny, ask, default_mode}` (the
array-union merge of §1.2 is designed for it; lands with the permission-rules work),
`cleanup_period_days` (trace GC, once traces exist), `tui.*` (display preferences).

### 1.5 Backward compatibility rules for settings

- **Additive-only evolution**: new keys get defaults; existing keys are never
  repurposed.
- **Rename = read-old-write-new**: a renamed key keeps a serde `alias` for the old
  name (read both, write new) — files migrate on their next save, old binaries still
  read their own key.
- **Structural change** (if ever): an explicit, one-shot migration function keyed on a
  marker (opencode's `ConfigMigrateV1` / codex's `.{name}_migration` marker files) —
  never silent reinterpretation.

## 2. The session trace

### 2.1 Location: Claude's cwd-in-path structure, with a bijective encoding

`~/.locode/sessions/<encoded-cwd>/rollout-<timestamp>-<session_id>.jsonl`.

Grouping sessions by cwd makes `--continue` an O(1) "list one directory, take the
newest" — no date-tree scan with per-file header reads, no SQLite index (Codex needs
one or the other because its paths encode the *date* instead; cwd-in-path and
date-in-path are a strict trade, and continue-for-this-repo is our hot query). The
known flaw of Claude's structure — its lossy `[^a-zA-Z0-9]` → `-` sanitization
(`foo-bar`/`foo_bar`/`foo.bar`/`foo/bar` all collide; any two same-length CJK names
collide) — is fixed by making the encoding a **bijection** *(user decision)*:

- `/` → `+` (the readable separator swap);
- literal `+` → `%2B`, literal `%` → `%25` (the self-escape that upgrades "less likely
  to collide" into "cannot collide");
- every other character, including non-ASCII, verbatim (`/Users/me/dev/项目` →
  `+Users+me+dev+项目`).

Decoding inverts it exactly, so a resume picker recovers real paths from dirnames with
zero file reads. Callers pass a canonicalized cwd. When an encoded name would exceed
the 255-byte filename limit: a stable `<fnv1a64-hex16>-<encoded-tail>` fallback name
(starts with a hex digit, never `+` — the first byte disambiguates it from every
decodable name) plus a `.cwd` sidecar inside the directory holding the original path
(grok's scheme; FNV-1a because it is stable and dependency-free — `DefaultHasher` is
not stable across Rust releases and a hashing dependency is an ask-first item).

The filename keeps `rollout-<timestamp>-<session_id>` (colons → dashes): name-sorting
within a directory is reverse-chronological for free, and the id in the name lets
`--resume <id>` scan without opening files. `session_id` is UUIDv7 (time-sortable).
*(Amendment 2026-07-24, Task 31 S3: v1 keeps the engine's existing
`sess-<millis>-<rand>` ids — equally time-sortable and unique-enough for a personal
store; UUIDv7 would add a `uuid` dependency (ask-first) for no v1 capability. Switch
when that dependency is otherwise justified — the format carries ids opaquely, so the
change is non-breaking.)*

**The directory key is the session's *start* cwd, immutably.** A session belongs to
the directory it was launched from; a (future) mid-session persistent `cd` appends a
`turn_context` record but **never moves the file** — moving would race concurrent
readers/`--resume` scans and break the dirname↔cwd bijection. All four studied
harnesses key by start cwd (Claude `getOriginalCwd()`; codex's cwd filter reads the
head `session_meta`; grok's by-id resolver scans *all* cwd dirs for exactly this
reason). Consequence: after a mid-session `cd A→B`, `--continue` in **A** finds the
session (and resumes with effective cwd = B via the latest `turn_context`);
`--continue` in **B** does not — its semantics are "the newest session *started*
here" — while `--resume <id>` still finds it anywhere via the scoped-then-global
scan. (Today this is moot: the engine's cwd and path-jail root are fixed at start —
a `cd` inside a shell call is per-invocation. If persistent `cd` ever makes the
B-side miss a real pain, the extension contract admits additive fixes — a pointer
file in B's dir or a `seen_cwds` header field — with no layout change.)

For the same reason, a future `--add-dir` (extra working roots — today a dormant
`extra_roots` seam, ADR-0023) widens **discovery and access** only: instruction
loading, skills roots, and the tool path-jail. It never participates in the trace's
directory key or `--continue` scoping — a session has exactly one primary cwd, fixed
at start (Claude's `--add-dir` likewise leaves the transcript's project dir alone).

### 2.2 The file: one append-only JSONL rollout per session

JSONL is authoritative — the trace maps 1:1 onto the engine's sample→dispatch→append
loop (the file literally *is* the appended history, preserving the
`tool_use`→`tool_result` pairing invariant), stays greppable/jq-able while the format
is young, and is the shape both Claude and Codex ship. **No database in v0** (§2.5).

Tracing is **on by default**; `--no-session-persistence` skips writing (no rollout is
created or appended) while `--continue`/`--resume` still *read* — Claude Code's flag of
the same name *(amendment 2026-07-24)*. It is a CLI-only per-run switch (§1.4's rule),
never durable settings.

### 2.3 Line format — the data structure

Every line is one JSON object with the same three-field envelope:

```json
{"timestamp": "<RFC3339 millis UTC>", "type": "<record type>", "payload": { ... }}
```

**Line 1 is always `session_meta`.** Its payload (v1 fields):

```json
{
  "schema_version": 1,
  "session_id":  "<uuidv7>",
  "kind":        "main",
  "parent_id":   null,
  "group":       null,
  "cwd":         "/abs/canonical/path",
  "git":         {"root": "...", "branch": "...", "head": "...", "remote": "..."},
  "cli_version": "0.1.9",
  "harness":     "codex",
  "api_schema":  "openai-responses",
  "model":       "gpt-5.6-sol"
}
```

`git` is `null` outside a repo. `harness` (not just `model`) is recorded so resume
rehydrates the right pack regardless of the model catalog — grok persists
`agent_name` for exactly this reason (`persistence.rs:786-870`).

**The `session_meta` payload is an open, growing record — plan on it.** Every future
feature that needs per-session metadata lands here as a **new optional field**, and
that must never break an existing reader or an existing file. The binding rules
(they restate §2.4 for this one payload because it is where growth will
concentrate):

- **New fields are optional with a defined default.** A v1 file missing the field
  reads as the default in a newer binary; a newer file carrying it is read by a v1
  binary, which ignores it (the reader never rejects unknown fields). Both
  directions hold *by construction*, not by review vigilance.
- **Existing fields are never removed, renamed, or repurposed.** A field that stops
  mattering is simply no longer written (absent ⇒ default), its meaning frozen.
- **`kind`, `parent_id`, `group` are the pre-reserved growth points** for
  subagents/workflows (§2.4); concrete future fields we already anticipate —
  `agent_type`/`description` (subagent resume routing, Claude's `.meta.json`
  sidecar fields), `seen_cwds` (§2.1), `title` (a resume-picker label, grok's
  `generated_title`) — all fit this pattern without touching `schema_version`.
- Grok's `Summary` struct is the working proof: every field
  `#[serde(default, skip_serializing_if)]` precisely so old and new binaries share
  one on-disk population (`persistence.rs:786-870`); our `session_meta` follows the
  same discipline.

**Subsequent lines** are one of the v1 record types:

| `type` | `payload` | Notes |
|---|---|---|
| `message` | a `locode-protocol` `Message`, **verbatim** (`{role, content: [blocks…]}`) | The workhorse. One appended `Message` = one line, in append order — preamble (`system`), user turns, assistant turns (with `tool_use` blocks), tool results (`user` role with `tool_result` blocks). Replay = collect `message` lines in order; the file is a **self-sufficient replay source** (it starts with the rendered preamble, like our `stream-json` `Init`). |
| `turn_context` | `{"cwd": "...", "model": "..."}` | Per-turn deltas of mutable run state, written only when a value changes. Codex-proven (`protocol.rs:3272`): resume reads the *latest* `turn_context` so a mid-session `cd`/model switch resumes correctly. |
| `usage` | the run's `Usage` (input/output/cache tokens) | Written at each run's end *(amendment 2026-07-24)*. The message stream alone cannot reconstruct token counts, so without this a resumed session could only estimate its context occupancy; with it, resume restores the exact figure. A textbook §2.4 additive record type — older readers skip it, older rollouts still load (callers fall back to a byte estimate). |
| `compacted` | `{"summary": "...", "replacement_history": [Message…]}` | **Reserved** — written by nobody in v1 (the engine does not compact yet). Defined now so shipping compaction later is purely additive: on replay, a `compacted` line replaces all prior `message` lines with `replacement_history` (codex's `CompactedItem`). |

Storing the protocol `Message` verbatim (not a lossy re-projection) is Claude's
"store the API message inside a metadata envelope" lesson: the trace can never drift
from what the engine actually appended, and every future `ContentBlock` variant
(reasoning formats, images, …) rides along without a trace-format change.

### 2.4 The extension contract (subagents, workflows — no breaking change)

The binding requirement: Claude-Code-class features must be addable **without
touching v1's shapes**. Mechanisms, fixed now:

1. **Readers skip unknown `type` values.** A future record type (e.g. `event`,
   `attachment`) is invisible to old readers, not an error.
2. **Readers ignore unknown `payload` fields.** New optional fields (never
   `deny_unknown_fields` in the trace reader — note this deliberately *diverges* from
   the type-strict tool-args policy, which is about model-facing schemas, not our own
   persistence).
3. **Open string enums.** `kind` is a string, not a closed enum: v1 writes `"main"`;
   subagents write `"subagent"`, workflow-spawned agents `"workflow"` later. A lister
   that sees an unknown `kind` simply does not surface that session (grok's
   `hidden`/`session_kind` pattern) — old binaries degrade to *not listing* new
   session kinds, never to crashing on them.
4. **Subagent mapping is header-fields, not directory nesting** (the codex/grok
   route, deliberately not Claude's `<sessionId>/subagents/` tree): a subagent's
   trace is an ordinary rollout file in the same `<encoded-cwd>` directory whose
   `session_meta` carries `parent_id: <parent session_id>` and `kind: "subagent"`.
   The parent's own trace already contains the spawn point (the agent tool's
   `tool_use`/`tool_result` `message` line, with the child id in the result) — the
   same three-way mapping Claude achieves, with zero new directory schema.
5. **Workflow grouping is the `group` field** (`null` in v1): a workflow run stamps
   its `run_id` into each spawned agent's `session_meta.group`. Claude solves this
   with `subagents/workflows/<runId>/` subdirectories; a header field expresses the
   same structure without a path-layout change.
6. **`schema_version` is the escape hatch, not the plan.** It exists so a truly
   breaking change is *representable*, but rules 1–5 exist so it never has to move.

### 2.5 Listing, resume, durability

- **`--continue`** = encode the cwd, list that one directory, take the newest rollout.
- **`--resume <id>`** = check the cwd's directory first, then scan the other cwd
  directories for `*-<id>.jsonl` (Claude's scoped-then-global resolver).
- **What resume recovers from the header, and what it doesn't** *(amendment
  2026-07-24, user decision)*: the **pack** and **wire** are header-bound (an
  explicit conflicting flag is a pre-run error) because they affect transcript
  validity — a session must not change its toolset or cross wires mid-transcript.
  The **model is not**: it resolves exactly like a fresh run (`--model` > settings >
  the wire's default), so yesterday's model never leaks into today's resumed run.
  The header still *records* the model the session started under (provenance), it
  simply does not steer the resumed run.
- **No SQLite in v0.** We are a headless core + one TUI — none of opencode's
  many-client/search/branching pressure that justifies its event-sourced DB (38
  migrations, a projection engine, an opaque store). If listing over a huge history
  ever needs it, the study's answer is codex's: a **derived, rebuildable** index that
  is never authoritative (DB-first, filesystem read-repair). Never opencode's model.
- **Durability** (grok/codex patterns): the writer guarantees newline-termination on
  reopen (torn-tail healing — a crash corrupts at most one line); readers skip
  unparsable lines; rollout files `0600`, session dirs `0700`; mutable shared files
  get sibling `.lock` files (advisory `flock`) when concurrent writers appear.

## 3. Skills (roots only — format/loader are the skills P0's ADR)

Discovery roots mirror the settings layers 1:1: `~/.locode/skills/<name>/SKILL.md`
(user) and `<repo>/.locode/skills/<name>/SKILL.md` (project), plus the manual
`skills.extra` entries from settings (§1.4 — single skills or `…skills` folders). **No `~/.claude/skills`
compat root** *(user decision)* — grok reads Claude's tree for migration convenience,
but importing skills written against another harness's tool names/conventions into an
A/B-oriented agent muddies provenance; our own tree only. This ADR reserves the
paths; frontmatter, the two-switch invocation gate, and injection live in the skills
P0 design (per ADR-0023 they are one shared-engine implementation, `User`-role
`<system-reminder>` listing).

## 4. What `~/.locode` deliberately does NOT contain

- **No `auth.json`.** Machine-managed auth files exist because OAuth refresh tokens
  rotate; static API keys don't. Keys stay **env-only** (`ANTHROPIC_API_KEY`,
  `OPENAI_API_KEY`, `XAI_API_KEY`, … read by the provider factories) — grok's
  API-key path. An `auth.json` (0600, separate from settings, never in the trace)
  becomes necessary only if an OAuth flow ever lands. *(User decision.)*
- **No XDG split.** One `~/.locode` instead of data/config/cache/state homes
  (opencode): a single tree is simpler to document, back up, and delete, and none of
  our v0 artifacts have divergent lifecycles yet. *(User decision.)* Revisitable
  additively (a future cache could move without touching settings/sessions).
- **No `~/.claude.json`-style monolithic state blob.** Machine-managed state that
  appears later (workspace trust, usage) gets its own small file, not a second
  settings store with an opposite edit model.

## Alternatives rejected

- **TOML config** (codex, grok) — a second serializer/format for no capability; JSON
  matches every other surface we ship. *(User decision.)*
- **Date-bucketed trace paths + index** (codex `sessions/YYYY/MM/DD/…`) — was this
  document's own first draft; superseded because `--continue`-for-this-cwd is the hot
  query and cwd-in-path answers it O(1). The date scheme's listing-order benefit is
  kept anyway via the `rollout-<timestamp>-…` filename. *(User decision.)*
- **Claude's lossy cwd sanitization** — collides real-world paths (hyphen/underscore/
  dot/nesting; all non-ASCII); replaced by the bijective encoding, which also makes
  dirnames reversible for pickers.
- **Dir-per-session** (grok) — its wins (per-stream locks, tiny `summary.json` index)
  buy concurrency we don't have, at the cost of a multi-file session and a separate
  index file duplicating what our line-1 `session_meta` already holds.
- **SQLite / event sourcing** (opencode v2) — see §2.5.
- **Directory-nested subagent traces** (Claude `<sessionId>/subagents/…`) — header
  fields (`parent_id`/`kind`/`group`) express the same parent↔child↔group mapping
  with no path-layout change, which is exactly the backward-compatibility property
  this ADR is required to guarantee.
- **`~/.claude/skills` compat root** (grok reads it) — provenance over convenience.
  *(User decision.)*

## Consequences

- The settings P0 implements §1 (loader + merge + denylist) and the persistence P0
  implements §2 (encoder, writer, reader, `--continue`/`--resume`) directly from this
  ADR; the skills P0 gets its roots from §3 and writes its own design for the rest.
- Subagents/workflows (P0.5+) extend the trace by writing new `kind`/`group`/
  `parent_id` values and (if needed) new record types — v1 readers keep working,
  because rules §2.4.1–5 are part of the shipped reader from day one.
- The trace makes every run replayable from disk; the stdout report (ADR-0009/0014)
  remains the machine artifact of a single run. Nothing here is pack-visible: no tool
  or prompt changes for any harness.
