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
