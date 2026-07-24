# Task 20 · Slice 6 — `Grep` (full ripgrep passthrough)

> Faithful port of Claude Code's `GrepTool` — the richest tool: three output modes,
> context flags, glob/type filters, head-limit pagination. Source: submodule
> `6a25909`. Follows [`../../docs/claude-pack-dev-process.md`](../../docs/claude-pack-dev-process.md) (S6).

## Phase 0 — status analysis
- **Merged:** S1–S5 (Bash/Read/Edit/Write + gate, Glob). Grep completes the six-tool set.
- **Next unit:** `Grep` — no shared state; reuses the resolved `rg` + `run_capture`
  (grok grep + Glob precedent). Largest schema (14 fields) + per-mode rendering.
- **Risks:** rg-flag-named fields (`-A/-B/-C/-n/-i`) via serde rename; per-mode
  rendering (content/count/files) + head_limit/offset pagination; absolute-path output.

## Phase 1 — source revisit (`6a25909`)
- **Schema** (`GrepTool.ts` inputSchema): `pattern`/`path`/`glob`/`output_mode`/`-B`/
  `-A`/`-C`/`context`/`-n`/`-i`/`type`/`head_limit`/`offset`/`multiline` (verbatim
  descriptions). `deny_unknown_fields`; type-strict.
- **Description** (`prompt.ts:6-18`) verbatim (`descriptions/grep.md`; mentions `Agent`).
- **rg args** (`call()`): `--hidden` → `--glob !<vcs>`×6 (`.git .svn .hg .bzr .jj .sl`)
  → `--max-columns 500` → `-U --multiline-dotall` → `-i` → mode (`-l`/`-c`/none) →
  `-n` (content + default-true) → context (`-C`/`context`, else `-B`/`-A`; content
  only) → pattern (`-e` if leading `-`) → `--type` → `--glob` (whitespace-split,
  brace-preserving) → the absolute target as positional (rg emits absolute paths,
  `ripgrep.ts:365`).
- **head_limit/offset** (`applyHeadLimit`, `:110-128`): default 250, `0` = unlimited;
  `appliedLimit` only when truncated. `files_with_matches` sorts by mtime **desc**
  (filename tiebreak); paths relativized under cwd.
- **Rendering** (`:254-311`): content → lines ("No matches found") + pagination note;
  count → lines + "Found N total occurrences across M files." summary; files → "No
  files found" or "Found N file(s) [limit]\n<paths>".
- **rg exit** (`ripgrep.ts:378-386`): 0 matches / 1 none (empty) / 2+ error.
- **Caps:** `maxResultSizeChars: 20_000` (`:164`).

## Design
- `grep.rs`: `GrepOutputMode` enum (snake_case, default `files_with_matches`); `GrepArgs`
  with serde renames for the rg-flag keys. `run()` builds rg args in CC's order, runs
  `run_capture`, then per-mode processing (`apply_head_limit`, `format_limit_info`,
  `relativize`, mtime-desc sort for files mode), and caps the body at 20k.
- `mod.rs`: register `Grep` (no state).

## Gaps (this slice)
- CC's permission ignore-patterns + plugin-cache exclusions → our `PathPolicy` jail.
- 20k `maxResultSizeChars` persist-preview approximated by a head char cap + the 30k
  engine belt (ADR-0008).

## Test matrix
Schema golden (all 14 fields incl. `-A/-B/-C/-n/-i`; only `pattern` required); rg-gated:
files mode default ("Found 1 file…"), no-match ("No files found"), content mode
(`a.txt:2:needle two`), content no-match ("No matches found"), count mode ("Found 3
total occurrences across 1 file."). Unit: `apply_head_limit`, `format_limit_info`,
`split_glob` (brace preservation), `plural`.

## Result (2026-07-24)
Shipped. `Grep` landed — **all six tools now ported.** 172 pack tests (11 new) + full
workspace suite pass; four-part gate green.

**Decisions/deviations:** rg-flag-named fields kept as exact wire keys via serde rename;
`files_with_matches` mtime-desc sort (filename tiebreak) matches CC production (CC's
test-mode filename sort not modeled); 20k cap approximated (head char cap + engine belt).

**Not yet done:** Slice 7 — the full byte-exact system prompt + env block + preamble
reshaping + `PackContext` growth. Then the pack is complete.
