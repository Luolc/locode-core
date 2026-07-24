# Task 20 · Slice 2 — `Read` + `ClaudeSessionState`

> Faithful port of Claude Code's `FileReadTool` + the per-run read-freshness store
> that the S3/S4 read-before-edit gate consumes. Source: Claude Code submodule
> `6a25909`. Follows [`../../docs/claude-pack-dev-process.md`](../../docs/claude-pack-dev-process.md) (S2).

## Phase 0 — status analysis
- **Merged:** Slice 1 (pack scaffold + `Bash` + minimal prompt + `Pack::shape_user_prompt`).
  `ClaudePack` is a unit struct; `register()` wires `Bash` only.
- **Next unit:** `Read` (`FileReadTool`) + `ClaudeSessionState` (the freshness store).
  Read records mtime per window; the store is the seam Edit/Write (S3/S4) will gate on.
- **Why now:** Read is the first tool needing shared state; standing the store up here
  (constructed in `register()`, cloned into Read) matches CC's per-session
  `readFileState` and unblocks S3/S4. Also lands the `descriptions/` pattern for a
  second, PDF-mentioning description (D8 gap discipline).
- **Prereqs (hold):** `Host::read_file` returns `FileRead { contents, stat.modified }`
  ✓ (mtime in one call — "the edit-freshness token"); jail via `resolve_in_jail` ✓.
- **Risks:** (1) line-numbering + trailing-newline edge; (2) the token cap uses a
  byte estimate (no API tokenizer); (3) the `file_unchanged` dedup needs the store to
  distinguish Read-origin (offset set) from Edit/Write-origin (offset None) entries.

## Phase 1 — source revisit (`6a25909`)
- **Schema** (`FileReadTool.ts:227-244`, `z.strictObject`): `file_path` (string,
  "The absolute path to the file to read"), `offset` (int nonnegative optional),
  `limit` (int positive optional), `pages` (string optional, PDF). `deny_unknown_fields`.
  `offset`/`limit` type-strict (repo policy). **`pages` kept schema-visible**, PDF
  behavior deferred + ignored for text — grok's `read_file` precedent (`read.rs`
  kept `pages`/`format`); a faithfulness-over-plan-§4.2 amendment (§4.2 said "drop
  pages"; grok keeps its equivalent → keep for schema fidelity, log the gap).
- **Description** (`FileReadTool.ts:347-358` → `renderPromptTemplate`, `prompt.ts:27-49`)
  for our config: `MAX_LINES_TO_READ` 2000; `includeMaxSizeInPrompt` default falsy →
  no max-size clause; `targetedRangeNudge` default falsy → `OFFSET_INSTRUCTION_DEFAULT`;
  `pickLineFormatInstruction` → the cat -n line; `isPDFSupported()` = true for a
  non-`claude-3-haiku` model (`pdfUtils.ts`) → **the PDF bullet renders** (kept
  verbatim; gap: PDF reads deferred). `BASH_TOOL_NAME` = `Bash`. Stored verbatim in
  `descriptions/read.md` (pinned).
- **`cat -n` format** (`addLineNumbers`, `file.ts:290-318`): default is the **compact**
  branch (`isCompactLinePrefixEnabled` killswitch off, `file.ts:278-285`) →
  `${lineNo}\t${line}` (1-indexed absolute, tab separator). We honor the documented
  "cat -n" contract with real cat -n line semantics (`str::lines()` — a trailing
  newline does not add a phantom numbered line).
- **Windowing** (`call()`, `FileReadTool.ts:497,1019`): `offset` default **1**
  (1-indexed; `offset===0?0:offset-1`), `limit` default undefined (read to the token
  cap). `MAX_LINES_TO_READ` is descriptive; the real guard is the token cap.
- **Warnings** (`FileReadTool.ts:704-707`): empty file → `<system-reminder>Warning:
  the file exists but the contents are empty.</system-reminder>`; window past EOF →
  `<system-reminder>Warning: the file exists but is shorter than the provided offset
  (N). The file has M lines.</system-reminder>`.
- **Token cap** (`limits.ts:18` `DEFAULT_MAX_OUTPUT_TOKENS = 25000`; validate
  `FileReadTool.ts:181,755-766`): error text "File content (${n} tokens) exceeds
  maximum allowed tokens (${max}). Use offset and limit parameters to read specific
  portions of the file, or search for specific content instead of reading the whole
  file." CC estimates then counts via API; we use a byte/4 estimate (grok precedent,
  no API tokenizer). Documented gap.
