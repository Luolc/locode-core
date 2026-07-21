# The TUI development process — autonomous slice loop

How the TUI workstream (Task 27: `locode-tui` + `locode-app`) is developed.
**Mode: near-fully autonomous** (user decision, 2026-07-21) — the agent drives
every phase end-to-end with minimal human intervention; the human reviews
merged PRs asynchronously and answers batched questions. This document is the
binding contract for that autonomy: it defines the loop, the quality gates,
what the agent decides alone, and the short list of things that still stop for
the user.

Grounding documents (the loop must stay anchored to these, in this order of
authority): accepted ADRs → [`SPEC-TUI.md`](../SPEC-TUI.md) →
[`docs/research/tui-harness-study.md`](research/tui-harness-study.md) →
[`tasks/todo.md`](../tasks/todo.md) Task 27. The repo-wide rules in
[`AGENTS.md`](../AGENTS.md) (ADR-first, faithful-vs-custom boundary, quality
triangle, git workflow) apply unchanged; this document adds the TUI-specific
loop on top.

---

## The loop

Every unit of work runs the same five phases. A "unit" is normally one of the
six SPEC-TUI slices, but a slice may be subdivided when a single PR would
exceed reviewability (see Ship phase) — subdivision is the agent's call and is
recorded in the plan doc.

### Phase 0 — Status analysis (recorded, not skipped)

At the start of each unit — and whenever resuming after an interruption —
re-derive the current state rather than trusting memory:

1. Re-read `SPEC-TUI.md`, the Task 27 checklist, and the previous unit's plan
   doc **Result** addendum (see Phase 4).
2. Inspect the actual code state (`git log`, crate tree, test list) — the
   merged code is the tie-breaker if any doc drifted.
3. Answer, in writing (top of the new plan doc):
   - **What is the minimal next unit?** The smallest slice of end-to-end,
     verifiable behavior.
   - **Why this, why now?** What it unblocks downstream; why nothing smaller
     suffices.
   - **Prerequisites** — what it assumes already works (and a check that it
     actually does).
   - **Considerations/risks** — the 2–4 things most likely to go wrong.

### Phase 1 — Harness revisit (mandatory, per unit)

Before designing, go back to the four harness sources **for this unit's
specific area** — not the study doc from memory, the actual code (AGENTS.md:
"planning is a research task, not a from-memory task"). The study doc's
citations are the entry points; follow them into the submodules and read
around them.

Record in the plan doc, as a four-row table or per-harness bullets:

- **What each harness does** for this area (with fresh `file:line` citations —
  new ones, not just copies from the study).
- **Lessons applicable** to this unit (including anti-patterns to avoid).
- **Our decision**, with rationale tied to the evidence.
- The three-way split: **implement now** / **deferred** (with the named
  extension path) / **rejected** (with the reason).
- **Needs user input (future)**: anything discovered that genuinely needs the
  user — flagged in the plan's "Open questions for the user" section and in
  the PR body, but **never blocking**: pick the reversible default, record
  it, and proceed. Questions accumulate for batched review.

### Phase 2 — Plan doc (written before code, committed with the work)

`tasks/plans/task-27-slice-N-<name>.md`, containing:

1. The Phase 0 status analysis.
2. The Phase 1 harness-revisit record.
3. **Design**: module touch points, data-flow deltas (which `Msg`/`Cmd`
   variants, which `App` fields), public-surface changes (should normally be
   none outside `locode-tui`).
4. **Edge cases** enumerated explicitly.
5. **Test matrix**: every acceptance target mapped to a concrete test
   (reducer table test / TestBackend buffer assertion / engine-task
   integration with `MockProvider` / lifecycle unit test), plus the
   manual-smoke items that cannot be automated (listed for the user's
   optional spot-check, never a merge gate).
6. **Preset targets**: the checklist the unit must fully satisfy before it
   ships. Targets are binary and testable; "feels done" is not a target.
7. Deferred / rejected / open-questions sections from Phase 1.

### Phase 3 — Implement + test until targets are met

