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

## Amendment (2026-07-23): the collapse is rejected; `locode-exec` stays a standalone library

User decision (2026-07-23), superseding the "remaining retire step" above:
**`locode-exec` is *not* collapsed into `locode-tui`.** It stays a standalone
library crate, so a headless-only consumer can depend on `locode-exec` (or the
`locode-core` facade) **without** pulling in the TUI — collapsing the headless
logic into `locode-tui` would force exactly that unwanted dependency. Done in this
change:

- **The `locode-exec` binary target is removed** (`src/main.rs` deleted). The crate
  is now library-only; `run_headless` + `main_with` remain as the headless library
  entry (downstream custom-provider headless binaries call `main_with(registry)`).
- The `locode-tui → locode-exec` **library** edge is **kept** (the deferred
  collapse/edge-drop is cancelled, not just postponed).
- The CLI end-to-end tests moved from `locode-exec/tests/cli.rs` onto the shipped
  `locode` binary at `locode-app/tests/cli.rs` (same `run_headless`, now under `-p`).

Net crate roles: `locode-app` owns the shipped `locode` binary; `locode-exec` is the
headless-runner library; `locode-tui` is the TUI library. No user-visible change.

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

- **Footer status bar: two rows, a component pinned in each corner** (user
  layout, 2026-07-22) — cwd top-left, clock+time top-right, model bottom-left,
  session tokens bottom-right. Replaces the earlier single-row `cwd · model · N
  tok  …  clock`; the ` · ` separators are gone (each corner stands alone). The
  frame footer is now `FOOTER_ROWS = 2` (grep: `live_rows`/`draw`). The tokens
  corner is **always shown** (a fresh 0-token session renders `0 tokens`) so the
  corner never looks empty. A pending armed-key hint replaces the cwd corner.

- **Footer clock**: the top-right corner shows the current local date + `HH:MM`
  behind a Nerd Font clock icon (`nf-fa-clock_o`, `\u{f017}`). Uses
  `chrono::Local`, which honors the `TZ` env var and `/etc/localtime`, so
  `TZ=America/Los_Angeles locode` (or a shell that exports `TZ`) sets the zone —
  no in-app timezone config, matching a zsh status bar (user's mental model,
  their server and workstation differ in zone). **No timezone label**: `Local`'s
  `%Z` can only print the numeric offset (`-07:00`, not `PDT`) because chrono has
  no zone abbreviation; a real `PST/PDT` label would need a tz-database dep,
  deferred as ask-first (user preferred dropping it, 2026-07-22). **Minute
  precision** (not seconds) because the loop has zero idle repaints (`event_loop`:
  animation ticks only while a run is active) — the clock refreshes on the next
  paint, like a shell prompt, and a seconds display would look frozen between
  keystrokes. When the row is too narrow to fit both corners, the right one is
  dropped.

- **Footer component colors** (user scheme, sourced from the user's
  `ccstatusline` config `~/.config/ccstatusline/settings.json`, 2026-07-22):
  each corner is **bold** in its own terminal-relative (ANSI-named,
  theme-following) color — cwd `LightBlue` (matches the config's
  `current-working-dir: brightBlue`), model `Gray`, tokens `Red` ("Cayenne");
  the clock/time is **dim** (the same lighter gray the old separators used) and
  not bold. Exact-RGB pinning is deferred to the future color-theme system.
  Implemented in `ui.rs` (`footer_lines` / `footer_row` / `footer_clock`).

## Amendment (2026-07-26): the teardown owns every mode it turned on

Decision §5's robustness floor ("teardown sequence defined once (idempotent),
shared by exit/error/panic-hook/signal paths") was true of the *sequence* but
not of its *reach*, and the gap shipped a broken shell:

- **The error path never reached it.** `event_loop::run` restored the terminal
  only on the `break`; every `?` in the loop (`paint`, `terminal.size`,
  `terminal.clear`) returned past that line, leaving raw mode and the kitty
  keyboard enhancement on — the one path §5 names but never had. The teardown is
  now owned by an RAII `term::RestoreGuard` handed out by `term::init`, so no
  return can skip it. `main_with`'s comment claimed this guard existed; it now
  does.
