# TUI text wrapping & wide characters (CJK) — bug fix + future upgrade plan

Conducted 2026-07-23 against the `coding-cli-survey` submodules, prompted by a real
bug: **CJK (Chinese/Japanese/Korean) text was truncated on the right edge** in the
interactive TUI — both the user's echoed prompt and the model's response. This note
records the root cause, the shipped surgical fix, and the **planned comprehensive
upgrade** to a mature wrapping stack (so we can revisit deliberately, not from
scratch).

## The bug

Our two hand-rolled wrappers measured width by **character count**, not terminal
**display width**:

- `crates/locode-tui/src/ui/markdown.rs::wrap_words` — the assistant/markdown path.
- `crates/locode-tui/src/ui/blocks.rs::wrap_plain` — the user-prompt band path.

A han character is **one `char` but two terminal cells**. CJK text also has **no
spaces**, so a whole sentence is a single "word" that hits the hard-split path — which
split at `avail` *characters* (`&rest[..avail]`), i.e. `2 * avail` *cells*. Every
wrapped line was ~2× too wide, so the vendored frame (ADR-0022) clipped it at the
right margin → the reported truncation. Table cells had the sibling bug (`col_natural`
/ `pad_into` measured by char count → misaligned borders).

## The shipped surgical fix (2026-07-23)

Minimal, low-risk, keeps our wrapper; measures **display width** via the
`unicode-width` crate (the same crate + `"0.2"` pin the codex and grok-build TUIs
use; already in our tree via ratatui). ~5 measurement sites changed, ~140 lines of
wrapping code total (`wrap_words` 61 + `wrap_plain` 51 + helpers):

- `ch_width(char)` = `UnicodeWidthChar::width(ch).unwrap_or(0)` (CJK/emoji = 2,
  zero-width = 0); `word_width`, and `prefix_by_width` — a per-scalar display-width
  accumulator that hard-splits an over-wide run on the **cell** boundary, always
  taking ≥1 char (codex's `take_prefix_by_width` guard against a lone wide char in a
  width-1 column).
- `wrap_words` / `wrap_plain` now track line width in cells; lead/marker/indent widths
  and table `col_natural` / `pad_into` use `UnicodeWidthStr::width`.
- Tests: space-less CJK paragraph, mixed CJK+ASCII, CJK table alignment, `wrap_plain`
  CJK, ASCII-unchanged regressions.

**What the surgical fix does NOT do** (the reason for the upgrade below): it iterates
by Unicode **scalar** (`char`), not **grapheme cluster**, and has **no UAX#14
line-break opportunities**. So it is correct for CJK and common text, but:

- ZWJ/emoji sequences and combining marks can be split mid-cluster.
- Breaks in CJK land between *any* two chars (no rule keeping closing punctuation
  「。』」 off a line start, or keeping a number+unit together).
- No optimal-fit (ragged-right minimization) or hyphenation.

## How the four harnesses do it (research)

