# ADR-0022: Dynamic composer via a vendored terminal + relative-frame rendering

## Status
Accepted — supersedes the **Rendering** decision of
[ADR-0019](ADR-0019-tui-architecture.md) (§Decision.2 and the two related
alternatives/deferrals). ADR-0019's crate shape, reducer loop, runtime, and
robustness decisions stand unchanged.

## Date
2026-07-22

## Context
ADR-0019 chose a **fixed-height** inline viewport (`Viewport::Inline(N)`) plus
`insert_before` for finalized transcript, explicitly deferring a *dynamic*
live-region height and rejecting terminal forks. On smoke feedback the user
wants Claude Code's **dynamic composer**, precisely:

- the composer's bottom rule + status line are pinned to the very bottom of the
  screen, always (even on a freshly-`clear`ed terminal);
- **Shift+Enter** grows the composer — its top edge and the transcript above
  rise together; **Backspace** shrinks it — both come back down, symmetric;
- the caret stays glued to the composer's bottom line while the draft overflows
  the height cap;
- scrolling the terminal back up shows clean native scrollback — **no blank
  rows injected**.

Multiple incremental attempts on the fixed-inline + `insert_before` architecture
failed, each in an instructive way (all on branch
`feat/tui-dynamic-viewport-v2`, PR #102):

1. **`scroll_region_down` on shrink** to pull the transcript back down — injects
   blank rows that later scroll into native scrollback (visible as gaps on
   scroll-back). Scrollback corruption.
2. **Rendering the transcript tail *inside* the repainted viewport** so it moves
   with the composer — collides with `insert_before` during streaming (the
   escape-sequence insert interleaves with the buffer diff), corrupting the
   display (frame rules mid-screen, overlapping text).

A four-harness source study (agent research, 2026-07-22; citations below)
re-framed the problem. The decisive finding: **the target behavior is Claude
Code's relative-frame model, not codex's reserved-viewport-rect model.** The
reserved-rect model (codex, grok) *requires* moving the viewport's `y` on
shrink, which forces either a reverse scroll (blanks — failure 1) or re-emitting
the vacated rows. Claude Code sidesteps this entirely: it owns no rect; the
composer is simply the last thing painted, so bottom-pinning is emergent and
shrink is an erase-up + repaint that **structurally cannot** write blanks into
scrollback.

### Source study (`~/dev/coding-cli-survey/submodules/`)
- **codex** — reserved viewport `Rect` at a fixed screen `y`; only the
  active/streaming cell + composer are diff-painted; finalized history is
  committed to native scrollback via a reverse-index (`ESC M`) choreography
  inside `DECSTBM`, run *before* the viewport diff, all in one synchronized
  update. `tui/src/custom_terminal.rs:145-168,299-323,357-438,500-502`;
  `insert_history.rs:193-245,331-357`; `tui.rs:772-804,815-851,1016-1067`;
  `chatwidget/rendering.rs:6-60`; `app/resize_reflow.rs`. Its shrink path
  deliberately does **not** reverse-scroll (`tui.rs:833`) — it re-emits from
  transcript source. This is correct but heavy, and *not* bottom-pin-symmetric
  without the reflow machinery.
- **grok-build** (`crates/codegen/xai-ratatui-inline`) — same vendored ratatui
  `Terminal` with a two-buffer diff; app owns scrollback as a `String`; commits
  with `emit_to_scrollback` (`\x1b[J` clear + reprint + reserve rows); on
  SIGWINCH **purges screen + scrollback and re-prints the whole history**
  (`resize_purge_rerender`). Notable extras worth porting: **`diff_large`**
  (fixes ratatui's `u16` `pos_of` overflow on >65 535-cell terminals,
  `terminal.rs:1141`) and an **OSC-8 hyperlink diff layer**
  (`flush_with_links`, `terminal.rs:363`). Its viewport is *not* bottom-pinned
  and its shrink leaves a gap at the bottom (`resize.rs:171-180`).
- **claude-code** (vendored Ink, `src/ink/`) — **the target model.** One
  relative-cursor "frame" (`log-update.ts`) diffed against the previous, all
  moves relative and assuming the physical cursor is at the screen bottom. No
  reserved rect, no scroll region for the live block. Grow renders new rows with
  `CR`+`LF` (the **LF scrolls the terminal**, pushing overflow into native
  scrollback); shrink is `clear(N)` from the bottom + repaint. A `viewportY`
  counter tracks rows already scrolled into scrollback; any diff targeting a row
  `< viewportY` triggers a full clear+repaint instead of an (impossible) edit.
  `log-update.ts:123,136-147,187-206,258-283,285-301,308-388,403-412,503-513,527-616`;
  `ink.tsx:568-595`.
- **opencode** — the checked-out build is the OpenTUI/SolidJS **full-screen
  alt-screen** renderer; not an inline-viewport reference. Confirms the only
  other viable architecture is "own the whole screen, never touch native
  scrollback" — which ADR-0019 already rejected for v1.

## Decision

Adopt **Claude Code's relative-frame model as primary**, implemented on a
**minimal vendored terminal** (a trimmed fork of ratatui's `Terminal`/`Frame`),
with **codex's ordering discipline** (commit-before-diff, one synchronized
update per frame). Concretely:

1. **Vendor a terminal.** Introduce `locode-tui`'s own `Terminal`/`Frame`
   (evolving the existing `inline_terminal.rs`), a two-buffer cell diff over
   ratatui's public `Buffer`/`Backend`. This reverses ADR-0019's "no terminal
   forks" — accepted, because stock ratatui cannot express a bottom-pinned
   relative frame with LF-commit. Port grok's `diff_large` to avoid the
   `u16`-overflow bug; keep the OSC-8 layer as a named later extension.

2. **One repainted frame, bottom-anchored, no reserved rect.** Each frame paints
   `[recent transcript tail] + [status] + [composer]` as the *last* thing on the
   screen, with the physical cursor ending at the bottom. "Bottom-pinned" is
   emergent, not a rect we move. Finalized transcript that has scrolled above the
   frame lives in **native scrollback** and is never repainted.

3. **Grow / shrink algorithm** (relative to a virtual cursor at the bottom):
   - **grow by k** (Shift+Enter): render the k new rows with `CR`+`LF`; the LF
     scrolls the top of the block (and any overflow) into native scrollback. The
     transcript above rises naturally. No scroll-region op.
   - **shrink by k** (Backspace): erase-up k lines from the bottom (`clear(k)` =
     cursor up k−1 + clear-to-end-of-screen), then repaint the shorter block. No
     reverse scroll → **no blank injection**.
   - **caret glued to bottom line:** the composer textarea scrolls internally so
     the caret's screen row is `min(logical_row − scroll, cap−1)`; the block
     height is capped (~50% of the screen).

4. **The `viewportY` guard.** Track how many rows have scrolled into native
   scrollback. If a frame diff would need to write a row above the frame top
   (i.e. into committed scrollback), fall back to a **full clear + repaint** of
   the frame rather than attempting the (impossible, corruption-causing) edit.
   This is the invariant that makes committed scrollback permanently safe.

5. **Terminal (SIGWINCH) resize: do not predict reflow.** On a size change,
   full-reset + repaint the frame (Claude Code's choice). Because our blocks own
   their source text, re-emitting recent transcript from source (codex's choice)
   is an available upgrade; v1 takes the simpler full-reset.

6. **One writer per frame, one synchronized update.** Wrap every frame's output
   in DEC 2026 `BeginSynchronizedUpdate`/`EndSynchronizedUpdate`. There is **no
   separate `insert_before` path** interleaving with the diff — the streaming
   transcript is part of the one repainted frame; finalized rows leave the frame
   only by scrolling off the top via LF. (If a distinct commit step is ever
   needed, it runs strictly *before* the viewport diff, codex-style.)

7. **Minimal vendored surface** (union of codex + grok, trimmed):
   `Terminal { backend, buffers:[Buffer;2], current, frame_area, last_size,
   last_cursor, viewport_y }`; `draw(FnOnce(&mut Frame))` = autoresize → render →
   diff-flush → cursor placement → swap; `set_frame_area`, `invalidate()` (reset
   prev buffer to force a full repaint after any raw op), `commit`/grow/shrink
   helpers; `Frame { cursor_position, area, &mut Buffer }`.

## Alternatives Considered
- **Codex reserved-viewport-rect + `insert_history` (DECSTBM reverse-index).**
  Rejected as the primary model: its shrink path is not blank-free *and*
  bottom-pinned without the full resize-reflow/`HistoryCell` re-emit machinery —
  more code, and it optimizes for a split (streaming cell in-rect, history in
  scrollback) we don't need at v1. We borrow its *ordering discipline*, not its
  geometry.
- **Grok `xai-ratatui-inline` app-owned-`String` + purge-rerender.** Rejected:
  viewport not bottom-pinned, shrink gaps at the bottom. We port its
  `diff_large` fix and (later) OSC-8 layer only.
- **Stock `insert_before` + `scroll_region_down` on shrink** (attempt 1).
  Rejected: injects blanks into native scrollback.
- **Transcript tail rendered inside the repainted viewport** (attempt 2).
  Rejected: collides with `insert_before` during streaming; corrupts the frame.
- **Alt-screen with app-owned scrollback** (opencode/grok-fullscreen). Rejected
  again per ADR-0019 — defers the 5–50k LOC of selection/search/folding and
  gives up native scrollback.

## Consequences
- We now **maintain a vendored terminal** (small, `Buffer`/`Backend`-based,
  TestBackend-tested). This is the deliberate reversal of ADR-0019's
  "no terminal forks"; scoped to `locode-tui`, core stays headless.
- The repaint surface is larger than the fixed-inline model (a screenful in the
  worst case) but bounded and diffed; still no alt-screen, RSS stays bounded
  (Claude Code's 59 GB post-mortem was an *unbounded owned buffer*, which we do
  not keep — overflow goes to native scrollback via LF).
- **On terminal resize, old wraps are rebuilt by full-reset** (or, later,
  re-emit from source) instead of ADR-0019's "resize keeps old wraps."
- `insert_before` as a public step goes away for the interactive path; the
  block-render-to-`Line` code is reused inside the frame.
- Geometry is unit-testable against `TestBackend` (grow/shrink/commit/viewportY
  guard/resize) before any on-device smoke — the process this project follows to
  get it right in one build.

## Reconciliation
ADR-0019 §Decision.2 (fixed-height `Viewport::Inline` + `insert_before`; "no
terminal forks") and its "Codex-style DECSTBM … rejected" / "Dynamic
live-region height … deferred" notes are **superseded by this ADR**. A pointer
is added to ADR-0019. SPEC-TUI's rendering-model section is updated in the same
change.
