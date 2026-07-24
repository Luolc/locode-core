# Task 20 · Slice 3 — `Edit` (read-before-edit + staleness gate)

> Faithful port of Claude Code's `FileEditTool` — exact string replacement guarded
> by the read-before-edit + modified-since-read gate (CC's signature guardrail; the
> deliberate divergence from grok, which has none). Source: submodule `6a25909`.
> Follows [`../../docs/claude-pack-dev-process.md`](../../docs/claude-pack-dev-process.md) (S3).

## Phase 0 — status analysis
- **Merged:** S1 (Bash + prompt), S2 (Read + `ClaudeSessionState` with `record_read`/
  `is_unchanged_read`).
- **Next unit:** `Edit` — the first tool that *consumes* the freshness store. Adds
  `check_fresh` + `record_write` to `ClaudeSessionState`. This is the highest-signal
  A/B tool vs grok (grok's `search_replace` has no gate).
- **Prereqs (hold):** store seam ✓; `Host::write_file` (returns post-write `FileStat`)
  ✓; `read_file` gives content + mtime + existence-via-error ✓.
- **Risks:** (1) exact check order + verbatim messages; (2) Edit *creates* files
  (source finding — plan §4.3 was wrong); (3) host doesn't mkdir parents.

## Phase 1 — source revisit (`6a25909`)
- **Schema** (`FileEditTool/types.ts:6-18`): `file_path`/`old_string`/`new_string`/
  `replace_all` (bool default false). `deny_unknown_fields`; `replace_all` type-strict.
- **Description** (`prompt.ts:8-28`) for our config (compact "line number + tab"
  prefix; non-ant → no minimal-uniqueness hint). Pinned `descriptions/edit.md`.
- **Check order** (`validateInput`, `:137-345`), verbatim messages:
  1. `old==new` (errorCode 1) — before touching the fs.
  2. permission deny (errorCode 2) → our `PathPolicy`/jail substitution (ADR-0008).
  3. size cap (errorCode 10, `MAX_EDIT_FILE_SIZE` 1 GiB) — not enforced (gap).
  4. nonexistent: empty `old_string` → **create** (`:216-220`); else errorCode 4
     ("File does not exist. Note: your current working directory is {cwd}.").
  5. existing + empty `old_string`: non-empty file → errorCode 3 ("Cannot create new
     file - file already exists."); empty file → replace.
  6. `.ipynb` → errorCode 5 ("File is a Jupyter Notebook. Use the NotebookEdit to
     edit this file.") — references a tool not in our pool (gap).
  7. **read gate** (errorCode 6): not in store → "File has not been read yet. Read it
     first before writing to it."
  8. **staleness** (errorCode 7): mtime > recorded read → "File has been modified
     since read, either by the user or by a linter. Read it again before attempting
     to write it." (CC's full-read content-fallback needs stored content — a gap; we
     store mtime only, sufficient on Linux/macOS.)
  9. not found (errorCode 8): "String to replace not found in file.\nString: {s}".
  10. multi-match without `replace_all` (errorCode 9): "Found {n} matches ...".
- **Creation** (`:216-220`, mkdir note `:427`): Edit creates via empty `old_string`;
  CC mkdirs parents. **Corrects plan §4.3** ("no file creation via Edit").
- **CRLF** (`:217`): normalize `\r\n`→`\n` for matching, re-expand on write (grok
  `search_replace` precedent).
- **Replacement** (`applyEditToFile`, `utils.ts:206-216`): `replace_all` → all; else
  the first (validated-unique) occurrence.
- **Success text** (`:583-593`): `replace_all` → "The file {p} has been updated. All
  occurrences were successfully replaced."; else "The file {p} has been updated
  successfully." (`userModified` note is interactive-only → empty headless.)

## Design
- `state.rs`: add `check_fresh(path, current) -> Option<bool>` (None=never read,
  Some(false)=stale, Some(true)=fresh) and `record_write(path, modified)` (offset=None,
  so Read's dedup never matches it — the origin distinction).
- `edit.rs`: `ClaudeEdit { host, state }`; `kind()=Edit`. `run()` implements the check
  order above; `write_result` writes, records post-write mtime, renders the success
  text. CRLF re-expansion via `reexpand`.
- `mod.rs`: register `Edit` sharing the store.

## Gaps (this slice)
- Host doesn't mkdir parents (ADR-0008) — creation needs an existing parent dir.
  **Batched question:** add a host mkdir-parents seam for Edit/Write creation?
- Quote-normalization (`findActualString`) — exact match only.
- 1 GiB size cap not enforced (host reads fully; un-hit).
- `.ipynb` message names `NotebookEdit` (not in pool) — kept verbatim (D8).
- Staleness content-fallback (Windows false-positive guard) omitted — mtime only.

## Test matrix
Schema golden; description pin; **gate:** unread → errorCode 6; read→edit ok +
sequential edit stays fresh; external modify → errorCode 7; old==new (1); not-found
(8); multi-match (9) + replace_all; create via empty old_string; empty old_string on
existing non-empty → errorCode 3. `state.rs`: `check_fresh` transitions; write-origin
never dedup-matches.

## Result (2026-07-24)
Shipped. `Edit` + the gate landed; `state.rs` gained `check_fresh`/`record_write`.
150 pack tests (11 new for Edit + gate) + full workspace suite pass; four-part gate
green; preset runs.

**Decisions/deviations (in-scope, autonomous):**
- **Edit creates files** (empty `old_string` + nonexistent) — corrects plan §4.3
  (amended there with a dated note). Host doesn't mkdir → creation needs an existing
  parent dir (documented gap; batched mkdir-seam question).
- CRLF-normalized matching + re-expansion (grok precedent).
- Exact-match only (quote normalization deferred); size cap not enforced; `.ipynb`
  message kept verbatim (names `NotebookEdit`); `#[allow(case_sensitive_file_extension_comparisons)]`
  to match CC's case-sensitive `.endsWith('.ipynb')`.

**Batched open question:** add a `Host` mkdir-parents seam so Edit/Write creation can
make parent dirs (CC does; our host deliberately doesn't, ADR-0008)? Reversible
default = creation needs an existing parent dir.

**Not yet done:** Write (S4, shares the mkdir gap), Glob (S5), Grep (S6), full prompt (S7).
