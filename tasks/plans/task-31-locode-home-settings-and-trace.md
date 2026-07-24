# Task 31 — `~/.locode`: settings loading + resumable session trace (`--continue`/`--resume`)

> Implements [ADR-0024](../../docs/decisions/ADR-0024-locode-home-settings-and-traces.md)
> §1 (settings) + §2 (trace). Source grounding:
> [`../../docs/research/harness-study-home-dotfolders.md`](../../docs/research/harness-study-home-dotfolders.md)
> (the four-harness dossier; per-mechanism citations below are into it). Shared **engine
> machinery** (ADR-0023): identical for every `--harness`, nothing pack-visible.

## Objective

1. `~/.locode/settings.json` + project layers load and merge per ADR-0024 §1, providing
   durable defaults for `model` / `api_schema` / `harness`, activating
   `instructions.root_stop_pattern`, and parsing `skills.extra` (stored for the skills P0).
2. Every run appends an ADR-0024 §2 rollout trace; `--continue` resumes the newest
   session for the cwd and `--resume <id>` resumes by id — in both the headless CLI and
   the TUI.

## Design constraints (from the ADR — not re-litigated here)

- JSON only; flag > project-local > project > user; deep-merge with array-union;
  unknown keys preserved; project layers denylisted for `api_schema`-class keys.
- Trace: one append-only JSONL per session, `sessions/<encoded-cwd>/rollout-<ts>-<id>.jsonl`,
  bijective cwd encoding (`/`→`+`, `+`→`%2B`, `%`→`%25`, 255-byte hash fallback + `.cwd`),
  envelope `{timestamp, type, payload}`, line 1 `session_meta`, v1 types
  `message`/`turn_context`/`compacted`(reserved), readers tolerant (§2.4 rules 1–6).
- No new crate (crate boundaries are ask-first): the home/settings/trace code lives in
  **`locode-host`** (the trusted OS seam — it already owns instruction discovery and
  direct out-of-jail reads, same posture) with thin consumption in `locode-exec`/`locode-tui`.
- One approved new dependency: **`regex`** (ADR-0024 §1.4, for `root_stop_pattern` only).

## Slices

### S1 — home resolver + settings loader (M)

- `locode-host`: `locode_home()` — `$LOCODE_HOME` (set ⇒ must-exist + canonicalize, the
  Codex contract) else `$HOME/.locode`; memoized `OnceLock`; `default_locode_home()`
  split (grok `paths.rs:27-47`); auto-create on first *write* use (reads tolerate absence).
  Rehome the existing global-`AGENTS.md` resolver onto it (today it re-derives from env).
- `Settings` loader: read the five layers (user file, its **`extends` files** —
  user-layer-only pointers, list-ordered, non-recursive, resolved against the
  referencing file's dir, ADR-0024 §1.2 amendment — then `<repo>/.locode/settings.json`,
  `<repo>/.locode/settings.local.json`, `--settings <file|inline-json>`), each parsed to
  `serde_json::Value`; merge value-wise (objects deep, scalars overwrite, arrays
  concat+dedupe — Claude `settings.ts:529-547`); then decode a typed `Settings` view
  (`serde(default)`, unknown keys untouched because merging happens on `Value`).
  **Denylist**: `api_schema` (the v1 list) stripped from the two project layers before
  the merge, with a stderr warning naming the file (`extends` files merge with user
  trust — no denylist; a project-layer `extends` key is ignored with a warning). A
  malformed layer degrades to "skipped + warning", never a hard error (Claude's
  filter-not-reject).
- v1 fields: `model`, `api_schema`, `harness`, `instructions.root_stop_pattern`,
  `skills.extra` (parsed + validated: entry with `SKILL.md` ⇒ single skill; else must
  end in `skills` ⇒ folder; `~` expanded; invalid ⇒ warning + entry dropped).
- Wire into `locode-exec` + `locode-tui`: settings provide the *defaults*; an explicit
  flag/env always wins (`--harness`, `--api-schema`/`LOCODE_API_SCHEMA`; `model` has no
  flag — settings is its first home, threaded to the provider factory via
  `ProviderInit`).
- Tests: precedence across all five layers (incl. extends between user and project,
  list order within the layer); extends is user-layer-only (project `extends` ignored
  + warned); no recursion (nested `extends` ignored + warned); missing extends file
  degrades; array-union vs scalar overwrite; denylist strips project layers only
  (extends files exempt); unknown-key round-trip; malformed-layer degradation;
  `skills.extra` validation matrix; flag-beats-settings in exec.

