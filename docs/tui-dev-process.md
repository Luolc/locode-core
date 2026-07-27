# The TUI development process — autonomous slice loop

How the TUI workstream (Task 27: `locode-tui` + `locode-app`) is developed. **Mode:
near-fully autonomous** (user decision, 2026-07-21).

**The loop, the quality gate, the autonomy contract, the hard-stop list, and the
context-reset protocol live in [`autonomous-workflow.md`](autonomous-workflow.md)** —
they are identical for every workstream and are not restated here. This document
carries only what is specific to the TUI.

## Grounding documents (authority order)

Accepted ADRs → [`SPEC-TUI.md`](../SPEC-TUI.md) →
[`docs/research/tui-harness-study.md`](research/tui-harness-study.md) →
[`tasks/tracker.md`](../tasks/tracker.md) Task 27.

## TUI specifics inside the shared loop

- **Plan doc path**: `tasks/plans/task-27-slice-N-<name>.md`; branch
  `feat/task-27-slice-N-<name>`.
- **Phase 1 sources**: the four harnesses' *UI* layers — the study doc's citations are
  the entry points.
- **Phase 2 test matrix — the shapes available here**: reducer table tests (always
  possible: `Msg → update → Cmd` is sans-IO), `TestBackend` buffer assertions for
  rendered output, engine-task integration over `MockProvider`, and lifecycle unit tests
  over an extracted sequence builder.
- **Where test-first does not reach**: terminal lifecycle. Extract the byte-sequence
  builder and pin that (`term.rs`'s `KEYBOARD_ENHANCEMENT_ON`/`_OFF` are the worked
  example); everything left over becomes a manual-smoke entry. Note that this exemption
  is where the 2026-07-27 kitty-teardown defect came from — treat "untestable" as a
  claim to be minimized, not a category to park work in.

## Autonomy — TUI-local additions

The shared contract applies. One workstream-specific carve-out: the spec-flagged
ADR-0017 amendment (engine `decide()` await observing the cancel token) may be
**proposed** as its own small PR with the ADR note — flagged loudly for review rather
than treated as an ordinary core-surface hard stop.

Scope hard stop for this workstream: expanding past SPEC-TUI's non-goals (mouse, themes,
multi-session, Windows), even if trivially reachable.

## Standing constraints — TUI-local additions

- Core crates stay headless (ADR-0001 amendment); anything the TUI needs from the core
  goes through the four seams or becomes a flagged core proposal.
- `locode-app` stays flag-free composition; substance lives in `locode-tui` behind
  `main_with` (SPEC-TUI crate shape).
- One TUI crate; splits only on the spec's named triggers.
