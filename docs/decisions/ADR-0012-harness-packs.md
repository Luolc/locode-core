# ADR-0012: Harness packs — faithful per-harness toolsets (supersedes ADR-0006)

## Status
Accepted

## Date
2026-07-17

## Supersedes
[ADR-0006](ADR-0006-dialects-and-edit-encoding.md)

## Context
ADR-0006 modeled tool variety as **one canonical implementation per `ToolKind`,
re-skinned per session** by name/param/description overrides (a "dialect"). That
optimizes for DRY. But locode's actual purpose is to be a **faithful experiment
bed** for studying real coding-agent harnesses: run the *same* task under a genuine
reproduction of each harness (Grok Build, Codex, Claude Code, OpenCode) and compare.
A re-skin hides exactly what we want to measure — each harness's **real behavior**
(edit matching, output shaping, freshness rules, search semantics), not just its tool
names. For this project, **fidelity beats DRY**.

## Decision
Replace dialects with **harness packs**. A **pack** is a complete, faithful
reproduction of one harness's toolset — its actual tool **implementations, names,
argument schemas, descriptions, and behavior** — selected at runtime via
`--harness <name>`. Packs:

- `grok`, `codex`, `claude`, `opencode` — faithful ports of the studied harnesses.
- `locode` — our own pack absorbing the best practices of the others (grok-build-style
  snake_case naming).

Within a pack, the **tool implementation/behavior is P0**; exact names and
descriptions are P1 (approximate first, refine later).

- **No shared canonical implementation and no re-skin layer.** ADR-0006's
  `Dialect`/`EditEncoding`-as-reskin machinery (`tool_name`/`param_rename`/`describe`
  overrides) is dropped. Each pack implements its harness's real tools directly.
- **`ToolKind` survives only as a classification tag** (this tool "is a read" / "is an
  edit") so cross-pack A/B reports can align comparable tools. It is not a shared impl.
- **Shared infrastructure stays pack-agnostic:** the `Tool` trait (typed Args/Output,
  derived schemas), soft/fatal errors, dual `output`/`prompt_text`, the transcript
  invariants, the one dispatch door, and the `locode-host` seam. Reusable primitives
  (host-backed fs/shell, an `apply_patch` parser when needed, truncation) are shared
  libraries the packs call.
- **Each pack owns its system prompt** (the harness's real prompt), not one templated
  prompt re-pointed by name.
- **Crate:** `locode-dialects` → `locode-packs`, a module per harness. The report
  envelope field and CLI flag become `harness` / `--harness`.

### apply_patch and provider coupling (clarified)
The `apply_patch` *format* is provider-agnostic: it can be delivered as a normal tool
with a single JSON string arg (`{ patch }`), which works on Anthropic and everyone
else. Only Codex's **freeform, grammar-constrained** delivery is OpenAI-Responses-
specific. So a future `codex` pack uses the JSON-string framing on the Anthropic wire;
the freeform-grammar delivery is deferred to an optional Responses wire. `apply_patch`
does **not** enter v0 (the grok pack edits via `search_replace`).

## Scope
**v0 = the `grok` pack only**, ported from `xai-grok-tools` and trimmed to
headless-minimal (dropping interactive/sandbox/MCP/streaming concerns that don't
apply). Additional packs, the `locode` best-of pack, and the first cross-pack A/B are
the next milestone.

## Alternatives Considered
### Dialect re-skins over one canonical registry (ADR-0006)
- Optimizes DRY; minimal code. **Rejected:** collapses every harness onto one shared
  behavior, contaminating the very comparison the experiment bed exists to make.

### A single house toolset only, no per-harness reproduction
- Simplest. **Rejected:** loses the study/experiment value entirely; the `locode` pack
  alone can't reveal how the real harnesses behave.

## Consequences
- More code — N packs × M real implementations rather than 6 impls + skins — accepted
  deliberately. In practice each pack is a faithful *port* (adapting the source
  harness's tools onto our `Tool` trait + host seam), largely mechanical.
- Comparisons are honest: `--harness grok` vs `--harness codex` exercise genuinely
  different tool behavior.
- The `locode` pack becomes a real, opinionated design deliverable (post-v0), not a
  naming choice.
- `EditEncoding` as a shared enum is gone; a pack that needs `apply_patch` gets a
  shared *parser*, not a variant in a canonical edit tool.
- The `Tool` framework (trait/registry/dispatch/errors) and `locode-tools` remain; what
  moves is the concrete tools — from "6 canonical impls in `locode-tools`" to
  "per-harness impls in `locode-packs`."
