# The autonomous workflow

**How a workstream in this repo is developed when it runs autonomously** — the agent
drives every phase end-to-end, the user reviews merged PRs asynchronously and answers
batched questions. This is the binding contract for that autonomy: the loop, the gates,
what the agent decides alone, and the short list of things that still stop for the user.

This document is **workstream-neutral**. Each workstream adds a thin companion doc
(`docs/<name>-dev-process.md`) carrying only what is specific to it — its grounding
documents, its resolved decisions, its slice plan, its own hard stops. When the user
says *"run the autonomous workflow"*, this is the loop they mean.

Written 2026-07-27 by extracting the identical parts of `tui-dev-process.md`,
`claude-pack-dev-process.md`, and `codex-pack-dev-process.md`, which had each restated
them (see [`META-AGENTS.md`](../META-AGENTS.md) §4). The repo-wide rules in
[`AGENTS.md`](../AGENTS.md) apply unchanged; this adds the loop on top.

---

## Authority order

When two sources disagree, the earlier one wins:

1. **Accepted ADRs** (`docs/decisions/`) — the load-bearing decisions.
2. **The spec** — `SPEC.md`, plus any workstream spec the companion doc names.
3. **The workstream's own doc** — its resolved decisions, which are *not* re-litigated
   mid-flight (reopening one is a hard stop).
4. **The relevant `docs/research/` study** — the source-grounded findings.
5. **`tasks/tracker.md`** — status, and the task's checkboxes.

**The merged code is the tie-breaker when a doc has drifted** — and then the drift gets
reconciled in the same PR (ADR-first), never left standing.

## The loop

Every unit of work runs the same five phases. A "unit" is normally one slice from the
workstream's plan; subdivide when a single PR would exceed reviewability — that is the
agent's call, recorded in the plan doc.

### Phase 0 — Status analysis (written, never skipped)

At the start of each unit, and whenever resuming after an interruption, re-derive the
state instead of trusting memory:

1. Re-read the workstream doc's resolved decisions, the task's checklist, and the
   **previous unit's plan Result addendum** (Phase 4).
2. Inspect the actual code state (`git log`, crate tree, test list).
3. Answer in writing, at the top of the new plan doc:
   - **What is the minimal next unit?** The smallest slice of end-to-end, verifiable
     behavior.
   - **Why this, why now?** What it unblocks; why nothing smaller suffices.
   - **Prerequisites** — what it assumes already works, plus a check that it does.
   - **Risks** — the 2–4 things most likely to go wrong.

### Phase 1 — Source revisit (mandatory, per unit)

Before designing, go back to the harness sources **for this unit's specific area** — the
actual code in `coding-cli-survey/submodules`, not the study doc from memory
(`AGENTS.md`: "planning is a research task, not a from-memory task"). The study's
citations are entry points; follow them into the source and read around them.

Record in the plan doc:

- **What each harness does** here, with **fresh `file:line` citations** — new ones, not
  copies from the study.
- **Lessons applicable** to this unit, including anti-patterns to avoid.
- **Our decision**, tied to that evidence.
- The three-way split: **implement now** / **deferred** (with the named extension path) /
  **rejected** (with the reason).
- **Needs user input**: anything genuinely requiring the user goes in the plan's "Open
  questions" section *and* the PR body — but **never blocks**. Take the most reversible
  default, record it, proceed. Questions accumulate for batched review.

If the revisit finds the study doc wrong or incomplete, **amend the study in the same
PR** with a dated note, and update its source-freshness line.

### Phase 2 — Plan doc (written before code, committed with the work)

`tasks/plans/task-NN-slice-N-<name>.md`, containing:

