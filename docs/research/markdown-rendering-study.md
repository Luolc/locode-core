# Markdown rendering in coding-agent TUIs — a four-harness study

**Date:** 2026-07-22 · **Author:** agent (source-grounded, per AGENTS.md)
**Scope:** how Claude Code, Codex, Grok Build, and opencode render Markdown /
rich text in their terminal UIs — parser, syntax highlighting, streaming,
wrapping, tables — and what `locode-tui` should do next.

Companion to [`tui-harness-study.md`](tui-harness-study.md) (the broader TUI
architecture study) and [`SPEC-TUI.md`](../../SPEC-TUI.md). Grounded in the
`coding-cli-survey` submodules (`file:line` citations throughout); the survey
write-ups do **not** cover rendering, so the source itself is authoritative.

---

## TL;DR — the four converge on one shape

1. **Nobody hand-writes the Markdown parser.** All four lex with a real library
   — `pulldown-cmark` (both Rust harnesses) or `marked` (both TS harnesses) —
   and hand-roll only the **styling + streaming** layer. Rolling your own
   CommonMark lexer is a mistake none of them make.
2. **Code blocks get real syntax highlighting.** The two Rust harnesses both use
   **`syntect` + `two-face`** (bat's ~250-language grammar set); Claude Code uses
   `cli-highlight`/`highlight.js`; opencode uses `web-tree-sitter`. Highlighted
   code is the single biggest visible-quality gap between a "toy" and a "real"
   agent transcript.
3. **Streaming re-renders, it does not stitch.** Every harness re-parses the
   message text (or a bounded tail of it) through the *same* renderer rather than
   incrementally mutating a token tree — because the only way to guarantee "the
   streamed frame equals the final frame" is to run the same code. They differ
   only in **how much** they re-parse per delta and how they bound the cost.
4. **Markdown wraps itself; code does not.** Prose is width-wrapped by the app
   (`textwrap` / `wrap-ansi` / native), preserving inline styles and hyperlink
   offsets. Fenced code is deliberately **left unwrapped** to preserve
   copy/paste fidelity (codex says so in a comment).
5. **Tables are always a bespoke layout pass** — none reuse a generic table
   widget; all compute column widths and wrap cells themselves, with a
   key/value transpose fallback when a table is too wide.

---

## Per-harness findings

### Codex (`codex-rs`, Rust + ratatui) — our closest twin

The most directly relevant reference: same language, same UI framework.

- **Parse:** `pulldown-cmark` (workspace-pinned `0.10`) with only
  `ENABLE_STRIKETHROUGH | ENABLE_TABLES` — everything else CommonMark default
  (`tui/src/markdown_render.rs:323-326`). Footnotes/task markers ignored.
- **Convert:** a stateful `Writer` consumes pulldown events and emits
  `Vec<HyperlinkLine>` (a ratatui `Line` + hyperlink column ranges) —
  `markdown_render.rs:368-433`, driven by `handle_event` (`:442`). All styling
  lives in one `MarkdownStyles` struct (`:87-123`): h1 bold+underline, h2 bold,
  h3 bold+italic, h4–h6 italic; inline code cyan; links cyan+underline;
  blockquote green; ordered-marker light-blue.
- **Code highlight:** `syntect` + `two-face` (`tui/src/render/highlight.rs`),
  runtime-swappable theme behind an `RwLock`, **backgrounds intentionally
  omitted** so the terminal background shows through (`:681`). Guardrails:
  inputs > **512 KB** or > **10 000 lines** skip highlighting (`:565-604`).
- **Streaming:** buffer deltas, **commit only at newline boundaries**
  (`markdown_stream.rs`, `commit_complete_source` = `buffer.rfind('\n')`), then
  **full re-parse of the entire committed buffer** on each newline-bearing delta
  (`streaming/controller.rs:289` `recompute_streaming_render`). A
  stable-prefix / mutable-tail split lets earlier lines settle into scrollback
  while a `TableHoldbackScanner` keeps a growing table region mutable so its
  columns can reshape (`streaming/table_holdback.rs`). An animation queue
  reveals stable lines a few at a time. Correctness is pinned by tests asserting
  **streamed output == full-render output** across many chunkings
  (`markdown_stream.rs:804-851`).
- **Wrap:** self-wrapped via a `textwrap`-backed module (`tui/src/wrapping.rs`),
  per logical line, preserving inline styles + remapping hyperlink ranges.
  **Code blocks are explicitly not wrapped** (`markdown_render.rs:1811-1812`).
- **Perf trade-off:** no caching of rendered lines — the finalized history cell
  stores only the raw source and re-parses every frame/width (chosen for
  correctness + clean resize). The one memoized value is a stable-prefix line
  count during table streaming (`StablePrefixLenCache`, `controller.rs:422`).

### Grok Build (Rust + ratatui) — the most sophisticated

A dedicated crate `xai-grok-markdown` (parsing lib + everything else bespoke).

- **Parse:** `pulldown-cmark` `0.13`. **Highlight:** `syntect` `5.3` + `two-face`
  `0.4` + `anstyle-syntect`; bundled **tokyo-night** theme
  (`syntax.rs:9-13,171`). **Wrap:** `textwrap` with a custom `WordSeparator`
  that protects URLs and number formatting (`$145,000`) from breaking
  (`parse.rs:252-341`).
- **Feature breadth (widest of the four):** headings h1–h6, bold/italic/**strike**,
  inline + fenced code, ordered/unordered/**task** lists (deeply nested),
  blockquotes with a quote bar, full box-drawing tables (three presets +
  alignment + shrink-to-fit), OSC-8 links + bare-URL detection, thematic breaks,
  **plus LaTeX math → Unicode** (`E=mc²`) and **Mermaid diagrams → ASCII art**.
- **Streaming — checkpoint-based incremental** (`streaming.rs:125`
  `StreamingMarkdownRenderer`): freeze completed **top-level** blocks
  (`checkpoint.rs:26-58`) and re-render only the tail past the last checkpoint —
  O(N) instead of O(N²). A bounded ambiguous suffix (a trailing `` ` `` that
  might open inline code, a partial `$` LaTeX delimiter) is **held back** until
  more text arrives. Open fenced code keeps a **resumable syntect
  `ParseState`/`HighlightState`** so a large streaming block highlights O(N)
  total. `finish()` does one authoritative full re-render to catch any
  streaming/one-shot divergence; extensive char-by-char equivalence tests.
- **Color downgrade ladder** `None → 16 → 256 → TrueColor` with per-style
  `adapt()` (`colors.rs:12-23`, `style.rs:167`); a fuzz target guards edge cases.

### Claude Code (TypeScript, custom Ink fork) — hybrid lexer + hand-rolled ANSI

- **Parse:** `marked` `^15` — but **only its lexer**; a ~230-line hand-rolled
  `formatToken` walks tokens and emits ANSI strings (`src/utils/markdown.ts:42,49`).
- **Style:** `chalk` for bold/italic/dim + raw ANSI; the string is then re-parsed
  back into styled spans by a custom `<Ansi>` component (tokens → ANSI string →
  spans → React Text nodes). **Tables** bypass this and render as real flexbox
  `<Box>` components (`MarkdownTable.tsx`).
- **Highlight:** `cli-highlight` (wrapping `highlight.js`), **lazy dynamic
  import** behind Suspense — first code block may flash unhighlighted ~50 ms
  (`cliHighlight.ts`, `Markdown.tsx:92-100`). Configurable off.
- **Streaming — incremental boundary split** (`StreamingMarkdown`,
  `Markdown.tsx:186-235`): a monotonic `useRef` boundary splits the buffer into a
  stable prefix (finalized blocks, memoized, never re-parsed) and one growing
  unstable block; each delta lexes only from the last boundary forward. Relies on
  `marked` treating an unclosed fence as a single token so boundaries stay valid.
- **Wrap:** `wrap-ansi` `^9` in the Ink layout layer; custom wide-char-aware
  `stringWidth`. **Quirks:** strikethrough deliberately **disabled** (`~` means
  "approximately"); nested ordered lists change numbering by depth
  (arabic→letters→roman); a 500-entry token cache + a "no markdown syntax" fast
  path skip the lexer for plain text.

### opencode (TypeScript + SolidJS + OpenTUI) — **migrated away from glamour**

Historically remembered as Go + Bubble Tea + Charm **glamour** + chroma. That
stack was **deleted** (commit `f68374ad2` "DELETE GO BUBBLETEA CRAP HOORAY"; zero
`.go` files remain). The current TUI is a TS/SolidJS rewrite on **OpenTUI** (a
Charm-analogue with a **native cell-diffing core**).

- **Markdown is a built-in OpenTUI renderable** — the JSX intrinsic `<markdown>`
  (`routes/session/index.tsx:1685`), not hand-rolled and not glamour. Under the
  hood OpenTUI parses with **`marked`** (`marked@17`, bundled) and highlights
  code with **`web-tree-sitter`** — grammars fetched as **WASM over the network**
  at runtime (`parsers-config.ts`), editor-grade but a real fragility/perf
  surface vs. an embedded highlighter.
- **Theming = a TextMate-scope style map** (`markup.heading.1..6`,
  `markup.bold`, `markup.raw.block`, …) — the direct analogue of a glamour style
  config (`theme/index.ts:793-899`). Native style handles are memory-managed
  (retained until `renderer.idle()`).
- **Streaming — re-render the whole part every delta**, with `streaming={true}`
  telling the parser to **tolerate incomplete markdown**, and
  `internalBlockMode="top-level"` hinting segmentation (`session/index.tsx:1687-1691`).
  **No app-level debounce/throttle** anywhere — flicker is avoided purely by
  SolidJS fine-grained reactivity (only the changed part's `content` prop
  updates) + OpenTUI's native double-buffered cell diff (only changed cells are
  written). Session caps at ≤100 messages for perf.

---

## Comparison matrix

| | **Codex** | **Grok** | **Claude Code** | **opencode** |
|---|---|---|---|---|
| Stack | Rust / ratatui | Rust / ratatui | TS / custom Ink | TS / SolidJS / OpenTUI |
| Parser | pulldown-cmark | pulldown-cmark | marked (lexer only) | marked (in OpenTUI) |
| Code highlight | syntect + two-face | syntect + two-face | cli-highlight / hljs | web-tree-sitter (WASM) |
| Wrapping | textwrap (self) | textwrap (self) | wrap-ansi (Ink) | OpenTUI native |
| Code wrapped? | no (copy/paste) | no | no | no |
| Tables | custom layout | custom box-draw | flexbox `<Box>` | OpenTUI grid |
| Streaming | full re-parse, newline-gated | checkpoint incremental | boundary-split incremental | full re-render/delta |
| Beyond CommonMark | — | LaTeX, Mermaid | issue-ref links | — |
| Highlight cap | 512KB/10k lines | resumable state | lazy-load | network WASM |

---

## Where `locode-tui` stands today

`crates/locode-tui/src/ui/markdown.rs` (310 lines) already does the *right shape*:
a `pulldown-cmark` pass → hand-rolled `Writer` → ratatui `Line`s, following
codex's `markdown_render.rs` pattern. It covers headings (bold), nested lists,
inline/fenced code (**dim, not highlighted**), blockquotes, inline bold/italic,
and self word-wraps. Deliberate v0 non-goals (per the module doc): **no syntect,
no tables, no streaming** (we don't stream yet — streaming is a deferred
core-touching feature).

So our **parser choice already matches both Rust twins.** The gap is entirely in
the styling depth + missing features, not the architecture.

---

## Recommendation for locode

Phased, cheapest-visible-win first. Each phase is a self-contained TUI slice.

**Phase 1 — syntax-highlighted code blocks (biggest visible win).**
Adopt **`syntect` + `two-face`**, exactly as codex and grok both do — this is the
single change that most closes the "looks like a toy" gap. Mirror codex's
choices: **omit backgrounds** (let the terminal bg show), guard with a
size/line cap (512 KB / 10k lines), theme behind swappable state. two-face gives
bat's ~250 grammars for free. **Dependency note (needs your OK):** `syntect`
pulls `onig`/`fancy-regex` + `two-face` bundles grammar/theme binaries — the
heaviest deps we'd add to the TUI, though both Rust references accept exactly
this cost and it's `rustls`-clean (no OpenSSL). This is the one real decision in
this doc.

**Phase 2 — proper wrapping + a few polish features.**
Replace our hand-rolled wrap with **`textwrap`** (codex/grok both use it),
preserving inline styles; keep **code unwrapped** for copy/paste. Add
strikethrough styling (already parse it), ordered-list markers, and OSC-8
hyperlinks (iTerm2 — your terminal — supports them). Small, low-risk.

**Phase 3 — tables.** A bespoke column-width + cell-wrap pass with a key/value
transpose fallback when too wide (all four do this). Meaningful work; schedule
only when agent output actually uses tables enough to matter.

**Phase 4 — streaming (only when the streaming core lands).**
Streaming is a deferred core-touching feature (off the vibe-coding autopilot).
When it arrives, adopt **codex's model as the default**: buffer deltas,
**re-parse the whole buffer gated at newline boundaries**, split
stable-prefix / mutable-tail. It is the simplest design that gives the
provable "streamed frame == final frame" guarantee, and it's directly
copyable into our ratatui `Line` pipeline. Grok's checkpoint-incremental
renderer is the performance upgrade if profiling ever shows the full re-parse
is too costly at our scale — but start simple. **Do not** hand-stitch an
incremental token tree; none of the four do, and it's the bug-prone path.

**Explicitly not recommended:** a high-level all-in-one renderer (glamour-style)
— opencode *migrated away* from exactly that, and it doesn't map onto our
`pulldown-cmark → ratatui Line` pipeline. LaTeX/Mermaid (grok) are neat but far
outside scope. Network-fetched tree-sitter grammars (opencode) trade robustness
for grammar breadth we don't need — `syntect`'s embedded set is the right call
for a single-binary agent.

**Next step:** if you approve Phase 1 (the `syntect`/`two-face` dependency),
this becomes an ADR (markdown rendering decision) + a TUI slice task. Phases 2–4
are follow-on slices tracked in `tasks/tracker.md`.