- **Freshness / dedup** (`FileReadTool.ts:540-570,1032`): `readFileState.set(path,
  {content, timestamp: floor(mtimeMs), offset, limit})`. Dedup: entry exists, came
  from a Read (`offset !== undefined`), same `offset`+`limit`, and `mtime ===
  timestamp` → return `FILE_UNCHANGED_STUB` (`prompt.ts:7-8`). Edit/Write store
  `offset=undefined`, so they never dedup-match — the seam S3/S4 use.

## Design
```
crates/locode-packs/src/claude/
├── state.rs   # ClaudeSessionState: path -> {mtime_ms, offset, limit}
├── read.rs    # Read (FileReadTool) — compact cat -n, window, warnings, cap, dedup
└── descriptions/read.md
```
- **`ClaudeSessionState`** (`state.rs`): `Mutex<HashMap<PathBuf, ReadRecord>>` where
  `ReadRecord { mtime_ms: Option<u64>, offset: Option<u64>, limit: Option<u64> }`.
  S2 methods (only what S2 uses, to avoid dead-code): `record_read(path, modified,
  offset, limit)` and `is_unchanged_read(path, modified, offset, limit) -> bool`.
  `check_fresh` + `record_write` land in S3/S4 (same struct). mtime stored as floored
  ms (CC's `Math.floor(mtimeMs)`) for exact-equality dedup + `>` staleness later.
- **`Read`** (`read.rs`): `ClaudeRead { host, state }`. `kind()=Read`;
  `description()=include_str!("descriptions/read.md")`. `run()`:
  jail-resolve + `read_file` (contents + mtime) → dedup check (unchanged → stub) →
  `lines()` window `[offset-1 .. +limit]` → empty/short-offset warnings → compact
  `N\tline` join → byte/4 token cap (25k) → record freshness → structured out + body.
- **`mod.rs`**: `register()` constructs `Arc<ClaudeSessionState>`, clones into `Read`;
  `Bash` unchanged. `ClaudePack` stays a unit struct (state is per-`register`, = CC's
  per-session store).

## Gap log (this slice)
- **PDF/images/notebooks** — description mentions them (verbatim, D8); `pages` accepted
  + ignored for text; image/PDF/notebook reads degrade to UTF-8 text (deferred tier).
- **Token count** — byte/4 estimate, not an API tokenizer (grok precedent).
- **`FILE_UNCHANGED_STUB` dedup** — ported (faithful, cheap); killswitch not modeled.

## Test matrix
- Schema golden: `Read` — `additionalProperties:false`; `file_path`/`offset`/`limit`/
  `pages` present with verbatim descriptions; only `file_path` required; offset/limit
  reject string/float (type-strict).
- Description provenance pin: `read.md` byte-len + opening line + sha256.
- Behavior: numbered `N\tline` output; offset/limit window; missing file soft error;
  empty file → empty warning; window past EOF → short-offset warning; records
  freshness; unchanged same-window re-read → `FILE_UNCHANGED_STUB`; changed file (touch
  after read) → full re-read; token cap → error text.
- Pack: specs list `["Bash","Read"]`.

## Preset target
`locode -p --harness claude --api-schema mock "read"` still returns a valid Report;
four-part gate green.

## Result (2026-07-24)

Shipped. `Read` (`FileReadTool` text path) + `ClaudeSessionState` landed;
`register()` constructs the per-run store and clones it into `Read`. Four-part gate
green; 139 pack tests (28 claude) + full workspace suite pass; preset runs.

**Decisions/deviations (in-scope, autonomous):**
- **`pages` kept schema-visible** (accepted, ignored for text) rather than dropped —
  faithfulness + grok's `read_file` precedent over plan §4.2's "drop pages". PDF/image/
  notebook reads deferred (text-only); logged gap.
- **Compact `cat -n`** (`N\tline`, tab, 1-indexed) — the killswitch-off default
  (`isCompactLinePrefixEnabled`). Real cat -n line semantics via `str::lines()` (no
  phantom trailing line) — honors the documented contract.
- **Token cap** 25k via a byte/4 estimate (no API tokenizer; grok precedent) — gap.
- **`FILE_UNCHANGED_STUB` dedup** ported: unchanged same-window re-read → stub. The
  store keys dedup on Read-origin entries (offset set); Edit/Write (S3/S4) will store
  `offset=None` and never dedup-match — the freshness-gate seam.
- **`ClaudeSessionState`** stores mtime as floored-ms (CC's `Math.floor(mtimeMs)`) for
  exact-equality dedup + `>` staleness later; poison-tolerant lock. S2 exposes only
  `record_read`/`is_unchanged_read`; `check_fresh`/`record_write` land with S3/S4 (no
  dead code).

**Not yet done:** Edit (S3) + Write (S4) consuming the gate, Glob (S5), Grep (S6),
full byte-exact prompt + env + preamble (S7).
