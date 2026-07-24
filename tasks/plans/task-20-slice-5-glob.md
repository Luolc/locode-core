# Task 20 · Slice 5 — `Glob`

> Faithful port of Claude Code's `GlobTool` (ripgrep-backed file pattern matching).
> Source: submodule `6a25909`. Follows [`../../docs/claude-pack-dev-process.md`](../../docs/claude-pack-dev-process.md) (S5).

## Phase 0 — status analysis
- **Merged:** S1–S4 (fs tool set: Bash/Read/Edit/Write + the freshness gate).
- **Next unit:** `Glob` — no shared state; reuses the host's resolved `rg` (ADR-0011)
  + `run_capture` (grok grep/list_dir precedent).
- **Prereqs (hold):** `rg_program()` + `Host::run_capture(program, args, cwd, …)` ✓;
  `resolve_in_jail` + `read_dir` for path validation ✓.
- **Risks:** sort direction + gitignore defaults (source, not memory); path relativization.

## Phase 1 — source revisit (`6a25909`)
- **Schema** (`GlobTool.ts:26-36`): `pattern` + optional `path` (verbatim, incl. the
  "IMPORTANT: Omit this field … DO NOT enter 'undefined'/'null'" sentence).
- **Description** (`prompt.ts:3-7`) verbatim (`descriptions/glob.md`); mentions the
  `Agent` tool (not in pool) — kept (D8 gap).
- **Mechanism** (`utils/glob.ts:66-155`): `rg --files --glob <pattern> --sort=modified
  --no-ignore --hidden` under the search dir. **`--sort=modified` is oldest first**
  (rg ascending); **`--no-ignore` (ignores `.gitignore`) and `--hidden` are the
  defaults** (`CLAUDE_CODE_GLOB_*` || 'true'). First 100 (`:157`). Paths relativized
  under cwd (`toRelativePath`). **Corrects plan §4.5** ("mtime desc"; "respects
  .gitignore").
- **Path validation** (`:96-133`): nonexistent → "Directory does not exist: {p}. Note:
  your current working directory is {cwd}."; not a dir → "Path is not a directory: {p}".
- **Result** (`:177-197`): none → "No files found"; else paths joined by `\n` + (if
  truncated) "(Results are truncated. Consider using a more specific path or pattern.)".

## Design
- `glob.rs`: `ClaudeGlob { host }`; `kind()=Glob`. `run()`: getPath (validate dir via
  `read_dir`) → `run_capture(rg, [--files --glob P --sort=modified --no-ignore
  --hidden], search_dir)` → exit 0 parse / 1 "No files found" / 2+ error → relativize
  under cwd → first 100 + truncation note.
- `mod.rs`: register `Glob` (no state).

## Gaps (this slice)
- CC's permission ignore-patterns + plugin-cache exclusions → our `PathPolicy` jail.
- Absolute glob patterns not base-extracted (rare; CC's `extractGlobBaseDirectory`).

## Test matrix
Schema golden (pattern required; path optional with the DO-NOT-enter sentence); rg-gated
happy path (`**/*.rs` finds `.rs`, excludes `.md`); no-match → "No files found"; missing
path → "Directory does not exist"; file path → "Path is not a directory".

## Result (2026-07-24)
Shipped. `Glob` landed. 162 pack tests (6 new) + full workspace suite pass; four-part
gate green.

**Decisions/deviations:** corrected plan §4.5 (amended, dated note) — `--sort=modified`
is oldest-first and `--no-ignore`/`--hidden` are CC defaults; the port passes those
flags and lets rg sort. Paths relativized under cwd. Gaps: permission ignore-patterns →
jail; absolute glob patterns not base-extracted.

**Not yet done:** Grep (S6), full byte-exact prompt + env + preamble (S7).