### S2 — `root_stop_pattern` activation (S)

- Add the `regex` dependency (workspace-pinned, approved).
- `find_root` (instructions.rs): after the marker check per directory, test the
  compiled pattern against the directory's absolute path — match ⇒ that directory is
  the root (ADR-0023 rule 2). Compile once per load; invalid pattern ⇒ warning + seam
  stays dormant (never a hard error).
- Thread `Settings.instructions.root_stop_pattern` → `InstructionsConfig` in exec/tui.
- Tests: pattern stops the ascent above a marker; no-match ⇒ unchanged behavior
  (the existing dormant-noop test flips to active semantics); invalid pattern degrades.

### S3 — trace writer (M)

- `locode-host`: the bijective `encode_cwd_dirname`/`decode_cwd_dirname` (+ FNV-1a
  fallback + `.cwd` sidecar write) — the reviewed spec from ADR-0024 §2.1.
- `TraceWriter`: an `EventSink` **wrapper** — the engine already emits everything the
  trace needs (`Init` carries session id/model/preamble; `Message` events are the
  appended history), so tracing is a sink decoration, zero engine changes:
  `Init` ⇒ create dirs (`0700`)/file (`0600`), write `session_meta` (+ preamble
  `message` lines); `Message` ⇒ one `message` line; model/cwd change (future) ⇒
  `turn_context`. Newline-heal on reopen (grok `jsonl/mod.rs:225-251`); serialize line
  + `\n`, flush per record (codex's writer discipline).
- Wire into exec (`run.rs`: wrap the existing sink) and tui (engine bridge): tracing
  **on by default**, `--no-session-persistence`-style opt-out flag deferred until asked.
- Tests: file lands in the right encoded dir; line-1 meta fields; message lines are
  verbatim protocol `Message`s; reopen heals a torn tail; `0600`/`0700` modes; the
  reserved `compacted` type round-trips through the reader even though nothing writes it.

### S4 — `--continue` / `--resume <id>` (M)

- Reader in `locode-host`: stream a rollout, skip unparsable lines (torn tail),
  ignore unknown `type`s/fields (§2.4 rules — pinned by tests with future-shaped
  records), fold `compacted` (replace prior messages with `replacement_history`),
  return `(SessionMeta, Vec<Message>)`.
- Resolver: `--continue` ⇒ encode cwd, list that dir, newest rollout (skip
  `kind != "main"`); `--resume <id>` ⇒ cwd dir first, then all-cwd-dirs scan for
  `*-<id>.jsonl` (Claude's scoped-then-global).
- Seed the engine via the ADR-0016 continuity seam with the recovered history; resumed
  runs **append to the same file** (id and file continue — codex's reopen-for-append).
  `session_meta.harness` beats the settings default so a resumed session keeps its pack;
  a `--harness` explicitly passed alongside `--resume` errors on mismatch (no silent
  pack swap mid-transcript).
- CLI: `-c/--continue`, `-r/--resume <id>` on exec + tui (flag shapes follow Claude's).
- Tests: continue picks newest-in-cwd only; resume finds cross-cwd; unknown-kind
  sessions skipped by continue but resumable by id; harness mismatch errors; resumed
  file keeps appending (one file, valid pairing across the boundary); empty/corrupt
  file ⇒ clean "not found / unreadable" error, never a panic.

## Explicitly out of scope (tracked elsewhere)

Skills discovery/injection (the skills P0 consumes `skills.extra`); `history.jsonl`
(TUI recall); `cleanup_period_days` GC; a listing index; TUI resume *picker* (flags
only for now); `env`/`permissions` settings; subagent/workflow records (§2.4 reserves
them).

## Preset targets (gate for each slice + final)

- S1: `echo '{"harness":"claude"}' > ~/.locode/settings.json` (temp HOME) →
  `locode -p --api-schema mock "hi"` reports `harness:"claude"`; `--harness grok`
  still wins.
- S3: a mock run leaves `sessions/<enc>/rollout-*.jsonl` whose line 1 parses as
  `session_meta` and whose `message` lines reconstruct the run's transcript.
- S4: `locode -p --api-schema mock "hi"` then `locode -p --api-schema mock -c "again"`
  → the second report's `session_id` equals the first's and the file grew in place.
- Four-part gate (`fmt · clippy · test · doc`) green per slice; PR per slice,
  auto-merge on green.

## Result
_(filled per slice at merge — Phase 4)_
