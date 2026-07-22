# Task 27 · Slice 8 — Composer frame + bottom status line

**Status:** done (PR pending) · **Date:** 2026-07-22 · **Deps:** none (no new crates)

## Status analysis — the minimal next unit

Second half of the first-user vibe-check asks (screenshots vs Claude Code). The
Markdown bugs shipped in slice 7; this slice does the two chrome asks the user
prioritized alongside it:

1. **Composer frame** — the bare `❯ …` input gains a top + bottom rule (user
   choice via previewed options, 2026-07-22: "top + bottom rules only", not a
   full box). It already auto-grows with content; the frame rows are added on top.
2. **Bottom status line** — replace the keybind-hints footer
   (`enter to send · …`) with `cwd · model · N tok`. The running-spinner status
   stays **above** the composer (user: "the current status above looks okay").

## Decisions / scope (flagged, per user)

- **cwd** is home-shortened (`~/dev/locode-core`) in the engine and sent on
  `EngineMsg::Ready { model, cwd }`.
- **tokens** = cumulative input+output across the session's runs (`session_tokens`,
  compact-formatted `3.1k`/`1.2M`). This is honest *usage*, not context-window
  occupancy — the engine sums `Usage` across turns, so a true "context" number
  needs per-request usage (deferred; named extension point).
- **Deferred** (user said so): git branch, cost/usage-with-cap (needs a cap
  interface since a real API is billed), a wall clock. All are additive to the
  same status line.
- Transient armed hints (ctrl+c again / esc again / cancelling) still override
  the status line — discoverability for the destructive keys is preserved.

## Design

- `ui/composer.rs`: `render` splits its area `[rule, editor, rule]` and draws a
  dim `─`×width top and bottom; `desired_height` adds `FRAME_ROWS = 2`.
- `engine.rs`: `build_session` also returns a home-shortened cwd; `home_relative`
  helper; both `Ready` sends carry it.
- `app.rs`: `App` gains `cwd: Option<String>` + `session_tokens: u64`; `Ready`
  sets cwd; `on_run_finished` accumulates tokens.
- `ui.rs`: `footer_line` → status line via `status_text` (`cwd · model · N tok`,
  omitting unknown parts) + `fmt_tokens`.

## Test matrix (all green)

- `status_line_shows_cwd_model_and_tokens` — exact `~/proj · opus · 3.1k tok`.
- `draw_renders_composer_bottom_anchored_with_status` — typed text visible, cwd
  on the last row, top row still margin.
- `engine_task.rs` — `Ready` now reports a non-empty cwd.
- Full workspace clippy + test green; no new deps, no public core surface change.

## Result

Composer framed with top/bottom rules; bottom status line shows cwd · model ·
tokens with the spinner status still above. Next candidates: dependency-gated
`syntect` code highlighting (needs the user's OK), then tables; and a true
context-window number when per-request usage is exposed.
