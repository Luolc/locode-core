# Task 27 · Slice 7 — Markdown rendering fixes (whitespace, spacing, indent)

**Status:** done (PR pending) · **Date:** 2026-07-22 · **Deps:** none (no new crates)

## Status analysis — the minimal next unit

First-user vibe-check (screenshots: locode vs Claude Code) surfaced that our
assistant Markdown reads "much worse." Grounded diagnosis against
`ui/markdown.rs` found **real rendering bugs**, not just missing polish. This
slice fixes the bugs with no new dependencies; syntax highlighting (`syntect`)
and tables stay separate, dependency-gated slices (see
`docs/research/markdown-rendering-study.md`).

## Bugs fixed

1. **Inline-code whitespace** (the worst). The old `text()` rebuilt spacing from
   `split_whitespace()`, which dropped the space *before* an inline code span and
   inserted a spurious one *after* it: `from `README.md`` → `fromREADME.md`,
   `` streaming `Event`s `` → `streamingEvent s`. Root fix: never reconstruct
   whitespace — keep source text verbatim in styled `Seg`s and only collapse at
   the word-wrap boundary.
2. **List marker lands mid-line** when an item starts with inline code
   (`` 1. `run.rs` … ``): the marker was only emitted on a "fresh line," which a
   leading code span never triggered. Now the marker is a first-line lead.
3. **Nested tight lists merged the parent item's text into the child**
   (`- two` + `  - nested` → `  • twonested`): a nested `List` start now flushes
   the parent item's pending inline first.
4. **No blank line between blocks** → dense wall of text. Added one-blank-line
   gaps between top-level blocks (not between tight list items).
5. **No hanging indent** on wrapped list items → continuation fell to column 0.
   Wrapping now takes a first-line lead + a continuation lead (aligns under text;
   quote bar repeats per line).
6. **Prose ran to the right edge.** `AssistantText` now wraps at `width - 2`.

Also added: strikethrough styling (already parsed), a full-width `─` rule.

## Design

Rewrote `ui/markdown.rs` to codex's shape: parse → collect inline content as
`Vec<Seg{text, style}>` per block (whitespace verbatim) → `flush_inline()` wraps
into lines via `wrap_words` with distinct first/continuation leads.
`segs_to_words` collapses whitespace only at word boundaries; `build_line`
coalesces same-style runs into spans. This is the correct architecture for
adding syntect + tables later without revisiting whitespace.

## Test matrix (all green)

Regression tests added: `inline_code_preserves_surrounding_spaces`,
`code_span_then_suffix_has_no_gap`, `list_item_starting_with_code_keeps_marker_first`,
`wrapped_list_item_has_hanging_indent`, `blocks_separated_by_blank_line`,
`strikethrough_is_styled`. Existing tests updated for the new (correct) spacing +
block gaps. `-p locode-tui`: 52 lib + integration pass; full workspace clippy +
test green.

## Result

`ui/markdown.rs` rewritten (whitespace-correct, hanging indent, block gaps);
`ui/blocks.rs` `AssistantText` gains a right margin. No new deps, no public
surface change. Next slice: composer border + status-line relayout (cwd · model ·
context), then the dependency-gated syntect highlighting.
