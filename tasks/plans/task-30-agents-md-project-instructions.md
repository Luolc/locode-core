# Task 30 — Shared `AGENTS.md` project-instruction loading (ADR-0023): implementation plan

**Status:** shipped 2026-07-23 (6 slices, PRs #146–150; see the Result addendum and
[`../tracker.md`](../tracker.md)). Design of record: [ADR-0023](../../docs/decisions/ADR-0023-fidelity-boundary-and-agents-md-loading.md).
Ships the first instance of the shared context machinery ADR-0023 defines. Source-grounded
against the `coding-cli-survey` submodules and the two studies
([agents-md](../../docs/research/harness-study-agents-md.md),
[cli-args](../../docs/research/harness-study-cli-args.md)); local seams mapped 2026-07-23.

## The one-paragraph shape

A repo has an `AGENTS.md` (maybe several, one per directory up to the git root). When a
session starts, the engine asks a **shared loader in `locode-host`** to walk `cwd → project
root`, collect the `AGENTS.md` files (deepest wins), and hand back a neutral
`ProjectInstructions`. The engine renders that into **one `User`-role `<system-reminder>`
message** — authority preamble, per-file `## From: <path>` sections in root→cwd order, a
"deeper wins" note, a relevance disclaimer, all under a byte budget — and injects it into the
conversation once at session init, re-scanning each turn to re-emit a replace/remove banner if
the files changed. It is **shared** (not pack-selectable): every `--harness` gets identical
behavior (ADR-0023 §1). Default-on in the binaries, with `--no-project-instructions` to skip.

## What already exists (this is wiring + one new host module, not greenfield)

| Piece | Where | Use |
|---|---|---|
| Single OS seam, no `Host` trait — concrete `Host` struct, `tokio::fs` inside | `locode-host/src/lib.rs:1-8`, `fs.rs` | loader is a method on `Host`; **reads directly** (host *is* the seam) |
| `find_git_root(start) -> Option<PathBuf>` — upward `.git` walk, **private, `.git`-hardcoded** | `locode-host/src/walk.rs:70-79` | model the ancestor walk on it; generalize + make usable by the loader |
| `Host::read_file(cwd, path) -> FileRead{contents:String, stat:FileStat}` (lossy UTF-8, **jailed**) | `locode-host/src/fs.rs:59-71` | *not* used by the loader (jail is cwd-rooted — see Key decision 1); loader reads directly |
| `FileStat { len, modified }` | `locode-host/src/fs.rs:19-25` | per-turn change-detection token (Slice 4) |
| `Host::is_path_ignored(cwd, path)` — gitignore check, false outside a repo | `locode-host/src/walk.rs:156-185` | skip a gitignored `AGENTS.md` (Slice 1) |
| `Conversation/Message/Role/ContentBlock`; `Role::User`, `ContentBlock::Text{text}` | `locode-protocol/src/lib.rs:25-116` | build the injected message as a struct literal (no builders exist) |
| Engine turn assembly — Init gated `turns_run==0`; first user msg pushed | `locode-engine/src/run.rs:29-54` | **the injection hook point** (after Init, before the user prompt push) |
| `EngineConfig { cwd, workspace_root, … }` | `locode-engine/src/config.rs:10-65` | already carries `cwd` (walk start); add an `instructions` sub-config |
| `Session::new(provider, registry, preamble, config, sink)` + `with_approver` builder | `locode-engine/src/session.rs:52-80` | add a `with_host(Arc<Host>)` builder (non-breaking) |
| Pack preamble = `[System(prompt), User(user_info)]` | `locode-packs/src/grok/mod.rs:48-69` | confirms injection sits **after** the preamble, added by the engine; packs never touch it |
| Headless build (`Host` Arc already here) | `locode-exec/src/run.rs:34-133` | pass `Arc<Host>` + `instructions` config into the Session |
| TUI build (near-duplicate) + `Cli::to_headless` field-by-field | `locode-tui/src/engine.rs:129-198`, `cli.rs:73-85` | **both** build paths + the mapping must change together |
| `session_with` / `CapturingProvider` (records request `messages`) | `locode-engine/src/lib.rs:128-141,954-995` | assert the injected message lands in the request |
| Full-stack: grok pack + mock wire | `locode-tui/tests/engine_task.rs` | e2e harness pattern (Slice 6) |
| Binary-level headless (`locode -p --api-schema mock`) | `locode-app/tests/headless.rs`, `locode-exec/tests/cli.rs` | binary e2e (Slice 5) |

## Key decisions (resolved before coding)