1. The Phase 0 status analysis.
2. The Phase 1 source-revisit record.
3. **Design** — module touch points, data-flow deltas, public-surface changes (normally
   none outside the workstream's own crates).
4. **Edge cases**, enumerated explicitly.
5. **Test matrix** — every acceptance target mapped to a concrete test, plus the
   manual-smoke items that cannot be automated (listed for the user's optional
   spot-check, never a merge gate).
6. **Preset targets** — the checklist the unit must fully satisfy before it ships.
   Targets are binary and testable; "feels done" is not a target.
7. Deferred / rejected / open questions from Phase 1.

Plan docs are **immutable records**: no live checkboxes (those live in the tracker), and
a target that turns out to be wrong is changed *with a dated note*, not silently dropped.

### Phase 3 — Implement + test until every target passes

- **Branch first** (`feat/task-NN-slice-N-<name>`) — before the first edit, not at commit
  time.
- **Test-first where the shape allows.** Where it does not (terminal lifecycle, exit
  paths, signals), extract the testable core — a sequence builder, a pure function — and
  pin *that*, then record the rest as a manual-smoke item. "Untestable" is a claim that
  needs the same scrutiny as any other.
- **An invariant sentence needs a test name.** If the plan, an ADR, or a doc comment says
  *always / never / every path / exactly one*, this unit either pins it with a test or
  downgrades the wording to what it really is (`META-AGENTS.md` §5.1).
- **Bounded-resource audit**: any new channel, queue, or buffer gets an explicit bound and
  a comment saying what happens at the bound.
- **The gate before every PR, all four mandatory** (the branch-protection check):

  ```sh
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace          # see the warning below
  RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
  ```

  For the test step, **confirm no `FAILED`/`panicked` lines directly**
  (`… | grep -E 'FAILED|panicked'` must be empty). Never trust a summed pass-count — it
  can hide an entire failed test binary. The `doc` step is the one people skip and it
  catches broken intra-doc links; skipping it has red-CI'd a PR and stalled a task.
- **Self-review pass over the full diff** before shipping: correctness re-read, dead code,
  naming, comment discipline (constraints and *why*, not narration), simplification
  opportunities. Fix them now — do not note-and-ship.

### Phase 4 — Ship

- **PR with a real body**: what and why, test evidence, **deviations from the plan doc
  listed explicitly** (even small ones), and the batched open questions.
- `gh pr merge --auto --squash --delete-branch`; merge on green; prune local branches
  after (`git fetch -p && git branch -vv | awk '/: gone]/{print $1}' | xargs -r git branch -D`).
- **A watcher must exit on CI failure**, not poll only for "merged" — a merged-only wait
  loops forever on red and looks stuck.
- **Same-PR bookkeeping, never deferred to "later":**
  - the task's checkbox in `tasks/tracker.md`;
  - a **Result addendum** on the plan doc — what shipped, deviations, measured facts worth
    keeping, and the pointer to what is next (this is Phase 0's input for the next unit);
  - **ADR/SPEC reconciliation** if any decision drifted (ADR-first).
- If CI reddens on something the local gates passed, **fix forward on the same branch**;
  never bypass the check.

### Phase 5 — Continue

Loop to Phase 0 for the next unit without waiting, unless a hard stop is pending. A defect
found in a merged unit becomes **a new small unit through the same loop** (analysis → plan
→ fix → Result note), not an ad-hoc patch.

---

## Autonomy contract

**The agent decides alone** (recording the judgment in the plan and PR): everything inside
the workstream's own crates — module design, naming, test design, slice subdivision,
reversible in-scope trade-offs — plus choosing the reversible default for any flagged open
question, and spec amendments that *narrow or clarify* scope.

**Hard stops — these still require the user**, batched where possible:

1. **Core public surface** — `locode-protocol` types, the `Tool`/`Provider` trait
   signatures, the report envelope / `schema_version`, `locode-core` facade exports.
2. **Crate boundary changes** — a new crate, a moved crate, a split.
3. **Publishing, releases, version bumps, tags** — always the user's call.
4. **Heavy, niche, or security-sensitive dependencies.** Reasonable, well-justified deps
   may be added *with the justification in the plan and PR*; anything heavy stops.
5. **Reopening a resolved decision** in the workstream doc, or expanding scope past its
   stated non-goals — even when trivially reachable.
6. **Anything destructive or outward-facing** beyond the normal branch → PR → merge flow.

Questions never block the next unit unless they are a hard stop on its critical path.

**Reporting**, at each unit's completion: outcome first (what shipped, with proof), then
deviations, then the accumulated open questions, then what the next unit is.

## Standing constraints

- **Core crates stay headless** (ADR-0001). A pack or UI reaches the OS only through
  `locode-host` — never `std::fs`/`Command` from a tool body.
- **Every `tool_use` gets exactly one `tool_result`**; tool failures are soft
  `tool_result{is_error}`, not fatals, unless the ported harness itself hard-fails.
- **Faithful mimicry wins over a repo default for a ported pack** — subject to truth
  (`AGENTS.md`); note each such call explicitly.
- **Stdout is sacred**: exactly one JSON document from the binary; nothing printed from
  library crates.
- **All writing in the repo is English**; the chat reply follows the user's language.
- **The study and plan docs are living**: a Phase 1 revisit that finds one wrong amends it
  in the same PR with a dated note.

## After a context reset

Read, in order: this document → the workstream's companion doc top-to-bottom → the task's
plan and the previous unit's Result addendum → then open the source for the next unit's
area and run Phase 0. Do not resume from a summary of what you were doing.

---

## What a workstream companion doc must add

Keep it to what is genuinely local — everything above is inherited, not restated:

1. **Grounding documents** — this workstream's spec, study, and task entry, in authority
   order.
2. **The source pin** — which submodule and **which commit** the port is faithful to.
3. **Resolved decisions** — the interview outcomes, numbered (`D1`, `D2`, …) so a PR can
   cite one and a hard stop can name the one being reopened.
4. **Gap log** — accepted, documented fidelity gaps, kept current with the module docs.
5. **Slice plan** — the proposed order, explicitly the agent's call to revise at Phase 0.
6. **Local hard stops and constraints** — only those *beyond* the six above.
