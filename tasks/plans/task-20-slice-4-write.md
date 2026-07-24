# Task 20 · Slice 4 — `Write` (create-or-overwrite, read-before-write gate)

> Faithful port of Claude Code's `FileWriteTool`. Source: submodule `6a25909`.
> Follows [`../../docs/claude-pack-dev-process.md`](../../docs/claude-pack-dev-process.md) (S4).

## Phase 0 — status analysis
- **Merged:** S1–S3 (`Bash`, `Read`, `Edit` + the `ClaudeSessionState` gate with
  `check_fresh`/`record_write`).
- **Next unit:** `Write` — reuses `check_fresh`/`record_write` unchanged (no new store
  API). Completes the fs tool set before Glob/Grep.
- **Prereqs (hold):** `check_fresh`/`record_write` ✓ (S3); `Host::stat` (existence +
  mtime without reading content) ✓; `Host::write_file` ✓.
- **Risks:** identical gate messages to Edit (share the strings); the mkdir gap
  (creation needs an existing parent dir).

## Phase 1 — source revisit (`6a25909`)
- **Schema** (`FileWriteTool.ts:56-64`): `file_path` ("The absolute path to the file
  to write (must be absolute, not relative)") + `content` ("The content to write to
  the file"). `deny_unknown_fields`.
- **Description** (`prompt.ts:10-18`, `getWriteToolDescription`) verbatim in
  `descriptions/write.md`.
- **Gate** (`validateInput`, `:153-222`): stat the path — ENOENT → **new file, write
  freely**; exists + not in store (or partial) → errorCode 2 "File has not been read
  yet. Read it first before writing to it."; exists + mtime > recorded read →
  errorCode 3 "File has been modified since read, either by the user or by a linter.
  Read it again before attempting to write it." (Same messages + store as Edit's
  errorCode 6/7.)
- **Write verbatim** (`:302`, `writeTextContent(..., 'LF')`): the model's `content`
  is written as-is (contrast Edit's line-ending preservation).
- **Success text** (`:418-430`): new → "File created successfully at: {path}";
  existing → "The file {path} has been updated successfully."
- Records the post-write mtime (offset=None → Write-origin, so Read's dedup skips it).

## Design
- `write.rs`: `ClaudeWrite { host, state }`; `kind()=Write`. `run()`: resolve → `stat`
  (existence + mtime) → for existing files, `check_fresh` gate (errorCode 2/3) →
  `write_file` verbatim → `record_write` → create/update success text.
- `mod.rs`: register `Write` sharing the store.
- No `state.rs` change (gate methods landed in S3).

## Gaps (this slice)
- Host doesn't mkdir parents (ADR-0008) — creation needs an existing parent dir
  (shared with Edit; batched mkdir-seam question).

## Test matrix
Schema golden; description pin; new file creates freely ("File created successfully
at:"); existing unread → errorCode 2; existing after Read → overwrite ("has been
updated successfully."); external modify → errorCode 3.

## Result (2026-07-24)
Shipped. `Write` landed, reusing the S3 gate unchanged. 156 pack tests (6 new) + full
workspace suite pass; four-part gate green; preset runs.

**Decisions/deviations:** none beyond the shared mkdir gap (creation needs an existing
parent dir). Content written verbatim (CC's LF-as-sent). Gate messages identical to
Edit's (shared strings).

**Not yet done:** Glob (S5), Grep (S6), full byte-exact prompt + env + preamble (S7).
The fs tool set (Bash/Read/Edit/Write) is now complete.