| Harness | Stack | Width primitive | Wrap engine | CJK line-breaks |
|---|---|---|---|---|
| **codex** | Rust + ratatui | `unicode-width` 0.2 (`UnicodeWidthChar/Str`) | **`textwrap` 0.16.2** (default features) via `wrap_ranges_trim` → styled-span re-slice (`tui/src/wrapping.rs`, `live_wrap.rs`) | **UAX#14** (`unicode-linebreak`) + `break_words(true)` hard-split |
| **grok-build** | Rust + ratatui | `unicode-width` 0.2 | **`textwrap` 0.16.2** via `word_wrap_line*` (`render/wrapping.rs`); tables **clipped+padded, never wrapped** (`fit_line_to_width`) to avoid wide-glyph desync | UAX#14 + `break_words` |
| **claude-code** | TS + Ink | `get-east-asian-width` + `emoji-regex` + `Intl.Segmenter` (grapheme) / `Bun.stringWidth` | `wrap-ansi` (`{hard:true}`) | grapheme + string-width hard-split (no UAX#14) |
| **opencode** | TS + Solid/@opentui | `Bun.stringWidth` + `Intl.Segmenter` (grapheme) | external `@opentui/core` (out of repo) | grapheme (no UAX#14 in-repo) |

**The pattern both Rust references share:** delegate to **`textwrap` 0.16 with default
features**, which pulls `unicode-linebreak` (UAX#14) + `unicode-width` + `smawk`. That
gives, for free: display-width measurement, **break opportunities *between* CJK
characters** (UAX#14 — solves the space-less-run problem *properly*), optimal-fit, and
`break_words(true)` as the hard-split fallback. Both then wrote a **styled-span
adapter** (`wrap_ranges_trim` / `word_wrap_line`) to map textwrap's flat-string
byte-ranges back onto ratatui `Span`s — this is why their wrapping files are large
(codex `wrapping.rs` 1657 LOC, grok `render/wrapping.rs` 1559 LOC; the *essential*
adapter core is ~150–250 LOC).

## Code-size analysis (bespoke vs. adopting textwrap)

| | Wrapping LOC | New deps | Risk |
|---|---|---|---|
| **Bespoke + surgical fix (shipped)** | ~140 | `unicode-width` (already in tree) | Low — 5 measurement edits, no algorithm rewrite |
| **Adopt textwrap (codex/grok style)** | ~200–250 core adapter (their full files 1500+) | `textwrap` + `unicode-linebreak` + `smawk` | Higher — rewrite the inline→wrap pipeline of a ~100-test markdown renderer |

Adopting textwrap is **not** "delete our code, call one function": integrating it into
our per-char-styled word model (`Vec<(char, Style)>`) requires the same flat-string ↔
styled-span byte-range adapter codex/grok wrote. Worthwhile, but a deliberate task.

## TODO — comprehensive wide-char wrapping upgrade (future)

Revisit as a dedicated task (not urgent — the surgical fix resolves the visible bug).
Adopt the codex/grok stack and cover the features the surgical fix omits:

- [ ] Add `textwrap` 0.16 (default features → `unicode-linebreak` UAX#14 + `smawk`
      optimal-fit + `unicode-width`). **Verify `unicode-linebreak` is in `Cargo.lock`**
      — without it `WordSeparator::new()` silently falls back to `AsciiSpace` and CJK
      becomes one unbreakable word again.
- [ ] Rewrite `wrap_words` (and fold in `wrap_plain`) on the codex `wrap_ranges_trim` +
      `word_wrap_line` model: flatten the styled line to a string with byte-tracked span
      bounds, `textwrap::wrap` with `break_words(true)` + `WordSeparator::new()` +
      `WrapAlgorithm::OptimalFit`, then re-slice spans by byte range.
- [ ] **Grapheme-cluster** iteration (`unicode-segmentation`) so ZWJ/emoji sequences and
      combining marks never split mid-cluster (claude/opencode use `Intl.Segmenter`).
- [ ] UAX#14 punctuation-aware breaks (don't strand CJK closing punctuation at a line
      start; keep number+unit runs together) — free with `unicode-linebreak`.
- [ ] Adopt grok's rule: **box-drawing / table rows are clipped+padded to content
      width, never word-wrapped** (`fit_line_to_width`), to avoid terminals that render
      a wide glyph wider than measured stranding a "ghost cell" at the right edge.
- [ ] Decide the **ambiguous-width** policy (East-Asian ambiguous chars: 1 or 2 cells;
      claude uses `ambiguousIsNarrow: true`). Pin it and test.
- [ ] Emoji width regressions (VS16 `⚠️`, flags, skin-tone ZWJ) — codex/grok both have
      dedicated tests worth porting.

**Source anchors to model on:** codex `codex-rs/tui/src/wrapping.rs::word_wrap_line`
(659) + `live_wrap.rs::take_prefix_by_width` (182); grok
`xai-grok-pager-render/src/render/wrapping.rs::word_wrap_line` (267) + `fit_line_to_width`
+ `is_table_line` (323).
