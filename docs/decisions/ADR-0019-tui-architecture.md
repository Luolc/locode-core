# ADR-0019: TUI architecture — inline print-once transcript, reducer loop, library-plus-thin-binary

## Status
Accepted

## Date
2026-07-21

## Context
Task 27 builds the interactive frontend (`locode-tui` + `locode-app`) on the
0.1.4 engine seams. The design space was mapped by a four-harness TUI source
study ([`docs/research/tui-harness-study.md`](../research/tui-harness-study.md));
scope and slices live in [`SPEC-TUI.md`](../../SPEC-TUI.md); the autonomous
process in [`docs/tui-dev-process.md`](../tui-dev-process.md). This ADR records
the load-bearing architectural decisions so they survive the spec's churn.

## Decision

1. **Crate shape**: `locode-tui` is ONE library crate holding the entire app
   behind `main_with(ProviderRegistry) -> ExitCode`; `locode-app` is a
   flag-free binary (command name **`locode`** — bin names are not namespaced
   on crates.io, so the taken `locode` crate name does not block it). The
   ADR-0015 exec pattern, and grok's `pager`/`pager-bin` split. Splits happen
   only on SPEC-TUI's named triggers.
2. **Rendering**: stock ratatui `Viewport::Inline` (fixed-height live region)
   + `insert_before` for finalized transcript blocks — native scrollback owns
   settled history; the repaint surface is bounded (status + composer +
   overlay). Grok's Minimal mode / codex's signature, chosen against Claude
   Code's documented repaint-region failure (59 GB RSS post-mortem). No
   alt-screen, no owned scrollback, no terminal forks.
