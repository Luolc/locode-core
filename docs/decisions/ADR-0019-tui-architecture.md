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
   **→ Superseded by [ADR-0022](ADR-0022-vendored-terminal-relative-frame.md)
   (2026-07-22):** to get Claude Code's dynamic bottom-pinned composer, the
   fixed-height inline + `insert_before` model is replaced by a minimal
   *vendored* terminal running a relative-frame render (LF-commit to native
   scrollback, erase-up on shrink). Everything else in this ADR stands.
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

## Amendment (2026-07-22): rendering decision superseded by ADR-0022

The **Rendering** decision (§Decision.2) — fixed-height `Viewport::Inline` +
`insert_before`, "no terminal forks", with dynamic height deferred and
codex-style DECSTBM writes rejected — is **superseded by
[ADR-0022](ADR-0022-vendored-terminal-relative-frame.md)**. To deliver Claude
Code's dynamic, bottom-pinned composer (grow/shrink with the transcript, caret
glued to the bottom line, no scrollback blanks), `locode-tui` gains a **minimal
vendored terminal** running a *relative-frame* render: the composer is painted
last (bottom-pinning is emergent), growth commits overflow to native scrollback
via `LF`, and shrink erases up + repaints (never a reverse scroll). Two
incremental attempts on this ADR's model — `scroll_region_down` (scrollback
blanks) and tail-in-viewport (streaming collision) — are documented there as
rejected. The crate shape, reducer loop, runtime, and robustness decisions of
this ADR are unchanged; only the rendering substrate moves.

## Amendment (2026-07-22): user-prompt shaded band + right-aligned footer clock

Two user-directed chrome refinements, grounded in a fresh re-read of grok-build
and codex (AGENTS.md "planning is a research task"):

- **User prompt renders as a full-width shaded band** (was `❯ `-prefixed dim
  text). Both reference harnesses draw the user's message as a pure background
  fill — no border: grok paints every cell of the block rectangle with
  `theme.bg_light` (`xai-grok-pager/src/scrollback/wrappers/entry_renderer.rs`
  fill loop; `RenderBlock::UserPrompt`), codex sets the line bg and issues
  `Clear(UntilNewLine)` (`codex-rs/tui/src/insert_history.rs`;
  `history_cell/messages.rs` `user_message_bg`), each with one blank shaded row
  of vertical padding above and below. We adopt the shape: a leading unshaded
  separator, a top vpad row, the `❯ `-prefixed wrapped text (col 4, aligned with
  the assistant bullet and composer input), a bottom vpad row; each band row is
  space-padded to the full width so the fill spans edge-to-edge (ratatui styles
  only the cells a span covers). **Background is `Color::DarkGray`** — the ANSI
  bright-black palette slot, so the band follows the user's terminal theme rather
  than a hard RGB, the same palette-relative approach as code highlighting
  (`ui/highlight.rs`). Codex's terminal-bg-detection blend and grok's per-theme
  `bg_light` are deferred to a future color-theme system. **No timestamp in v1**
  (grok's is off by default behind `/timestamps`; adding one needs a time field
  on the block and would break `Block: PartialEq` determinism) — deferred.
  Implemented in `ui/blocks.rs` (`render_user_prompt`).

- **Footer clock**: the bottom status row right-aligns the current local date +
  `HH:MM` (grok/codex both surface a wall clock). Uses `chrono::Local`, which
  honors the `TZ` env var and `/etc/localtime`, so `TZ=America/Los_Angeles
  locode` (or a shell that exports `TZ`) sets the zone — no in-app timezone
  config, matching a zsh status bar (user's mental model, their server and
  workstation differ in zone). **No timezone label**: `Local`'s `%Z` can only
  print the numeric offset (`-07:00`, not `PDT`) because chrono has no zone
  abbreviation; a real `PST/PDT` label would need a tz-database dep, deferred as
  ask-first (user preferred dropping it, 2026-07-22). **Minute precision** (not
  seconds) because the loop has zero idle repaints (`event_loop`: animation ticks
  only while a run is active) — the clock refreshes on the next paint, like a
  shell prompt, and a seconds display would look frozen between keystrokes. When
  the row is too narrow to fit both status and clock, the clock is dropped.

- **Footer component colors** (user scheme, 2026-07-22): the left status
  components render **bold**, each in its own terminal-relative (ANSI-named,
  theme-following) color — cwd `LightBlue`, model `Gray`, tokens `Red`
  ("Cayenne"), clock `Gray`; the ` · ` separators stay **dim and un-bold** (the
  same lighter gray as dimmed output) so the colored components read as the
  foreground. Exact-RGB pinning is deferred to the future color-theme system.
  Implemented in `ui.rs` (`footer_left` / `footer_clock` / `compose_footer`).
