# ADR-0006: Dialect packs over one registry; `grok` default; `EditEncoding` enum

## Status
Accepted

## Date
2026-07-17

## Context
The reason to build a *study* harness (not a toy) is to A/B different tool-harness styles against the same underlying implementations. Grok proves this is cheap: one registry keyed by canonical identity, re-skinned per session by name/param/description overrides plus an edit-encoding choice. The four studied surfaces differ mainly in (a) which tools are enabled, (b) naming/casing, and (c) how "change a file" is expressed.

## Decision
Keep **one canonical registry** (six `ToolKind`s) and layer **dialect packs** over it. A `Dialect` carries: `enabled: Vec<ToolKind>`, `tool_name: ToolKind -> String`, `param_rename`, `edit_encoding: EditEncoding`, and a description renderer that substitutes sibling tool names. `list_specs(dialect)` builds the model's tool array by re-skinning derived schemas; `dispatch` maps the client-facing name back to a `ToolKind` and runs the one real impl. **Default/house dialect is `grok`** (snake_case, first-class FS tools). `claude`/`codex`/`opencode` are added afterward as re-skins.

`EditEncoding` is an enum with **only `ExactString` implemented in v0**; `ApplyPatchFreeform` (Codex) and `AnchorEdits` (Grok hashline) are reserved variants (P1/P2 seams). The `codex` dialect implies `ApplyPatchFreeform` + a reduced enabled set (no first-class read/write/edit), so it waits on P1.

## Alternatives Considered
### One hard-coded tool surface, no dialects
- Rejected: eliminates the harness's whole purpose (comparing surfaces) and would require duplicated tool logic to add one later.

### Grok's full multi-skin machinery (Concise/Hashline sub-variants, per-wire template packs)
- Deferred: v0 keeps a small-but-real four-dialect table; sub-variants are additive.

## Consequences
- Four dialects share six implementations; running `--dialect claude` vs `--dialect grok` on one task yields comparable trajectories with zero duplicated tool code.
- Tool impls never hard-code the name the model sees — the one change that makes dialects cheap.
- `apply_patch` drops in later as one `EditEncoding` variant + parser, with no loop/registry refactor.