- **A balanced pop is not a safe teardown.** The kitty keyboard enhancement
  (`CSI > 1 u`, added for Shift+Enter) is a *stack push*, and we popped only
  when our own push had succeeded. That balances our push and nothing else: an
  entry leaked by any other program — a full-screen editor a tool spawned and
  killed, an earlier locode killed with SIGKILL before its teardown ran —
  survives, and the shell inherits CSI-u mode, where **Ctrl+C arrives as the
  literal `ESC [ 9 9 ; 5 u` and Esc as `ESC [ 2 7 u`** instead of interrupting.
  Teardown is now unconditional and healing: pop (`CSI < 1 u` — a no-op on an
  empty stack per the kitty protocol, so it is safe when we never pushed), then
  clear the flags on whatever entry is left (`CSI = 0 ; 1 u`). Order matters:
  clearing first would zero the entry we are about to discard. Claude Code's ink
  reaches the same shape from the same bug (`src/ink/ink.tsx:883-887,1492`,
  `src/ink/termio/csi.ts:301-307`).

**The invariant**, for every terminal mode the TUI adds later (mouse tracking,
focus reporting, synchronized output): *enabling* is capability-gated and
best-effort, *disabling* is unconditional and runs on every exit path. The
asymmetry is deliberate — a mode we fail to turn on costs a feature, a mode we
fail to turn off costs the user their shell.

### Follow-up (2026-07-27): the two remaining exit paths, and a startup that heals

The user keeps Shift+Enter (so the enhancement stays) and asked for the cleanup
to hold on *every* exit. Two gaps were left:

- **SIGHUP** — closing the terminal window or losing an ssh connection hangs up
  the controlling terminal, and its default disposition kills us with no
  teardown at all. Over ssh the damage outlives the process: the pty dies on the
  server, but the *local* emulator keeps the mode it was left in. It now joins
  SIGINT/SIGTERM on the graceful-quit path (as does SIGQUIT, reachable only as
  an explicit `kill -QUIT` since raw mode turns ISIG off — we trade its core
  dump for a usable terminal). Best-effort: if the connection is already gone
  the teardown writes go nowhere.
- **The inherited stack.** SIGKILL/OOM can never run a teardown, so a leak from
  a previous session is a permanent state the *next* session must handle. Setup
  therefore pops before it pushes (`CSI < 1 u` then `CSI > 1 u`): stacking on top
  of an inherited entry is what made the breakage self-perpetuating — each clean
  exit pops one entry, the inherited one keeps the shell in CSI-u mode, and the
  next session inherits it again. Popping first consumes it. This is Claude
  Code's pop-before-push rule (`src/ink/ink.tsx:905-909`), applied at startup
  rather than on re-assert.

Codex reaches the same "exit needs more than a balanced pop" conclusion from the
same failure: `reset_keyboard_reporting_after_exit` pops *and* sends a second
`CSI < u`, because "process exit gets a stronger reset so the parent shell does
not inherit enhanced key reporting if a terminal misses the normal stack pop"
(`codex-rs/tui/src/tui/keyboard_modes.rs:1-5,221-232`). We zero the flags instead
of popping twice — that heals a leak at any stack depth, not just depth two.

Verified end-to-end in a pty against a simulated kitty-capable terminal: startup
emits pop-then-push, exit emits pop-then-clear, and a SIGHUP mid-session still
emits the full teardown.

**Not adopted**: re-asserting the modes after idle gaps or tmux/ssh reconnects
(Claude Code's `reassertTerminalModes`). It keeps Shift+Enter working across a
reconnect, but it is also what generated their unbalanced-stack bug, and our
push happens once. Revisit only with the pop-before-push rule attached.
Codex's `CODEX_TUI_DISABLE_KEYBOARD_ENHANCEMENT`-style opt-out is likewise
deferred until someone wants the feature off.