1. **The loader reads directly within `locode-host`, bypassing the tool path-jail.** The jail
   is rooted at **cwd** (`EngineConfig.cwd == workspace_root == canonical cwd`,
   `locode-exec/src/run.rs:98-99`; jail root `locode-host/src/lib.rs:102`), but instruction
   discovery legitimately spans **cwd → git root**, i.e. *ancestors above the jail root*. Going
   through the jailed `read_file` would reject every ancestor file. Resolution: the loader lives
   in `locode-host` (the trusted OS seam — ADR-0008) and reads the discovered files directly
   (`tokio::fs`), bounded to the `AGENTS.md`/`AGENTS.override.md` names along the bounded
   ancestor walk. ADR-0008's jail governs **tools**; the loader is engine machinery, not a tool.
   *(ADR-0023 gets a one-line implementation note recording this; see "ADR reconciliation".)*

2. **Engine gains a `Host` handle via a `with_host` builder.** ADR-0023's per-turn rescan needs
   live loader access, so `Session` stores `Option<Arc<Host>>` (None → loading skipped, keeping
   every existing hostless unit test green). `EngineConfig` gains an `instructions:
   InstructionsConfig` sub-config.

3. **Re-injection appends a banner message; it never mutates prior history.** `history` persists
   across `run()`s (ADR-0016) and the transcript is immutable. On a mid-session change the loader
   appends a *new* `User` message with a "these replace previously provided instructions" banner
   (Codex/opencode's pattern — `codex: context/world_state/agents_md.rs:9-11`;
   `opencode: instruction-context.ts:36-37`), tracking a last-injected content hash on `Session`.