3. **Runtime**: dedicated input-reader OS thread (`poll+read` → mpsc; never
   `EventStream` in `select!` — crossterm #936); one biased `tokio::select!`;
   engine-event arm gated on an empty input queue with bounded batch drain;
   event-driven draw capped at ~16 ms; zero idle wakeups.
4. **State**: one `App` struct + sans-IO reducer `Msg → update(&mut App, now)
   → Vec<Cmd>`; the loop owns all IO; `now` injected for testability.
   Namespaced enums from day one.
5. **Robustness floor from slice 1**: teardown sequence defined once
   (idempotent), shared by exit/error/panic-hook/signal paths; SIGINT/SIGTERM
   → graceful quit task; bracketed paste with CR normalization; resize
   debounce; bounded channels.
6. **Core stays untouched**: the TUI consumes only the facade's four seams
   (ADR-0014/0016/0017/0018). The one flagged core proposal (engine
   `decide()` await observing the cancel token) rides its own reviewed PR if
   adopted.

## Alternatives Considered
- **Alt-screen with app-owned scrollback** (grok fullscreen, opencode):
  rejected for v1 — 5–50k LOC of selection/search/folding machinery for
  interactivity the spec defers.
- **Codex-style DECSTBM scroll-region history writes**: rejected — requires
  forked ratatui/crossterm; stock `insert_before` is the 90% answer.
- **Dynamic live-region height** (codex custom terminal): deferred — fixed
  rows with bottom-anchored layout reads as margin; revisit on smoke feedback.
- **A wire-protocol seam to the engine** (codex JSON-RPC in-process):
  deferred — typed channel messages give the same client purity at v1 scale;
  extraction is localized in the engine-task module.

## Consequences
- Printed history cannot be rewritten (no cancel-rewind polish; resize keeps
  old wraps) — accepted; blocks own their source so reflow-from-source is the
  documented extension.
- The composer widget (`tui-textarea`) is wrapped behind one module so
  replacing it (grok/codex both ended up with custom editors) is local.
- Streaming deltas, kitty keyboard, mouse, themes, multi-session all have
  named extension points in SPEC-TUI; none require re-architecture.

## Amendment (2026-07-21): the `locode` binary unifies TUI + `-p` headless

`locode` becomes the single entry point for both modes (Task 28), matching
Claude Code (`claude` interactive / `claude -p "…"` headless) and grok
(`grok -p`). `locode_tui::main_with` detects `-p`/`--print` and dispatches:

- **default** → the interactive TUI (this ADR's architecture);
- **`-p`** → a headless one-shot (no terminal setup), reusing
  `locode-exec`'s engine via the new `locode_exec::run_headless(cli,
  registry)` — the same session assembly, `--output-format` emit (ADR-0009),
  and SIGTERM handling. The two CLIs share `Harness`/`OutputFormat`; the
  headless prints stay inside `locode-exec`'s audited stdout writers, so the
  workspace print-ban is not weakened.

`locode-tui` therefore depends on `locode-exec` (lib) for now. **Retire plan:**
`locode-exec` (the binary) is slated for removal after this version, and the
installers will ship `locode` instead of `locode-exec`; when that happens the
headless logic (`run.rs`/`output.rs`/`signal.rs`) migrates into `locode-tui`
or a shared lib, and the `locode-tui → locode-exec` edge is dropped. Until
then the reuse avoids duplicating the proven headless path.

A bare positional prompt (`locode "task"`) pre-fills the composer in TUI mode
(not auto-sent). The publish/installer switch to `locode` remains a user-gated
release decision.

## Amendment (2026-07-22): the `locode-exec` *binary* is retired

User decision (2026-07-22): stop shipping the standalone `locode-exec` binary;
release only the unified `locode`. Done in this change:

- `release.yml` drops the second `upload-rust-binary-action` step — only
  `locode-<target>.tar.gz` is attached to a Release now (ADR-0010 amendment,
  same date). `install.sh` already installs `locode` (since 0.1.5).
- README stops advertising the `locode-exec` binary / `cargo install
  locode-exec`; headless examples use `locode -p`.

**Not yet done (the remaining retire step):** `locode-exec` stays as a
*published library crate* because `locode-tui` still calls its
`run_headless`. Fully collapsing it — migrating `run.rs`/`output.rs`/
`signal.rs` into `locode-tui` or a shared lib and dropping the `locode-tui →
locode-exec` edge — is deferred; it is a mechanical move with no user-visible
change, scheduled when the headless/TUI split is next touched. Until then the
`locode-exec` *crate* remains, only its binary target is no longer released.

## Amendment (2026-07-22): dynamic live-region height via a vendored inline terminal

Supersedes the fixed-height decision. A first attempt to grow the region by
recreating ratatui's `Terminal` (PR #99) **blinked** — every resize did a
`Clear` + full repaint and could not scroll the transcript as a block. Confirmed
by smoke; reverted.

**Root cause.** Stock ratatui 0.29 cannot change an inline viewport's height:
`Terminal::viewport` / `set_viewport_area` are private and `resize()` is
hardwired to the stored height. Both Rust/ratatui references solve this by
**vendoring a terminal** (codex `tui/src/custom_terminal.rs`; grok
`xai-ratatui-inline`) that owns a mutable `viewport_area` and resizes it with
DECSTBM **scroll regions** so content moves as one block (no clear/repaint).

**Decision.** Vendor a *minimal* inline terminal, `term::inline` — a `Frame` +
`InlineTerminal` — that reuses ratatui's **public** `Buffer`/`Backend` (so we do
NOT copy the diff engine or the crossterm backend), and adds exactly what stock
ratatui withholds:
- `draw(f)`: render into a back buffer, `Buffer::diff` vs the front buffer,
  `Backend::draw` the delta, flush, swap+reset (ratatui's own loop).
- `insert_before(lines)`: the transcript path — `scroll_region_up` above the
  viewport, then draw the freed rows (ratatui's `insert_before_scrolling_regions`
  recipe), so native scrollback still owns settled history (ADR-0019 unchanged).
- `set_height(rows)`: the new capability. The viewport stays **bottom-anchored**;
  on **grow** `scroll_region_up` scrolls the transcript up to make room; on
  **shrink** `scroll_region_down` drops the transcript back down. No gap between
  transcript and composer, ever; no clear/blink.

`ui::draw`/`composer`/approval take `term::inline::Frame` (same `area()` /
`render_widget()` surface as ratatui's, so the change is mechanical). The event
loop computes a desired row count (`ui::desired_live_rows`, unit-tested) and
calls `set_height` before each paint, clamped to `[MIN, ~50% of the terminal]`.

**Edge cases to test** (against ratatui `TestBackend`, which implements the
scroll-region ops + a `scrollback()` buffer, so the geometry is validated
headlessly; only raw escape-sequence behavior needs a real-terminal smoke):
1. grow by 1 and by many rows; transcript moves up, viewport bottom-anchored;
2. shrink by 1 and by many; transcript moves back down; no gap; no bottom blank;
3. grow past what fits above (transcript shorter than the delta) → pulls blanks,
   never panics/underflows;
4. clamp at `MIN_LIVE_ROWS` and at ~50% of the terminal;
5. `insert_before` while at a non-default height (transcript still lands
   correctly above the resized viewport);
6. a terminal resize (`SIGWINCH`) mid-session re-anchors without corruption;
7. no-op when the requested height equals the current height (no escape output).

**Fallback.** If a terminal mishandles the scroll-region ops in the wild, the
seam is one method (`set_height`); it can degrade to a fixed height without
touching app logic.
