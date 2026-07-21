# grep — fidelity audit vs Grok Build

Audited 2026-07-20 directly against `xai-grok-tools/src/implementations/grok_build/grep/mod.rs`
("gb" below). Ours: `crates/locode-packs/src/grok/grep.rs`.

## Verdict
**FAITHFUL as of PR #51** (was: DRIFT — 8 schema issues, 5 behavior issues in the
Task 11 port). One documented output-equivalent deviation remains (see below).

## Schema comparison (post-fix)

| Field (wire name) | Grok (gb/grep/mod.rs) | Ours (grep.rs) | Status |
|---|---|---|---|
| `pattern` | required, `:50-52` | `:60-63` | MATCH (verbatim description) |
| `path` | optional, `:55-57` | `:64-68` | MATCH |
| `glob` | optional, `:60-62` | `:69-73` | MATCH |
| `output_mode` | **wire-accepted, `#[schemars(skip)]`** `:63-67` | `:74-78` | MATCH (quirk reproduced) |
| `-B` | serde rename, `:72-76` | `:79-81` | MATCH |
| `-A` | serde rename, `:81-85` | `:82-84` | MATCH |
| `-C` | serde rename, `:90-94` | `:85-87` | MATCH |
| `-i` | serde rename of `case_insensitive`, `:98-105` | `:88-90` | MATCH — **was RENAMED-drift** (exposed as `case_insensitive` pre-fix) |
| `type` | `:108-111` | `:91-95` | MATCH — **was MISSING** |
| `head_limit` | `:114-117` | `:96-100` | MATCH — **was MISSING** |
| `multiline` | `:120-127` | `:101-105` | MATCH — **was MISSING** |

Pre-fix, `output_mode`, `-B`, `-A`, `-C`, `type`, `head_limit`, `multiline` were
all MISSING ("dropped in v0 as reserved seams") — 7 of 11 fields.

## Tool description comparison
Grok's real template (`gb/grep/mod.rs:248-258`) is the 5-bullet
"Search file contents with regular expressions (ripgrep). …" block — now ported
verbatim (`grep.rs:150-157`). Pre-fix ours was an invented one-liner
("Search file contents with a regular expression (ripgrep-backed). Respects
.gitignore.") — DRIFT, fixed.

## Behavior comparison (post-fix)
- rg arg order faithful (`gb:760-828` → `grep.rs:164-227`): base flags → `-i` →
  `--glob` → `--type` → `-U --multiline-dotall` → `-C`/`-B`/`-A` (only when >0)
  → `-l`/`-c` per mode → `-e PATTERN` → PATH → `--max-filesize 5M`.
  Pre-fix: no type/multiline/context/mode flags, `--regexp … -- PATH` framing,
  no `--max-filesize`.
- Head-limit resolution faithful (`resolve_effective_head_limit`, `gb:197-203`):
  defaults 200 (content) / 500 (files/count), caps 2000 / 10000; exact-fit
  results are never flagged truncated (`gb:355-360` rule).
- Exit-code mapping (0 matches / 1 no-match / 2+ error) — MATCH both eras.
- **Remaining deviation (documented, `grep.rs:9-12`):** grok reads
  `head_limit + 1` stdout lines and kills rg early on overflow; we capture under
  the host byte cap and apply the line budget post-capture. Output-equivalent
  (same first-N lines, same truncation flag); perf-only difference. Revisit only
  if the host gains line-capped capture.
- **Not ported (out of scope for the pack):** grok's managed read-deny globs
  (`gb:783-787`) — policy is host-jail territory (ADR-0008), grok's equivalent
  of a permission subsystem we deliberately model differently; and the
  streaming-progress gate (`gb:298+`) — TUI-only surface.

## Quirks
- `output_mode` hidden-but-accepted (soft-deprecated model arg; grok's own
  models may still emit it from training) — reproduced exactly.
- Flag-literal JSON keys (`-B`/`-A`/`-C`/`-i`) — reproduced via serde renames.
- Context flags only emitted when value > 0 (`gb:800-814`) — reproduced.

## Fixing task
Done — PR #51 (commit `fbe48fa`). Regression guards in place:
`schema_uses_grok_wire_names_and_hides_output_mode`,
`flag_fields_deserialize_from_wire_names`, head-limit unit tests
(`grep.rs:290-360`). Remaining acceptance criterion for full closure:
1. [ ] When/if `locode-host` gains line-capped capture, switch to grok's
   read-N+1-and-kill-early and delete the documented deviation note. (S, optional)