- Branch first (`feat/task-27-slice-N-<name>`), per the repo git rules.
- Test-first where the shape allows (the reducer and block renderers always
  allow it; terminal lifecycle mostly doesn't — those get their manual-smoke
  entries plus whatever unit surface exists, e.g. the teardown-sequence
  builder).
- Iterate until **every preset target passes**. A target that turns out to be
  wrong is *changed in the plan doc with a dated note*, not silently dropped.
- Quality gates before PR, all mandatory:
  1. `cargo fmt --all -- --check`
  2. `cargo clippy --workspace --all-targets -- -D warnings`
  3. `cargo test --workspace`
  4. `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`
  5. A self-review pass over the full diff: correctness re-read, dead code,
     naming, comment discipline (constraints only), simplification
     opportunities — fix before shipping, don't note-and-ship.
- Bounded-resource audit per unit (the study's rule 9): any new channel,
  queue, or buffer added this unit must have an explicit bound and a comment
  saying what happens at the bound.

### Phase 4 — Ship

- PR with a real body: what/why, test evidence, **deviations from the plan
  doc** (explicitly listed, even small ones), open questions for the user.
- `gh pr merge --auto --squash --delete-branch`; merge on green; prune local
  branches after.
- Same-PR bookkeeping (never deferred to "later"):
  - Task 27 checkbox / sub-checkbox updates.
  - **Result addendum** appended to the plan doc: what shipped, deviations,
    measured facts worth keeping (sizes, timings), and the pointer to what's
    next — this is Phase 0's input for the next unit.
  - ADR/SPEC reconciliation if any decision drifted (ADR-first extends to
    SPEC-TUI: reconcile the spec *before or with* the code change, in the
    same PR).
- If CI fails on something the local gates passed, fix forward on the same
  branch; never bypass the check.

### Phase 5 — Continue

Loop to Phase 0 for the next unit without waiting, unless a hard-stop item
(below) is pending. If a merged unit is later found defective, the fix is a
new small unit through the same loop (analysis → plan → fix → Result note),
not an ad-hoc patch.

---

## Autonomy contract

**The agent decides alone (recording the judgment in plan/PR):** everything
inside the approved spec — module design, naming, test design, slice
subdivision, reversible in-scope trade-offs, choosing defaults for flagged
open questions (default chosen = the most reversible one), small SPEC-TUI
amendments that *narrow* or *clarify* scope.

**Hard stops — still require the user (batched where possible):**

1. **New dependencies** — relaxed (user, 2026-07-21): *reasonable,
   well-justified* deps may be added without asking, recorded in the plan doc
   and PR body with the justification; anything heavy, niche, or
   security-sensitive still stops for the user.
2. **Changes to core crates' public surface** (traits, envelope,
   `schema_version`, facade exports) — with one pre-authorized exception: the
   spec-flagged ADR-0017 amendment (engine `decide()` await observing the
   cancel token) may be *proposed* as its own small PR with the ADR note, but
   flagged loudly for review.
3. **Crate boundary changes** beyond the two approved crates (i.e., firing a
   split trigger from SPEC-TUI needs a go-ahead).
4. **Publishing / releases / version bumps** (crates.io, tags, `publish`
   flips).
5. **Expanding scope past SPEC-TUI's non-goals** (streaming, mouse, themes,
   multi-session, Windows, …) — even if trivially reachable.
6. Anything destructive or outward-facing beyond the normal branch→PR→merge
   flow.

**Communication protocol:** at each unit's completion, the report to the user
is: outcome first (what shipped, proof), deviations, the accumulated
open-questions list, and what the next unit is. Questions never block the next
unit unless they're hard-stop items on its critical path.

---

## Standing constraints (inherited, restated for the TUI context)

- Core crates stay headless (ADR-0001 amendment); anything the TUI needs from
  the core goes through the four seams or becomes a flagged core proposal.
- `locode-app` stays flag-free composition; substance lives in `locode-tui`
  behind `main_with` (SPEC-TUI crate shape).
- One TUI crate; splits only on the spec's named triggers.
- All writing in the repo in English; user-facing chat follows the user's
  language.
- The study doc is living: if a harness revisit (Phase 1) finds something the
  study missed or got wrong, amend the study doc in the same PR with a dated
  note.
