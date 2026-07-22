# Task 27 · Slice 9 — Code-block syntax highlighting (syntect + two-face)

**Status:** done (PR pending) · **Date:** 2026-07-22
**Deps:** +`syntect`, +`two-face` (user-greenlit; ADR-0020)

## Status analysis — the minimal next unit

Phase 1 of the markdown study (`docs/research/markdown-rendering-study.md`): the
biggest remaining visual gap after the slice-7 whitespace fixes is that fenced
code blocks render as flat dim text. Both Rust/ratatui references (codex, grok)
use `syntect` + `two-face`; the user greenlit the dependency.

## Design (ADR-0020)

- New `ui/highlight.rs` (~130 lines) — a compact analog of codex's
  `render/highlight.rs`: `SYNTAX_SET`/`THEME` `OnceLock`s, `find_syntax` (with a
  few aliases), `highlight_lines(code, lang) -> Option<Vec<Vec<Span>>>`, syntect→
  ratatui style conversion (fg only, bold kept, italic/underline dropped, ANSI
  alpha-palette decoding), 512 KB / 10 000-line guardrails.
- Pure-Rust **fancy-regex** engine (not codex's C oniguruma) — this is our own
  UI, not a ported pack, and keeps the static musl release C-free (ADR-0007).
- Fixed **base16-256** theme — ANSI-palette-encoded, so code adapts to the
  terminal's own colors (light/dark) with no config.
- `ui/markdown.rs`: buffer the code block + its language, highlight as one unit
  at the closing fence (indented, no wrap); fall back to plain dim on unknown
  language / oversized input.

## Test matrix (all green)

- `highlight.rs`: highlights + byte-preserves content, resolves aliases
  (rust/rs/python/py/bash/sh/shell/js/csharp), unknown/empty → None, oversized →
  None.
- `markdown.rs`: `fenced_code_with_language_is_highlighted` (colored, not the dim
  fallback); existing no-language fence test still hits the dim fallback.
- `-p locode-tui` 58 lib + integration pass; full workspace clippy + test green.
  Deps resolve with pure-Rust fancy-regex (no C build).

## Result

Fenced code blocks are syntax-highlighted, terminal-adaptive, with a safe plain
fallback. Next study phases: **P2** textwrap polish + OSC-8 links (unblocked),
**P3** tables (unblocked), **P4** streaming markdown (BLOCKED on the core
streaming feature). A theme override and true streaming-highlight are named
ADR-0020 extensions.
