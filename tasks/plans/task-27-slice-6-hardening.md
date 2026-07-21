# Task 27 / Slice 6 — hardening + release readiness

Per [`docs/tui-dev-process.md`](../../docs/tui-dev-process.md). Grounding:
SPEC-TUI §Success criteria / §Build order slice 6.

## Phase 0 — status analysis

- **State**: slices 1-5 merged; `locode --yolo` is feature-complete (runs,
  cancel, approvals, queue/history/slash, markdown). Gaps found: run errors
  (`report.error`) are never surfaced; no README mention of the TUI; the
  real-wire smoke and the publish decision are outstanding.
- **Minimal next unit**: surface run errors in the transcript; a resize
  robustness check; a README section introducing the `locode` binary (it
  ships now — ADR-0001 amendment said introduce it when it ships); and flag
  the **publish/release decision as a user hard-stop**.
- **Why now**: last slice; makes a real-wire session legible (errors) and
  documents the product.
- **Prereqs**: everything (feature-complete UI).
- **Unblocks**: the release (user-gated).
- **Risks**: (1) real-wire smoke needs `LOCODE_API_KEY` — I can't run it, only
  document it; (2) publish/version-bump is a hard-stop, not autonomous.

## Phase 1 — harness revisit (fresh reads 2026-07-21)

- **codex** `TurnStatus::Failed` (`chatwidget/protocol.rs:265`, re-read): a
  failed turn adds an error cell before settling. → **Adopt**: on a run whose
  `report.error` is `Some` (ModelError/Error), push an error notice before the
  `TurnEnd` separator so the user sees *why* (rate limit, bad key, tool fatal).
- Resize: all four own the relayout; codex reflows from source, we accept the
  print-once wrap (ADR-0019). → Just verify no corruption on resize.

**Decisions**: error surfacing (now); README TUI section (now); real-wire
smoke = a documented manual checklist (can't run keyless); publish flip +
version bump + crates.io + tag = **user hard-stop** (flagged, not done).

## Phase 2 — design

- `app.rs on_run_finished`: `if let Some(err) = &report.error` push
  `Block::Notice(format!("error: {err}"))` before `turn_end`. Covers
  ModelError (provider/network after retries) and Error (fatal tool).
- README: a short "Interactive app (`locode`)" section — build/run,
  `--yolo`, `--api-schema`, the keyless `mock` demo; note it's `publish=false`
  (pre-release).
- No code for the real-wire smoke; a manual checklist in the Result.

### Edge cases

Cancelled run (error is None — no notice, correct); Completed (no notice);
ModelError with a long error string (Notice wraps? Notice is one line — fine
for v1, the status separator plus the error text suffice).

### Test matrix / preset targets

1. [reducer] `RunFinished` with `error: Some("boom")` → an `error: boom`
   notice precedes the `model_error`/`error` `TurnEnd`; Completed/Cancelled →
   no error notice.
2. [smoke] resize the terminal mid-session → no corruption, live region
   relayouts, transcript intact (manual + a scripted SIGWINCH check).
3. [docs] README builds a coherent Install→run→interactive story.
4. [gates] fmt/clippy/test/doc green (FAILED-explicit check).

## Open questions / hard-stops for the user

- **RELEASE (hard-stop):** flip `locode-tui`/`locode-app` `publish`? Bump to
  0.1.5 and cut a release (the `locode` binary would then ship via the
  installer)? Or keep `publish=false` and iterate? **Needs your call.**
- Real-wire Anthropic smoke is a manual step (`LOCODE_API_KEY` + `locode
  --yolo`); checklist in the Result.

## Result (2026-07-21)

Shipped: run errors surfaced (`report.error` → an `error: …` notice before the
`TurnEnd` separator, so ModelError/Error runs say *why*); a README "Interactive
app (`locode`)" section (build/run, `--yolo`, keys, `publish=false` note);
plan doc.

All autonomous preset targets met: 342 workspace tests (2 new error-surfacing
reducer tests). Full gates + doc green (FAILED-explicit check). Release-binary
smokes: unknown `--api-schema` → "engine unavailable" notice + app stays usable
+ `/quit` exits 0; markdown renders correctly at a narrow 60-col width
(layout robust).

Manual checklist (can't run keyless):
- Real-wire Anthropic: `LOCODE_API_KEY=… locode --yolo`, ask it to run a
  command / read a file; confirm tool calls render and the reply streams.
- Live resize: drag-resize the terminal mid-session; the live region
  relayouts, printed history keeps its wrap (accepted, ADR-0019).
- Panic restore: it's covered by the idempotent-teardown unit + panic hook.

**HARD-STOP for the user (not done):** the publish flip + version bump +
crates.io + tag. The app crates stay `publish = false`. Flagged in the report.

Task 27 (all six slices) is functionally complete: `locode --yolo` is a
usable interactive coding agent.
