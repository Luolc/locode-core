# ADR-0020: TUI Markdown code-block syntax highlighting (syntect + two-face)

## Status
Accepted

## Date
2026-07-22

## Context
The first-user vibe-check (screenshots vs Claude Code) found the assistant
transcript reading "much worse." The whitespace/layout bugs were fixed first
(PR #90). The remaining biggest visual gap is **unhighlighted code blocks** —
our `ui/markdown.rs` rendered fenced code as flat dim text.

The four-harness study
([`docs/research/markdown-rendering-study.md`](../research/markdown-rendering-study.md))
found both Rust/ratatui references — **codex and grok** — highlight code with the
**same stack: `syntect` + `two-face`** (bat's ~250-language grammar + theme
bundles). This is "Phase 1" of that study's recommendation. User greenlit the
dependency 2026-07-22 ("if both Grok Build and Codex do that, we're good").

Adding a dependency is an AGENTS.md "ask first" item; this ADR records the
accepted decision and the one deliberate deviation.

## Decision

Add `syntect` + `two-face` to the workspace and `locode-tui`; highlight fenced
code blocks in the markdown renderer.

- **Regex engine: pure-Rust `fancy-regex`, NOT oniguruma (C).** codex uses
  `two-face`'s `syntect-default-onig` feature (the C Oniguruma engine). We select
  `default-fancy` / `syntect-default-fancy` instead. Rationale: this is our own
  `locode` app UI, **not a ported harness pack**, so the faithful-mimicry rule
  does not force onig; and a C dependency would complicate the fully-static musl
  release, which the repo keeps C-free by design (rustls, no OpenSSL — ADR-0007).
  Trade-off: fancy-regex is marginally slower and a few oniguruma-specific
  grammar constructs may highlight imperfectly — acceptable for code blocks.
- **Fixed theme `base16-256`** (via `two_face::theme::extra()`), chosen because
  it encodes colors as **ANSI palette indices** (bat's alpha-marker encoding),
  not hard RGB — so highlighted code **adapts to the user's terminal theme** and
  reads on both light and dark backgrounds. No per-user theme config at v1.
- **Foreground only** — backgrounds omitted so the terminal background shows
  through; **italic and underline suppressed** (poor/again-terminal rendering,
  and some themes underline type scopes). These mirror codex's `convert_style`.
- **Guardrails**: skip highlighting past 512 KB or 10 000 lines; unknown/empty
  language → `None`. In every fallback the caller renders the block as plain dim
  indented text (never fails, never drops content).
- **Buffer-then-highlight**: the renderer buffers the whole code block and
  highlights it at the closing fence (a highlighter needs full lines). Code is
  **not word-wrapped** — lines are preserved for copy/paste, as codex does.

## Alternatives Considered
- **Oniguruma (codex's exact config)** — rejected for the musl/C reason above;
  faithfulness doesn't apply to our own UI.
- **tree-sitter (opencode)** — rejected: opencode fetches WASM grammars over the
  network at runtime; `syntect`'s embedded set is the right call for a
  single-binary agent.
- **Per-user RGB theme (e.g. tokyo-night like grok)** — deferred: an
  ANSI-adaptive theme is more robust across terminals with zero config; a theme
  override is a clean later addition (codex has one).
- **Keep dim-only code** — rejected: it was the single biggest visual gap.

## Consequences
- `syntect` + `two-face` (+ `fancy-regex`, `plist`, etc.) are added to the TUI
  build — the heaviest deps in `locode-tui`, but pure-Rust and musl-clean. The
  release workflow validates the static musl build on the next tag.
- `ui/highlight.rs` is a compact (~130-line) analog of codex's `render/highlight.rs`;
  a theme-override seam and true streaming highlight are named later extensions.
- Inline `` `code` `` stays cyan (unchanged); only fenced blocks are highlighted.