4. **`--add-dir` is deferred to a later task (seam only here).** Its instruction-loading half is
   feasible now, but its **other** half — widening the tool path-jail so tools may access the
   extra dirs — is an ADR-0008 security-posture change (the code map's "largest structural
   change": `Host` holds a *single* `workspace_root`). Shipping a flag that widens instructions
   but not tool access is incoherent, and the user has said `--add-dir` will ultimately live in
   `settings.json` (CLI-overrides-settings), which is unreviewed. So the loader supports an
   `extra_roots: Vec<PathBuf>` **config seam** (complete + unit-tested), but **no `--add-dir` CLI
   flag** lands until the jail-widening task. (User-flagged prerequisite.)

5. **`root_stop_pattern` regex is a dormant seam.** Per the user, the real implementation waits
   for `settings.json`. The `InstructionsConfig` field exists and is plumbed, but root detection
   currently uses `.git` markers + the cwd-only fallback only; the regex branch is a documented
   `TODO(settings)` and needs the `regex` crate (not a current dep — confirmed) when wired.
   (User-flagged prerequisite.)

6. **Global `~/.locode/AGENTS.md` is included** (lowest precedence). Home resolution is a
   dependency-free `std::env::var_os("HOME")` join (macOS/Linux — the shipped targets); the
   loader reads it directly (Decision 1 already covers out-of-repo reads). Gated by a
   `global_file: bool` config (default `true`); tests point `HOME` at a tempdir for determinism.
   Scanned **first** so it is lowest priority and the deepest project file still wins
   (`grok: agents_md.rs:88,124` scans home first).

7. **The disable flag is `--no-project-instructions`, not `--bare`.** Honest scope — it turns off
   exactly this one behavior — and it leaves the broader `--bare` name free for the future
   atomic "skip all startup side-effects" flag (cli-args study).

8. **Injection order:** a standalone `User` message inserted **after** the pack preamble
   (`[System, User(user_info)]`) and **before** the user's actual prompt. Consecutive `User`
   messages are valid; `repair_pairing` only touches tool_use/tool_result (`run.rs:66`), so a
   plain text message is safe.

### What v1 explicitly does NOT do (recorded seams / out of scope)
- `root_stop_pattern` matching (dormant, Decision 5) · `--add-dir` / `extra_roots` CLI + jail
  widening (Decision 4) · `@import` + rules dirs (ADR-0023, rejected) · "re-inject after
  compaction" (no compaction seam exists yet).

---

# Slice 1 — the loader in `locode-host`: types + walk + discovery + dedup

**Goal:** a pure, unit-tested `Host::load_project_instructions(cwd, cfg) -> ProjectInstructions`
that produces ordered, deduped entries from the `cwd→root` chain. No engine, no rendering.

### 1a. New module `locode-host/src/instructions.rs`
Public types (re-exported from `lib.rs`):
```rust
pub struct ProjectInstructions { pub entries: Vec<InstructionEntry> }
pub struct InstructionEntry { pub source_path: PathBuf, pub content: String }

pub struct InstructionsConfig {
    pub enabled: bool,                    // default true
    pub byte_budget: usize,               // default 64 * 1024; 0 = unbounded off (Slice 2 applies it)
    pub root_markers: Vec<String>,        // default [".git"]
    pub root_stop_pattern: Option<String>,// DORMANT seam (Decision 5) — not matched in v1
    pub extra_roots: Vec<PathBuf>,        // seam (Decision 4) — honored by the walk, no CLI flag
    pub global_file: bool,                // default true — read ~/.locode/AGENTS.md (lowest priority)
}
impl Default for InstructionsConfig { /* the above */ }
```

### 1b. Root detection + ancestor walk
`fn discover_root(start, markers) -> RootKind` — ascend from `start`; the **nearest** ancestor
containing any `root_markers` entry is the root (default `.git`; generalize `walk.rs:70-79`).
No marker up to the FS root ⇒ **cwd-only** (return `start`). FS root is the hard backstop. The
`root_stop_pattern` branch is a `TODO(settings)` stub. Reference behavior:
`codex: agents_md.rs:172-187` (marker walk), `:141-143` (cwd-only outside a repo).

### 1c. Per-directory file discovery (override-first-match-wins)
For each dir root→cwd: probe `AGENTS.override.md` then `AGENTS.md`; take the **first that
exists** — same-directory *replacement*, not additive (`codex: agents_md.rs:211-217`, verified
2026-07-23). Read the chosen file directly (Decision 1).

### 1d. Assemble + dedup + gitignore
Order: **global** `~/.locode/AGENTS.md` first (lowest priority, when `global_file` and it
exists — home via `std::env::var_os("HOME")`, Decision 6), then the primary chain **root→cwd**
so the deepest file is last (wins on conflict — `grok: agents_md.rs:121 "CRITICAL: Reverse"`),
then each `extra_roots` entry's own root→dir chain, appended after (Decision 4). Dedup by a
**canonical key** (`std::fs::canonicalize` + lowercased for case-insensitive FS — new helper).
Skip gitignored files via `Host::is_path_ignored` (`walk.rs:156-185`; global/extra-root files
are outside a repo → not ignored). Empty/whitespace-only files dropped.

### Slice 1 test matrix
| # | Test (tempdir) | Asserts |
|---|---|---|
| 1 | git repo, `AGENTS.md` at root + subdir, cwd=subdir | 2 entries, **root→cwd** order, correct paths |
| 2 | no `.git` anywhere, `AGENTS.md` at cwd + parent | **cwd-only**: 1 entry (cwd), parent ignored |
| 3 | dir has both `AGENTS.override.md` + `AGENTS.md` | override **replaces**; sibling `AGENTS.md` absent; other dirs unaffected |
| 4 | symlinked/duplicate path reachable two ways | dedup → single entry |
| 5 | `AGENTS.md` gitignored | skipped |
| 6 | empty + whitespace-only `AGENTS.md` | dropped |
| 7 | `extra_roots` = a second tempdir with its own `AGENTS.md` | appended after primary chain, labeled by path |
| 8 | `enabled=false` | empty `ProjectInstructions` |
| 9 | `root_stop_pattern=Some(..)` provided | currently a no-op (documents the dormant seam; guards against accidental activation) |
| 10 | `HOME`=tempdir with `~/.locode/AGENTS.md` + a project `AGENTS.md` | global entry present, **first** (lowest priority); `global_file=false` omits it |

**Merge gate:** all four `just check` commands; no engine/CLI change yet.

---

# Slice 2 — assembly/render: `ProjectInstructions` → `User` `<system-reminder>` message

**Goal:** pure rendering `render_instructions(&ProjectInstructions, budget) -> Option<Message>`
in a new `locode-engine/src/instructions.rs` (engine owns protocol rendering; ADR-0023 §2
"engine … injects the neutral value"). `None` when there are no entries.

### 2a. Envelope (best-of, one format for all packs — ADR-0023 §2 Injection)
A `Role::User` message, single `ContentBlock::Text`:
```
<system-reminder>
As you answer the user's questions, you can use the project instructions below (deeper
directories take precedence on conflict). They are context, not a message to answer.

## From: <source_path>
<content>

## From: <source_path>
<content>
</system-reminder>
```
Preamble adapts Claude's framing (`claude-code: api.ts:461-473`); per-file `## From:` +
deeper-wins note adapt Grok's (`grok: agents_md.rs:194-227`).

### 2b. Byte budget (Decision, 64 KiB)
Truncate the assembled body at `byte_budget` bytes (UTF-8 boundary-safe) and append a
`\n…[truncated]…` marker; `0` = unbounded. Codex's discipline
(`codex: agents_md.rs:95-130`).

### Slice 2 test matrix
| # | Test | Asserts |
|---|---|---|
| 1 | 2 entries | exact envelope string: preamble, both `## From:` sections in order, disclaimer, tags |
| 2 | empty | returns `None` |
| 3 | body > budget | truncated at ≤ budget, marker present, valid UTF-8 boundary |
| 4 | `Role` / block shape | message is `Role::User` with one `ContentBlock::Text` |

---

# Slice 3 — engine wiring: thread `Arc<Host>`, inject once at init

**Goal:** with a host present, the engine loads + renders + injects the message once per session.

### 3a. `Session` + `EngineConfig`
- `EngineConfig { …, instructions: InstructionsConfig }` (additive; `Default` = enabled).
- `Session { …, host: Option<Arc<Host>> }` + `with_host(mut self, Arc<Host>) -> Self` (mirrors
  `with_approver`, `session.rs:77-80`). Default `None`.

### 3b. Injection at the hook point (`run.rs:47-54`)
Inside `drive()`, after the `turns_run==0` Init block and before the user-prompt push: if
`host.is_some() && config.instructions.enabled`, `host.load_project_instructions(cwd, cfg)` →
`render_instructions(..)` → if `Some(msg)`, push into `self.history` and emit `Event::Message`.
For Slice 3, gate on `turns_run==0` (once per session).

### 3c. Facade
Re-export `ProjectInstructions`, `InstructionEntry`, `InstructionsConfig` from `locode-core`.

### Slice 3 test matrix
| # | Test (engine + `CapturingProvider` + real `Host` over a tempdir) | Asserts |
|---|---|---|
| 1 | tempdir with `AGENTS.md`, one `run()` | request `messages` contain the `User` `<system-reminder>` with the content, positioned after preamble / before the prompt |
| 2 | two `run()` calls | injected **once** (not duplicated on turn 2) |
| 3 | `instructions.enabled=false` | absent |
| 4 | no host (existing pattern) | absent; all pre-existing engine tests unchanged |
| 5 | tempdir with **no** `AGENTS.md` | absent (loader returns empty → `None`) |

---

# Slice 4 — per-turn rescan + diff refresh (replace/remove banners)

**Goal:** edits to `AGENTS.md` mid-session take effect without a restart, idempotently.

### 4a. Mechanism
- `Session` stores `last_instructions: Option<u64>` (content hash of the last injected body).
- Move the Slice-3 injection to run **every** `drive()` (not just `turns_run==0`). Compute the
  new body hash: unchanged → do nothing; changed → append a `User` message whose envelope opens
  with *"These instructions replace all previously provided project instructions."*; entries
  gone → append a `User` remove banner (*"The previously provided project instructions no longer
  apply."*). Update `last_instructions`. Detection is a **per-turn rescan** — the bounded walk is
  cheap (ADR-0023 Refresh).

### Slice 4 test matrix
| # | Test | Asserts |
|---|---|---|
| 1 | unchanged file across 2 runs | no second injection |
| 2 | edit `AGENTS.md` between runs | second run appends a **replace** banner + new content |
| 3 | delete `AGENTS.md` between runs | second run appends a **remove** banner |
| 4 | hash stability | identical content → identical hash → idempotent |

---

# Slice 5 — turn it on in the binaries + `--no-project-instructions`

**Goal:** `locode -p` (and the TUI) load `AGENTS.md` by default; a flag disables it.

### 5a. Both build paths call `with_host` + set the config
- `locode-exec/src/run.rs` (`Arc<Host>` already at `:58`): `Session::new(..).with_host(host)`;
  set `EngineConfig.instructions.enabled = !cli.no_project_instructions`.
- `locode-tui/src/engine.rs::build_session` (`:129-198`): same (Host at `:145`).

### 5b. CLI flag in both structs + the mapping
- `locode-exec/src/cli.rs`: `#[arg(long)] no_project_instructions: bool`.
- `locode-tui/src/cli.rs`: same field **and** map it in `to_headless` (`:73-85`) — else `-p`
  won't see it.

### Slice 5 test matrix
| # | Test | Asserts |
|---|---|---|
| 1 | `locode -p --api-schema mock` in a tempdir with `AGENTS.md` (binary e2e, `locode-app/tests/headless.rs` pattern) | trace/report reflects the injected instructions |
| 2 | `+ --no-project-instructions` | not injected |
| 3 | clap parse (both `Cli`s) + `to_headless` round-trips the flag | field present, default `false` |

---

# Slice 6 — end-to-end + docs/ADR reconciliation

**Goal:** one full-stack proof and all records reconciled.

### 6a. Full-stack e2e (`locode-tui/tests/engine_task.rs` pattern: real grok pack + mock wire)
Nested tempdir git repo: `AGENTS.md` at root **and** a subdir; run a session from the subdir.
Assert the injected `<system-reminder>`: contains both, **root→cwd** order, `## From:` labels,
the deeper-wins note, and respects the byte budget. A second turn with an edited file shows the
replace banner (ties Slices 1–5 together).

### 6b. Docs / ADR
- `tasks/tracker.md`: flip **Task 30** to shipped in the archive; add a plan **Result** addendum
  here (what shipped vs the recorded seams).
- `locode-protocol/src/lib.rs:41-45`: fix the stale `Role::Developer` doc comment to match the
  ADR-0013 amendment (narrowed semantics).
- Confirm `SPEC.md`'s fidelity-boundary paragraph still reads true (it does).

### Slice 6 test matrix
| # | Test | Asserts |
|---|---|---|
| 1 | full-stack nested-repo run | complete envelope: both files, order, labels, note, budget |
| 2 | full-stack, edited file, 2 turns | replace banner path exercised end-to-end |

---

# Cross-cutting

- **Testing — strict and thorough** (user preference, 2026-07-23): exact-string envelope
  assertions (not "contains"), UTF-8-boundary truncation checked, dedup/gitignore/override
  corner cases each pinned, idempotence asserted by re-run. Tempdirs via the existing test
  utilities; no network. Every slice passes the full four-part gate before merge.
- **Ordering:** 1 → 2 → 3 → 4 → 5 → 6, each independently shippable and PR'd. Slices 1–2 are pure
  and side-effect-free; 3 adds the (opt-in) engine seam; 5 flips the default on.
- **No new dependencies.** (`regex` only enters with the future `root_stop_pattern` wiring.)
- **Autonomy / prerequisites:** Decisions 4 (`--add-dir`) and 5 (`root_stop_pattern`) are the
  two user-flagged prerequisites — both land as **seams**, not features, and are called out in
  the final PR summary.

## ADR reconciliation (done in the plan PR, before code — ADR-first)
Add a dated **implementation note** to ADR-0023 recording what planning revealed: (a) the loader
reads directly within `locode-host`, bypassing the tool jail, because discovery spans ancestors
above the cwd-rooted jail (Decision 1); (b) `--add-dir`/`extra_roots` is a seam deferred behind
the tool-jail-widening (ADR-0008) and settings work (Decision 4); (c) `root_stop_pattern` is a
dormant seam pending `settings.json` (Decision 5). None of these change ADR-0023's decisions —
they record scope/sequencing discovered in planning.

---

## Result (shipped 2026-07-23)

All six slices landed as planned; two deviations, both simpler than planned, are noted below.

- **Slice 1** (#146) — `locode-host::load_project_instructions` + `InstructionsConfig` /
  `ProjectInstructions` / `InstructionEntry`: the cwd→root walk, `AGENTS.override.md` override,
  global file, canonical dedup, gitignore filter. 13 unit tests.
- **Slices 2+3** (#147, **combined** — a render fn with no caller would be dead code): render to
  a `User` `<system-reminder>` + 64 KiB budget; engine injection once per session. 5 render + 3
  injection tests.
- **Slice 4** (#148) — per-turn rescan + replace/remove banners, content-hash idempotence,
  never mutating prior history. 3 unit + 3 integration tests.
- **Slice 5** (#149) — `--no-project-instructions` on both binaries + `to_headless`; the feature
  is on by default. 2 binary e2e tests.
- **Slice 6** (#150) — nested-repo full-stack e2e (root + subdir `AGENTS.md`, root→cwd order,
  labels), the `Role::Developer` doc-comment reconciliation (ADR-0013 amendment), tracker +
  this addendum.

**Deviations from the plan (simpler):**
1. **The loader is a free `fn` taking `cwd + cfg`, not a `Host` method** (it reads directly), so
   the engine needed **no `Arc<Host>` / `with_host` builder** — just the SPEC-declared
   `engine → host` dependency edge. This removed the plan's biggest structural wrinkle (the map's
   "engine has no Host handle" flag).
2. **The feature went live in the binaries at Slice 2+3** (they build `EngineConfig` via
   `..default()`, which is enabled), so Slice 5 was purely "add the disable flag + binary e2e",
   not "turn it on".

**Seams left dormant (as planned, recorded here):** `--add-dir` / `extra_roots` (config field
honored by the loader, no CLI flag — needs tool-jail widening, ADR-0008) and `root_stop_pattern`
(config field, matching is `TODO(settings)` — needs `settings.json` + the `regex` crate).
