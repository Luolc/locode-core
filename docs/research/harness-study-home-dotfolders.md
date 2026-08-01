# Harness study — home dotfolders (`~/.claude` · `~/.codex` · `~/.grok` · opencode) → `~/.locode`

> **Source freshness.** Last verified against the `coding-cli-survey` submodules:
> **2026-07-24** (the newest dated note below — this stamp was inferred from the
> document's own history, not from a re-read on 2026-07-27).
> Submodule commits as of 2026-07-27: `claude-code` 6a25909 · `codex` f201c30c · `grok-build` b189869 · `opencode` 1754480.
>
> `AGENTS.md` requires a fresh source re-read when planning each task
> ([`autonomous-workflow.md`](../autonomous-workflow.md) Phase 1). **Update this line
> — date and commits — in the same PR as that re-read.** Without it a reader cannot tell
> whether the `file:line` citations below still point at what they claim — which is how a
> wrong injection point survived months in the subagent study (corrected 2026-07-26, #240).

Source study of the **home dotfolder / user-data directory** across the four surveyed
harnesses, conducted 2026-07-24 against the `coding-cli-survey` submodules (source) and
the live `~/.claude` / `~/.codex` / `~/.grok` on disk (read-only, secrets redacted).
Citations are `path:line` relative to each submodule root. This document feeds two P0s:
**skills** and **settings + trace persistence to `~/.locode`** (`--continue`/`--resume`).
See [`harness-study-skills.md`](harness-study-skills.md) for the skills-format deep dive
(this report covers where skills live on disk + the surrounding folder); see
[ADR-0023](../decisions/ADR-0023-fidelity-boundary-and-agents-md-loading.md) for the
fidelity boundary that keeps this machinery on the **shared engine**, not per-pack.

> **Scope note.** `~/.locode` is our own agent's home dir — it is **loop/engine
> machinery, not a harness pack surface** (ADR-0023). It is the *same for every
> `--harness`*; a pack varies tools + prompt, never where settings or traces live. So
> this is a "best-of" design (like the `locode` pack), not a faithful port of any one
> harness.

---


> **Correction (2026-08-01) — how Claude Code survives two writers on one transcript.**
> This study recorded that `--resume` reuses the session id and appends to the same
> JSONL with no lock (`sessionRestore.ts:435-437`, `sessionStorage.ts:2579`;
> `concurrentSessions.ts` is a PID registry for *counting* sessions, not a lock). True,
> but it omitted the property that makes it safe: **every entry carries `parentUuid`,
> and replay walks back from the newest leaf** (`buildConversationChain`, `:2069`; leaf
> selection `:2317`). Their transcript is a tree, so file order carries no meaning and
> interleaving is harmless. A reader who took only the recorded half — "they append
> concurrently and it is fine" — would build a flat log and lose a session to it, which
> is exactly what happened here (ADR-0024 amendment 2026-08-01). *Record the mechanism,
> not just the behavior.*

## 0. Headline findings

1. **All four converge on the same skills contract** — `SKILL.md` (YAML frontmatter
   `name`+`description`) in a per-skill directory, discovered from user + project (+
   plugin/bundled) roots, with a **two-switch invocation gate** (`user-invocable` for the
   `/slash`, `disable-model-invocation` for model autonomy). Claude and grok are byte-for-byte
   compatible enough that **grok reads `~/.claude/skills` directly** (compat roots).
2. **They split into two persistence philosophies:**
   - **Single append-only JSONL rollout per session** (Claude Code, Codex) — one file, one
     total order, crash-tolerant tail; needs a **sidecar index** for fast listing (Codex
     added a rebuildable SQLite mirror; Claude scans the cwd dir).
   - **Dir-per-session with a small `summary.json` index** (Grok) — many small streams per
     session dir; the picker reads only the tiny summaries, never the multi-MB logs.
   - **SQLite + event-sourcing** (opencode v2) — the heavy end; opencode *also* kept its
     older one-JSON-file-per-record store (v1), so it is the living sqlite-vs-files A/B.
3. **Config is either JSON (Claude, opencode) or TOML (Codex, Grok)**, always **layered**
   user < project < flags, always with a **security asymmetry**: project-committed config
   is attacker-controlled, so every harness either **denylists** sensitive keys from the
   project layer (Codex, Grok) or **excludes project settings** from trust-relevant reads
   (Claude). This is the load-bearing idea, not the precedence order itself.
4. **Every harness memoizes a `$X_HOME` env override** (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
   `GROK_HOME`, `OPENCODE_*`) and **auto-creates** the dir on first access.
5. **Secrets live in a separate `0600` file** (`auth.json` / `~/.claude.json`), never in the
   hand-edited config; most mutable files carry a sibling `.lock` (advisory `flock`).

---

## 1. Home-dir resolution + env override

| Harness | Default | Env override | Notes |
|---|---|---|---|
| Claude | `~/.claude/` **dir** + `~/.claude.json` **sibling file** | `CLAUDE_CONFIG_DIR` (redirects both) | `envUtils.ts:7` / `env.ts:14-25`; the two artifacts have opposite edit models (§4). |
| Codex | `~/.codex/` | `CODEX_HOME` (must-exist + `canonicalize()` when set) | `utils/home-dir/src/lib.rs:20-66`. |
| Grok | `~/.grok/` (canonicalized via `dunce`) | `GROK_HOME` (memoized `OnceLock`) | `paths.rs:34-47`; `default_grok_home()` split so callers detect "am I on default". |
| opencode | `$XDG_DATA_HOME/opencode` (data) + `$XDG_CONFIG_HOME/opencode` (config) | `OPENCODE_CONFIG_DIR`, `OPENCODE_DB`, `OPENCODE_TEST_HOME` | `global.ts:10-43`; XDG split of data/config/cache/state. |

**→ `~/.locode`:** `LOCODE_HOME` → `~/.locode`, memoized in a `OnceLock`, auto-created on first
access, with a `default_locode_home()` split so we can detect the default (mirror grok
`paths.rs:27-47`). Codex's must-exist+canonicalize contract when the env is *explicitly* set is
worth copying (catches a typo'd `LOCODE_HOME` early). One small resolver crate. We already honor
`$LOCODE_HOME` in the earlier `--bare` memory note — keep that.

---

## 2. Config / settings layering  ← P0 axis

### 2.1 Format + precedence per harness

- **Claude (JSON).** `SETTING_SOURCES` (`constants.ts:7-22`), low→high: `userSettings`
  (`~/.claude/settings.json`) < `projectSettings` (`.claude/settings.json`, committed) <
  `localSettings` (`.claude/settings.local.json`, gitignored — auto-added to `.gitignore` on
  write) < `flagSettings` (`--settings <file-or-json>`) < `policySettings` (enterprise, MDM,
  `managed-settings.json` + a `managed-settings.d/*.json` **drop-in dir**). Merge =
  lodash `mergeWith` with a customizer where **arrays concat+dedupe** (permission
  allow/deny lists *accumulate* across layers) and scalars overwrite (`settings.ts:529-547`);
  session-cached, de-duped by realpath. Zod `.passthrough()` so unknown keys survive; one bad
  permission rule is filtered, not fatal.
- **Codex (TOML).** Layered `merge_toml_values` (`loader/mod.rs`), low→high: `/etc/codex/config.toml`
  (system) < managed/MDM < `~/.codex/config.toml` (user) < `<repo>/.codex/config.toml`
  (project-local, **trust-gated + denylisted**) < CLI `-c key=value` < `[profiles.<name>]`
  overlay. Project trust is itself a user-config table `[projects."<abs>"] trust_level`.
- **Grok (TOML).** `deep_merge_toml`: `system_managed → managed → user → user_requirements →
  system_requirements → mdm_requirements` — **requirements merged *last* so an admin
  `requirements.toml` overrides the user** (`loader.rs:234-248`). `$VAR` expanded on load; TOML
  parse errors rebuilt from the **span, never `Display`** (Display echoes the offending line,
  which may hold a secret — `loader.rs:44-56`).
- **opencode (JSON/JSONC).** `opencode.json[c]`, low→high: global `~/.config/opencode/` <
  project files (walk up to worktree root) < `.opencode/` dirs (project + `~/.opencode`) <
  `OPENCODE_CONFIG_DIR`. Merge = **last-non-undefined-wins per top-level key** (shallow, not
  deep); permission rules use the **reverse** order so a user-global rule beats a repo rule
  (`config.ts:122-126,201-211`).

### 2.2 The security asymmetry (the real lesson)

Precedence order alone is unsafe — **project-committed config is attacker-controlled** (a
cloned repo ships a `.claude/`/`.grok/`/`.codex/` file). Each harness defends differently:

- **Claude:** trust-relevant reads **skip `projectSettings` entirely** — e.g.
  `hasSkipDangerousModePermissionPrompt` (`settings.ts:882-889`), `getAutoMemPathSetting`
  (the `autoMemoryDirectory: "~/.ssh"` attack, `memdir/paths.ts:171-186`). Policy can lock out
  user/project layers wholesale (`allowManagedPermissionRulesOnly`, `strictPluginOnlyCustomization`).
- **Codex:** a `PROJECT_LOCAL_CONFIG_DENYLIST` (`loader/mod.rs:64-76`) blocks the project layer
  from setting `openai_base_url`, `model_provider`, `model_providers`, `notify`, `otel`, … —
  "repo contents should not choose where credentials are sent or which commands run" — plus
  project **trust** gating before the project layer loads at all.
- **Grok:** the user tier **refuses a cwd-relative fallback** (`loader.rs:96-104`); a project
  `.grok/config.toml` contributes only a **narrow allowlist** — `[mcp_servers]`, `[plugins]`,
  `[permission]` — everything else is user-home-only.

### 2.3 → `~/.locode` config recommendation

- **Format: JSON.** Our report envelope + tool schemas are already JSON with one serializer;
  going TOML (Codex/Grok) adds a second. `settings.json` matches Claude/opencode and is
  jq-friendly. (This is a deliberate deviation from the two Rust harnesses — noted.)
- **Layers:** `~/.locode/settings.json` (user) < `<repo>/.locode/settings.json` (project,
  committed) < `<repo>/.locode/settings.local.json` (gitignored) < `--settings` flag <
  managed/policy (if ever needed). Deep-merge with **array-union** for permission allow/deny
  (Claude's semantics). Session-cache the merged result.
- **Security:** adopt a **project-layer denylist** for anything that redirects the provider
  endpoint / model / command execution (Codex's list is the template), and **exclude the
  project layer** from any future "skip dangerous prompt" style trust setting (Claude's rule).
  Report TOML/JSON parse errors from the span, not by echoing the source line (grok).
- **Starter fields:** `model`, `api_schema`/provider, `permissions{allow,deny,ask}`, `harness`
  default, `env`, `hooks` (later), `cleanupPeriodDays` (trace GC), skills paths. Keep an
  unknown-key-preserving parse (Claude's `.passthrough()`).

---

## 3. Session / trace persistence  ← P0 axis (`--continue`/`--resume`)

### 3.1 The three shapes, side by side

| | Claude | Codex | Grok | opencode v1 / v2 |
|---|---|---|---|---|
| **Shape** | 1 JSONL / session | 1 JSONL / session | **dir per session** | JSON-per-record files / **SQLite+events** |
| **Path** | `projects/<sanitized-cwd>/<uuid>.jsonl` | `sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl` | `sessions/<urlenc-cwd>/<uuidv7>/` | `storage/{session,message,part}/…json` / `opencode.db` |
| **Index for listing** | scan cwd dir | **rebuildable SQLite mirror** (`state_*.sqlite`) + fs fallback | small `summary.json` per dir | dir scan / DB query |
| **Cross-session input history** | `history.jsonl` (`{display,project,sessionId,timestamp}`) | `history.jsonl` (`{session_id,ts,text}`) | per-cwd `prompt_history.jsonl` (cap 10k) | — |
| **Record identity** | `parentUuid` **tree** (branch/rewind) | `ordinal?` + tagged union | per-stream files + timestamps | event `seq` per aggregate |

### 3.2 The two clean models to copy

**A. Single append-only JSONL rollout (Codex/Claude).** One file per session, first line a
**SessionMeta header**, every other line a typed record; one total order for free, crash-tolerant.

- **Codex** (`rollout/src/recorder.rs`): path `sessions/YYYY/MM/DD/rollout-<YYYY-MM-DDThh-mm-ss>-<uuid>.jsonl`
  (colons→dashes for FS compat, `recorder.rs:1547`); UUIDv7 id. **Line 1** = `RolloutItem::SessionMeta`
  (`protocol.rs:3066,3157`): `session_id`, `id`, `timestamp`, `cwd`, `originator`, `cli_version`,
  `source`, `model_provider`, `base_instructions` (full preamble), `git{commit_hash,branch,repository_url}`.
  **Each other line** = `RolloutLine { timestamp, ordinal?, #[flatten] item }` where `RolloutItem`
  is `#[serde(tag="type", content="payload")]` over `session_meta | response_item | turn_context |
  compacted | event_msg | world_state | inter_agent_*` (`protocol.rs:3193`). `turn_context` carries
  per-turn cwd/model/approval/sandbox (resume reads the latest to recover cwd); `compacted` records
  the summary + `replacement_history` inline so resume reconstructs collapsed context. Writer =
  background Tokio task, bounded mpsc, `to_string(line)+'\n'` then flush; resume reopens for append
  and **guarantees newline-termination** (`ensure_rollout_is_newline_terminated`, `recorder.rs:1850`).
- **Claude** (`sessionStorage.ts`): path `projects/<sanitizePath(realpath cwd)>/<uuid>.jsonl`,
  mode `0600`/dir `0700`. `sanitizePath` = `[^a-zA-Z0-9]→-` (`sessionStoragePortable.ts:311`), lossy +
  non-invertible → needs a realpath+hash **prefix-scan fallback** (`findProjectDir`). Each line is a
  typed envelope wrapping the **verbatim Anthropic message** + `{uuid, parentUuid, sessionId,
  timestamp, cwd, gitBranch, version, …}`; **one API turn fans out to N lines** (thinking/text/tool_use
  each own a line sharing `message.id`); `tool_use`↔`tool_result` pair across an assistant line and a
  `type:"user"` line. `parentUuid` makes the transcript a **tree** (branch/rewind). Control records
  (`mode`, `ai-title`, `last-prompt`, `file-history-*`, `system/compact_boundary`) interleave in the
  same stream; on resume everything before the last `compact_boundary` is dropped unless it carries a
  `preservedSegment`.

**B. Dir-per-session + `summary.json` index (Grok).** `sessions/<urlenc-cwd>/<uuidv7>/` with a
`.cwd` sidecar when the encoded name would exceed 255 bytes (`paths.rs:112-149`). The dir holds many
streams: `updates.jsonl` (authoritative ACP conversation for `/resume`), `chat_history.jsonl` (model
messages, `chat_format_version`-tagged), `rewind_points.jsonl`, `signals.json` (~60 telemetry
counters), `events.jsonl`, `system_prompt.txt`, `prompt_context.json`, plus `compaction_checkpoints/`,
`subagents/`, `prompts/`. The **`summary.json`** (`persistence.rs:786-870`) is the picker index — its
presence marks a dir resumable; fields = the resume-picker spec:
`info{id,cwd}`, `session_summary`/`generated_title`/`title_is_manual`, `created_at`/`updated_at`/
`last_active_at` (advanced only by real content appends; picker sorts on `last_active_at ?? updated_at`),
`num_messages`, `current_model_id`, `agent_name` (the harness/agent def, so resume doesn't depend on
the mutable model catalog), `reasoning_effort`, `sandbox_profile`, `git_root_dir`/`git_remotes`/
`head_commit`/`head_branch`, `parent_session_id`/`forked_at`/`session_kind`/`inherited_prefix_len`,
`hidden`. **Durability patterns worth stealing** (§3.4).

### 3.3 The SQLite question (opencode + Codex)

- **opencode v2** is a real CQRS/event-sourcing DB (`opencode.db`, Drizzle ORM, 38 migrations):
  `event`/`event_sequence` log + `session`/`session_message`/`session_input` **projections rebuilt
  from it**. It buys indexed listing/search/pagination over thousands of sessions, cascade deletes,
  branching (`parent_id`), and **live event streaming to many concurrent clients** (TUI + web +
  desktop + Slack). It costs an ORM, 38 forward-only migrations, a projection engine, and an opaque
  store (message bodies are JSON crammed into `data` TEXT columns → debug via `sqlite3` + JSON
  extraction, not `cat`/`grep`/`jq`). opencode itself **shipped both** v1 (one JSON file per record)
  and v2 — the living A/B.
- **Codex** keeps JSONL **authoritative** and adds `state_*.sqlite` as a **derived, rebuildable
  index** (DB-first listing with a filesystem read-repair/reconcile fallback, `recorder.rs:424`).

### 3.4 Durability patterns (independent of shape — copy these)

- **Torn-tail healing** (Grok `jsonl/mod.rs:225-251`; Codex's newline-guarantee): appends aren't
  crash-atomic; before appending, seek to the last byte and prepend `\n` if it's missing → bounds
  damage to exactly one line that lenient readers skip. **Readers are corruption-tolerant by design.**
- **Full-file rewrites are crash-atomic** via temp-file + rename (Grok `write_jsonl`).
- **Index patched, never snapshot-rewritten** (Grok `summary_write.rs`): a `SummaryPatch` + exclusive
  `flock` on a never-renamed sidecar lock spans the whole read-modify-write; monotonic fields
  (`last_active_at`, counters) never regress. A lock-free whole-struct RMW lost concurrent updates.
- Sibling `.lock` files (advisory `flock`) on every mutable file; `0600` on anything with secrets.

### 3.5 → `~/.locode` trace recommendation

**Recommended: single append-only JSONL rollout (Codex model) with a small sidecar meta, JSONL
authoritative, no DB for v0.** Rationale:

- Our engine is a **headless core + one TUI** — not opencode's many-client server. We have neither
  the concurrency nor the cross-session-search pressure that justifies SQLite's 38-migration weight.
  opencode's own v0 was files; Codex/Claude ship JSONL rollouts with no DB on the hot path.
- The dominant ops are **append a turn** and **replay one session in order** — a perfect JSONL fit;
  it maps 1:1 onto our sample→dispatch→append loop and the `tool_use`→`tool_result` pairing invariant
  (the transcript literally *is* the appended history).
- JSONL stays **greppable/jq-able**, which matters enormously while the format churns (opencode needed
  5 `session_*` migrations in one month to settle its model).

Concretely:
- **Path (user decision 2026-07-24 — Claude's structure, with a bijective encoding):**
  `~/.locode/sessions/<encoded-cwd>/rollout-<timestamp>-<sessionId>.jsonl`. Grouping by cwd makes
  `--continue` an O(1) "list one directory, take newest" (Claude's win) instead of a date-tree scan
  with per-file header reads or a SQLite index (Codex's cost). The earlier draft preferred Codex's
  date buckets *because* Claude's encoding is lossy — that objection is fixed by making the encoding
  a **bijection** instead (spec below; implement with the persistence P0):
  - `/` → `+` (readable separator); literal `+` → `%2B`; literal `%` → `%25`; **everything else
    verbatim** (non-ASCII preserved — Claude's `[^a-zA-Z0-9]`→`-` collides `foo-bar`/`foo_bar`/
    `foo.bar`/`foo/bar` and any two same-length CJK names; ours collides nothing).
  - Fully reversible: decoding recovers the real cwd from the dirname, so a resume
    picker needs no file reads (grok's URL-encode benefit, prettier names).
  - Over the 255-byte filename limit: a stable `<fnv1a64-hex16>-<tail>` fallback (starts with a hex
    digit, never `+`, so it is never mistaken for a decodable name) + a `.cwd` sidecar written by
    the store (grok's scheme). Callers pass a canonicalized cwd.
  The `rollout-<timestamp>-<id>.jsonl` filename keeps reverse-chron name-sorting within the dir and
  the id in the name for `--resume` scans.
- **Line 1 = SessionMeta header:** `{session_id, parent_id?, timestamp, cwd, git:{root,branch,head,remote},
  cli_version, model, provider, harness, base_instructions?}` — grok's `summary.json` field set shows
  what a resume picker needs; codex's SessionMeta shows the header form. Carry `harness`/`agent_name`
  so resume rehydrates the right pack independent of the model catalog.
- **Each other line:** `{timestamp, type, payload}` internally-tagged union, `type ∈ session_meta |
  user_message | assistant_message | tool_use | tool_result | turn_context | compacted`. Keep the
  variant set small; include `turn_context` (per-turn cwd/model) and `compacted` (summary +
  replacement history) so resume survives compaction — both proven load-bearing in Codex.
- **`--continue`** = list the current cwd's encoded dir, take the newest rollout (O(1) scoping —
  the point of the cwd-in-path structure); **`--resume <id>`** = check the cwd dir first, then a
  global scan across cwd dirs for `*-<id>.jsonl` (Claude's scoped-then-global resolver). A
  `parent_id` on the header gives fork/branch lineage cheaply.
- **Durability:** newline-guarantee on reopen + torn-tail-tolerant reader (Codex+Grok); write under a
  sibling `.lock`; `0600`.
- **Listing index:** for v0, scan headers (line 1 only) — cheap enough for a personal history. If a
  fast picker over thousands of sessions is later needed, add a **rebuildable** SQLite index à la Codex
  (never authoritative), *not* opencode's event-sourced DB.
- **Separate `history.jsonl`** for cross-session composer recall (`{session_id, timestamp, text}`, `0600`,
  append+flock) — do **not** conflate it with the transcript (both Codex and Claude keep them apart).

---

## 4. Auth + the config/auth split

Every harness keeps secrets **out of the hand-edited config**, in a `0600` file the app rewrites
under a lock:

- **Claude:** `~/.claude.json` — a *sibling* of the dir, not inside it (`env.ts:14`). A monolithic
  `GlobalConfig` blob: OAuth/auth, per-project `{allowedTools, mcpServers, hasTrustDialogAccepted,
  lastSessionId, usage}`, `numStartups`, onboarding. Rewritten constantly under a lock with a re-read
  guard that **refuses to wipe auth** (`config.ts:1219`). *Opposite edit model to `settings.json`.*
- **Codex:** `~/.codex/auth.json` (`0600`): `{auth_mode, OPENAI_API_KEY?, tokens{id,access,refresh,
  account_id}, last_refresh}`; keyring option via a `keyring-store` crate.
- **Grok:** `~/.grok/auth.json` (`0600` + `.lock`), keyed by `"{oidc_issuer}::{client_id}"` → OIDC
  device-flow tokens `{auth_mode, key(JWT), refresh_token, user_id, email, team_id, tier, expires_at}`.
  API-key auth comes via `XAI_API_KEY` env instead.
- **opencode:** `~/.local/share/opencode/auth.json` (provider→`{type, key/refresh/access}`) + a
  separate `mcp-auth.json`; **also** stores `access_token`/`refresh_token` in the SQLite
  `account`/`credential` tables — secrets in *two* places, a wart to avoid.

**→ `~/.locode`: no `auth.json` — env-only (user decision, 2026-07-24).** The machine-managed
auth file exists *because OAuth refresh tokens rotate* (the app must rewrite it); a static API key
doesn't rotate, so grok's API-key path is the model: keys come from env (`ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, `XAI_API_KEY`, …), read once at process start by the ProviderRegistry factories —
no file, no sync loop. Boundary: an `auth.json` (0600, separate from `settings.json`, never in the
trace) becomes necessary only if an OAuth/token-refresh flow ever lands; a convenience
`locode auth set` one-shot write can be added then. The general lesson stands: keep machine-managed
state (auth, trust, usage) apart from hand-edited settings — Claude's two-artifact split is the
clearest statement of this.

---

## 5. Skills on disk (see `harness-study-skills.md` for the format deep-dive)

All four: a **`SKILL.md` directory** (dir-name = id), YAML frontmatter (`name`, `description`, +
`when-to-use`, `allowed-tools`, `argument-hint`, `model`, `effort`, …), **discovered cwd→repo-root→user**
(deeper wins), dedup by realpath/name, **progressive disclosure** (only name+description up front, body
on invocation), **two orthogonal invocation switches** (`user-invocable` default true;
`disable-model-invocation` default false). Roots:

- **Claude:** `~/.claude/skills/<name>/SKILL.md` (user) + `<dir>/.claude/skills/` (project walk to git
  root) + managed + plugins + legacy `~/.claude/commands/` (`loadSkillsDir.ts`).
- **Grok:** `./.grok/skills` > `<repo>/.grok/skills` > `~/.grok/skills` + `.agents/skills/` at each
  tier + **cross-harness compat roots** `~/.claude/skills`, `~/.cursor/skills` (toggle-gated). Bundled
  under `~/.grok/bundled/skills`; flat `commands/*.md` = slash commands; subagent packs under
  `~/.grok/bundled/{agents,roles,personas}/`.
- **Codex:** project `<repo>/.codex/skills` + `.agents/skills`; user `$CODEX_HOME/skills` (deprecated)
  + `$HOME/.agents/skills`; **system bundled** cached at `$CODEX_HOME/skills/.system` with a marker;
  admin `/etc/codex/skills`. The **`allow_implicit_invocation` gate lives in a sidecar
  `agents/openai.yaml`**, *not* in `SKILL.md` frontmatter (`core-skills/src/loader.rs:112-141`) — a
  Codex-specific split. **Codex dropped user `~/.codex/prompts/` entirely — custom prompts are now
  skills.**
- **opencode:** `{skill,skills}` dirs + a `skills` config array + a **remote skill fetcher** (GET
  `index.json` → download into `~/.cache/opencode/skills/<name>/` with a version marker). Commands
  `{command,commands}/**/*.md`, agents `{agent,agents}/**/*.md`. Frontmatter parser has a **permissive
  fallback** *because "claude code allows invalid yaml in their frontmatter"* (`markdown.ts:16`) — an
  interop note if `~/.locode` reads Claude skill files.

**→ `~/.locode`:** `~/.locode/skills/<name>/SKILL.md` (user) + `<repo>/.locode/skills/` (project), the
same frontmatter start-set and two-switch gate; roots line up 1:1 with the settings layers. Cleanest
loader model = **Claude's plain dir scan** (`loadSkillsDir.ts`). Per ADR-0023 the loader + any listing
injection is a **single shared-engine implementation**, not per-pack; the injection is a `User`-role
`<system-reminder>`. Consider grok's **compat roots** (read `~/.claude/skills`) so existing skills work
day one. Defer the `allow_implicit_invocation`-in-sidecar (Codex) and the remote fetcher (opencode).

---

## 6. Everything else the folders carry (worth knowing, mostly defer)

- **AGENTS.md from home:** Codex loads `$CODEX_HOME/AGENTS.override.md` then `$CODEX_HOME/AGENTS.md`
  as global user instructions; Grok scans a neutral filename list (`AGENTS.md`, `CLAUDE.md`, …) cwd→root
  and re-injects on compaction. **We already ship this** (ADR-0023 v1, correction 2026-07-24 — an
  earlier draft of this doc wrongly listed it as deferred): `locode-host/src/instructions.rs` loads
  `~/.locode/AGENTS.md` (+ same-dir `AGENTS.override.md`) as the lowest-precedence layer, honoring
  `$LOCODE_HOME` (ADR-0023 amendment 2026-07-24).
- **Workspace trust:** Codex `[projects."<abs>"]` table; Grok `trusted_folders.toml`; Claude
  `hasTrustDialogAccepted` per project — a first-run "trust this workspace?" gate, persisted.
- **MCP:** always config-declared (a `[mcp_servers]` TOML table / `mcp` JSON key), plus project-scoped
  overrides; Claude also has per-project MCP in `~/.claude.json` + a committed `.mcp.json`.
- **Model catalog cache** (`models_cache.json` / `models_cache.json` / `models_cache.json`) for offline
  start; **version/update-check** sidecars; **shell snapshots** (Codex `shell_snapshots/`, Claude
  `shell-snapshots/`) so exec inherits the user's shell; **file-history** content-addressed backups for
  edit undo (Claude); **active-sessions registry** (Grok `active_sessions.json`) for concurrency.
- **XDG split** (opencode): durable data in `~/.local/share`, ephemeral locks/pids in `~/.local/state`,
  downloads/skills in `~/.cache`. Clean, but a single `~/.locode/` is simpler for v0.

---

## 7. Recommended `~/.locode` layout (synthesis)

```
~/.locode/                              # $LOCODE_HOME override; memoized; auto-created
├── settings.json                       # user config (JSON); < project .locode/settings.json < .local < --settings
├── AGENTS.md (+ AGENTS.override.md)     # home-level global instructions — SHIPPED (ADR-0023)
├── sessions/
│   ├── <encoded-cwd>/                                    # bijective cwd encoding (§3.5):
│   │   │                                                 # `/`→`+`, `+`→`%2B`, `%`→`%25`, rest verbatim
│   │   ├── rollout-<timestamp>-<sessionId>.jsonl         # authoritative trace; line1=SessionMeta,
│   │   │                                                 # then {timestamp,type,payload} records
│   │   └── .cwd                                          # sidecar, only for >255-byte hash-fallback dirs
│   └── history.jsonl                    # cross-session INPUT history {session_id,timestamp,text} (0600)
│                                        # — the composer's up-arrow/reverse-search recall (shell-history
│                                        # analog), NOT the transcript; needed only once the TUI does recall
├── skills/<name>/SKILL.md               # user skills; project skills live in
│                                        # <repo>/.agents/skills/ (ADR-0025 §2 amendment
│                                        # 2026-07-24 — the cross-agent tree)
└── (defer) trusted_folders / models_cache / version / logs / a rebuildable sessions index
```

No `auth.json`: env-only until an OAuth/refresh flow exists (§4, user decision 2026-07-24).

**The three load-bearing decisions:**
1. **Config = layered JSON** (user<project<local<flag) with **array-union merge** for permissions and a
   **project-layer denylist** for endpoint/model/exec-redirecting keys. One serializer (no TOML).
2. **Trace = single append-only JSONL rollout per session**, date-bucketed, SessionMeta line 1 + a small
   tagged-union record set, JSONL authoritative, torn-tail-tolerant, `--continue`=newest-for-cwd /
   `--resume <id>`=scan; **no SQLite for v0** (add a rebuildable index only when a fast picker over huge
   history is proven necessary — never opencode's event-sourced DB for a single-client agent).
3. **Skills = `SKILL.md` dirs** (user + project roots mirroring the settings layers), Claude's plain-scan
   loader, two-switch invocation gate, **shared-engine** (ADR-0023) not per-pack; optional Claude compat
   root.

**Resolved with the user (2026-07-24):**
- **No `auth.json`** — env-only until OAuth/token-refresh ever lands (§4).
- **Home-level `AGENTS.md`** — already shipped (ADR-0023); `$LOCODE_HOME` honored by amendment.

**Open questions to confirm with the user:**
- JSON vs TOML for `settings.json` (recommend JSON — one serializer; deviates from the Rust harnesses).
- Single `~/.locode/` vs an XDG split (recommend single dir for v0 simplicity).
- Whether to read `~/.claude/skills` as a compat root (recommend yes — cheap day-one skill reuse).

---

*Method: one deep source + on-disk read per harness (subagents, 2026-07-24), then cross-comparison.
This supersedes the compacted first-pass notes captured in the `home-dotfolders-research` session
memory. Feeds the two P0s (skills; settings + trace persistence) and a future `~/.locode` ADR.*
