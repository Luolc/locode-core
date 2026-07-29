# META-AGENTS.md

**The document about the documents.** [`AGENTS.md`](AGENTS.md) tells an agent how to
work in this repo; this file records **how we decide what goes into `AGENTS.md`, the
skills, the ADRs, and the process docs — and what we have learned about our own
workflow that should change them.**

Both the user and the agent read this one. Read it before you edit `AGENTS.md`, a
skill under `.claude/skills/`, `SPEC.md`, or a `docs/*-dev-process.md`. When the user
says *"we keep hitting X — update our rules"*, this file is where the finding lands
first and where the routing decision is made.

It is **not** a status file (that is [`tasks/tracker.md`](tasks/tracker.md)) and not a
place for design decisions about the product (those are ADRs). It holds decisions about
**our process and our instruction-writing**, plus the evidence behind them.

---

## 1. The document map

| File | Answers | Lifetime | Never put here |
|---|---|---|---|
| `AGENTS.md` | *How do I work here?* Rules an agent must follow. | Living; edited when a rule changes | Status, design rationale, per-task detail |
| `META-AGENTS.md` (this) | *How do we write those rules?* Process findings + doc conventions | Living; append-only findings log | Product decisions, task status |
| `SPEC.md` | *What are we building, and what counts as done?* | Slow-moving; reconciled when scope shifts | The *why* behind a specific decision |
| `docs/decisions/ADR-*.md` | *Why is it this way, and what did we reject?* | Living via dated amendments | Status, step-by-step task plans |
| `tasks/tracker.md` | *What is done, next, deferred?* The **only** checkbox home | Living | Design detail, rationale |
| `tasks/plans/task-*.md` | *How was this task designed, and what actually shipped?* | **Immutable** + a "Result" addendum | Live checkboxes |
| `docs/research/*.md` | *What do the four studied harnesses actually do?* | Living; re-read per task | Our decisions (those graduate to an ADR) |
| `docs/*-dev-process.md` | *What is the loop for this workstream?* | Per workstream | Anything true of the whole repo (→ `AGENTS.md`) |
| `.claude/skills/*/SKILL.md` | *How do I do a generic engineering activity?* | Rarely; vendored, generic | Repo-specific rules (→ `AGENTS.md`) |
| `tasks/audits/*.md` | *How faithful is our port of one tool?* | Per audit | — |

**The routing question.** Before writing a paragraph anywhere, ask: *what question does
a reader have when they open this file?* If the paragraph does not answer that file's
question, it belongs in a different file. Most drift starts as a well-meant paragraph in
the wrong place.

## 2. Routing a new learning

When something is learned — a bug taught us an invariant, a habit turned out to be
wrong, a rule proved unenforceable — route it:

| The learning is… | Goes to | Form |
|---|---|---|
| A product design decision, or a rejected alternative | An ADR (new, or a dated amendment) | ADR-first: reconcile the ADR **before** the code change |
| An invariant the code must hold | The ADR **and** a test that pins it (§5) | The test's name goes next to the claim |
| A rule every agent must follow, in any harness | `AGENTS.md` | Imperative, with the *why* in one clause |
| A rule for one workstream only | That workstream's `docs/*-dev-process.md` | — |
| What happened in one task | That task's plan "Result" addendum | Dated |
| Something to build later | `tasks/tracker.md` (Deferred / backlog) or §6 here if it is *tooling* | One line, with the trigger to revisit |
| A fact about how **we** work, or about these documents | **This file**, §4 | Dated finding + evidence |

Two failure modes this table exists to prevent: (a) a learning that lives only in a
commit message or a PR body — nobody reads those again; (b) the same learning written
into three files, which then drift.

## 3. What actually goes in `SPEC.md` vs an ADR — prescribed vs. evolved

The user asked the right question: *are these formats something the skills demanded, or
did we evolve them?* Measured answer: **the skeletons are inherited literally; everything
load-bearing is ours.**

### 3.1 What the skills literally require

`.claude/skills/spec-driven-development/SKILL.md` is prescriptive, not hand-wavy. It
demands **six core areas** and ships a copy-paste template: Objective, Tech Stack,
Commands (*"full executable commands with flags, not just tool names"*), Project
Structure, Code Style (*"one real code snippet beats three paragraphs"*), Testing
Strategy, Boundaries (a three-tier **Always / Ask first / Never**), Success Criteria,
Open Questions. It also states a gated pipeline (SPECIFY → PLAN → TASKS → IMPLEMENT, each
human-reviewed), and a "keep the spec alive" rule: update the spec *first*, then implement.

`.claude/skills/documentation-and-adrs/SKILL.md` is equally concrete for ADRs: numbered
files under `docs/decisions/`, and six sections — **Status, Date, Context, Decision,
Alternatives Considered, Consequences** — where alternatives are written pros / cons /
why-rejected. Its lifecycle rule is `PROPOSED → ACCEPTED → (SUPERSEDED | DEPRECATED)`,
with *"don't delete old ADRs; when a decision changes, write a new ADR that supersedes
the old one."*

### 3.2 What we inherited verbatim

- `SPEC.md` carries the template's nine sections **in the template's order**.
- `ADR-0001` is the six-section template exactly.

So the format was not invented here — the first ADR is the skill's example with our
content in it.

### 3.3 What we evolved, and why

`SPEC.md` grew two sections the skill does not have:

- **`## Assumptions`** — the skill treats assumption-surfacing as a *conversational* step
  ("correct me now or I'll proceed"). We made it a written section, because in a
  re-implementation project the assumptions (which harnesses, which wire, what "faithful"
  means) outlive any single conversation.
- **`## Decisions of record`** — an index into the ADRs. The skill has no link between
  spec and ADRs; without it, 28 ADRs are unnavigable from the spec.

ADRs grew four (measured across 28 ADRs, 2026-07-27):

- **`## Amendment (date): …` — 47 of them, in 17 of 28 ADRs.** By far the biggest
  divergence, and a deliberate conflict with the skill: see §3.4.
- **`## Scope`** (4) — what the ADR does *not* cover, so a reader stops looking.
- **`## Resolved (user decisions + source, date)`** (4) — open questions that got answered,
  kept next to the question instead of silently deleted. This is what makes an ADR
  re-readable a month later.
- **`## Open Questions`** (5) — the skill has no such section for ADRs (only for specs).
  Ours carry the questions the design could not settle, which is how a draft ADR
  (e.g. ADR-0027, *not approved*) can still be committed and useful.

Plus one habit no template asked for and which is arguably this repo's best documentation
practice: **every non-obvious claim carries a source citation** (`file:line` into the
vendored harness sources). That comes from `AGENTS.md`'s "planning is a research task",
not from the skills.

### 3.4 The one deliberate conflict: amend vs. supersede

The skill says *change a decision → write a superseding ADR*. `AGENTS.md` says: **minor
delta → dated amendment; large delta → supersede.** The measured result is lopsided —
**47 amendments vs. 2 supersessions.** In a fast-moving solo repo the amendment rule is
right: a new ADR per small delta would produce a chain nobody can read.

But it has a cost, and we have already paid it once: **amendments make an ADR a *living*
document, and a living document accumulates claims that nothing re-checks.** ADR-0019 §5
claimed the teardown was "shared by exit / error / panic-hook / signal paths" — the error
path never reached it, and `main_with`'s comment cheerfully described a guard that did not
exist. That sat undetected for months and surfaced as a broken shell on the user's server
(#248, #249). Hence the rule in §5.1.

## 4. Findings — the learning log

Append-only. Each entry: what we observed, the evidence, and what changed (or an explicit
"recorded only").

### F1 (2026-07-27) — The process gates on "tests pass"; the standard is "the user looks at it"

**Evidence.** 254 commits: 100 `feat`, 87 `docs`, **37 `fix`**. Reading all 37: the large
majority are defects only a human in a real terminal could see — markdown whitespace and
lists, table borders rendered dim, the streaming cell fixed at 12 rows, CJK width-wrapping
(twice), recovered prompts not rendering on resume, hidden-context blocks unwrapped,
notices unwrapped, the token counter showing the wrong quantity (twice), the mid-run echo
at the wrong position (three times), the terminal left in kitty mode (twice).

**Why it happens.** ~600 tests cover logic; they structurally cannot catch "this number
means the wrong thing" or "this line renders in the wrong place". The only detector for
those is the user, and because every PR auto-merges on green CI, that detection happens
**after** merge — so each finding becomes another PR. The fix stream is a *measurement of
the gap*, not of carelessness.

**Corroborating detail:** the autonomous loop's implement phase already admits it — terminal
lifecycle "mostly doesn't [allow test-first]; those get their manual-smoke entries". The
manual smoke is the user.

**Correction (2026-07-29).** This finding first claimed the TUI had "no `TestBackend`
anywhere in the workspace". That was wrong — there are 36 uses across five files
(`ui.rs`, `ui/composer.rs`, `ui/dropdown.rs`, `frame_terminal.rs`, `event_loop.rs`). The
claim came from a grep truncated by `head`, which is a reminder that a number in this file
is only as good as the command behind it: **re-run it, do not trust the first screenful.**

The corrected finding is sharper, not weaker. Render tests *do* exist, and the defects
shipped anyway, because most of them were not rendering bugs. "The token counter shows
accumulated usage instead of context occupancy", "the echo renders where the user typed it
rather than where it entered the conversation", "the streaming cell is a fixed 12 rows" —
a widget snapshot pins *how a given input draws*; every one of those was a mistake about
**which input to pass**. That is the class of defect no amount of widget-level testing
reaches, and the reason the verification gap is about running the real thing, not about
adding more render assertions.

**Changed:** nothing yet — deliberately (user, 2026-07-27). Recorded in §6.

### F2 (2026-07-27) — An invariant stated in an ADR but not pinned by a test will rot

**Evidence.** ADR-0019 §5 vs. the missing `RestoreGuard` (see §3.4). The ADR was accurate
when written and became false through ordinary edits, with no mechanism to notice.

**Changed:** rule §5.1 below.

### F3 (2026-07-27) — Vendored generic commands drift into actively wrong advice

**Evidence.** `.claude/commands/plan.md` instructed the agent to write `tasks/plan.md` +
`tasks/todo.md`; `AGENTS.md` forbids exactly those files (three status copies drifted once
already, 2026-07-22). `/build` refused to work without `tasks/plan.md`. `/ship` knew
nothing of this repo's four-step release. A command that contradicts the repo is worse
than no command: it is a plausible-looking instruction pointing the wrong way.

**Changed:** `.claude/commands/` deleted in full (user decision, 2026-07-27). The skills
under `.claude/skills/` stay — they are generic *technique* references, not repo workflow.

### F4 (2026-07-27) — Our upstream is stricter than the skills; our downstream is weaker

The skills' pipeline is SPECIFY → PLAN → TASKS → IMPLEMENT. Our real loop is
**research → ADR → plan → implement → the user looks at it → fix**. Two of those steps
have no equivalent in any skill:

- **Research** (re-read the four vendored harnesses, cite `file:line`) — the nearest skill,
  `source-driven-development`, is about official docs, not competitor implementations.
  This step is the project's core method and `AGENTS.md` already enforces it.
- **The user looks at it** — no skill covers it, and no gate in our process covers it
  either. See F1.

The skills' `/review` (five axes, pre-merge) is the slot where the second one *should*
live, and it is unused.

## 5. Rules that came out of the findings

### 5.1 An invariant sentence needs a test name

If an ADR (or `SPEC.md`, or a doc comment) says **always / never / every path / exactly
one**, the same PR either adds a test that fails when the claim is violated, or the
sentence is downgraded to what it really is ("intended", "today", "not enforced").

Naming it makes it checkable: `term.rs`'s teardown claims now sit next to
`teardown_pops_then_clears_the_keyboard_flags` and `dropping_the_guard_restores`. An
invariant you cannot name a test for is a wish, and should read like one.

### 5.2 Reconcile the doc in the same PR as the code — never "later"

Already in `AGENTS.md` (ADR-first). Restated here because F2 is what happens when the
reconciliation is silently skipped: nobody discovers it from the ADR side, only from a
broken terminal.

### 5.3 A rule with no enforcement mechanism is a preference

Before adding a rule to `AGENTS.md`, say how a violation gets noticed: a test, a CI gate,
a checklist step in a process doc, or an explicit "the user will catch this". If the
answer is "an agent will remember", expect it to decay — write the mechanism instead.

### 5.4 Delete instructions that contradict the repo

Stale guidance is more expensive than missing guidance, because it is followed (F3). This
applies to vendored skills too: if a skill's procedure conflicts with `AGENTS.md`, either
note the override in `AGENTS.md` (as it does today for `/plan`'s artifacts) or remove the
skill.

## 6. Backlog — tooling ideas, recorded not scheduled

Deliberately not started (user, 2026-07-27). Each has its trigger.

1. **TUI render snapshots where they are missing.** `TestBackend` is already used for the
   composer, dropdown, and frame mechanics. The gap is `ui/blocks.rs` — the transcript
   renderer, which is tested at the `Vec<Line>` level and is where the wrapping, border, and
   spacing fixes clustered. Extend the existing pattern there rather than inventing one.
   Note F1's correction: this catches *rendering* mistakes, not wrong-input mistakes, so
   estimate its value modestly. *Trigger:* the next rendering bug that reaches a release.
2. **A replayable smoke harness.** The kitty investigation used a real pty via
   `script`, a faked terminal reply to the capability query, `kill -HUP` for the signal path,
   and a byte-level assertion on the exit stream. That is a repeatable recipe: launch the
   binary, feed scripted keys, capture frames and the exit stream, diff. It converts "the
   user opens locode and looks" into "CI runs it; the user reads a diff". It is also what
   would retire the two claims the 2026-07-27 invariant sweep had to leave unpinned — the
   signal path through the teardown, and `init`'s own failure paths. *Trigger:* the
   background-task workstream (P0.5), which will need to observe long-running behavior anyway.
3. **A repo-specific review checklist.** The generic five-axis review does not know this
   repo's invariants: ADR-vs-code drift, `tool_use` ↔ `tool_result` pairing, no stdout in
   library crates, user-facing text truthfulness, bounded channels. *Trigger:* if a class of
   defect repeats that a checklist would have caught.

## 7. Editing the instruction layer itself

**`AGENTS.md`** — harness-neutral (Claude Code, Codex, Grok Build all read it), imperative,
and every rule carries its *why* in a clause, because a rule without a reason gets
rationalized away. It is not a place for status, and not a place for anything true of only
one workstream. Keep it scannable: an agent reads it on every session start, so length is a
real cost. When a rule is added, apply §5.3.

**Skills** (`.claude/skills/`) — vendored, generic, and about *technique*. Write a new one
only if the procedure is (a) reusable outside this repo and (b) too long to sit in
`AGENTS.md`. Anything repo-specific belongs in `AGENTS.md` or a process doc. When a skill
conflicts with this repo, §5.4.

**This file** — append findings to §4 with a date and the evidence that produced them; keep
the evidence, not just the conclusion, so a later reader can re-judge it. Numbers beat
adjectives: "37 of 254 commits are fixes, mostly visual" is re-checkable; "quality is
uneven" is not. When a finding turns into a rule, put the rule in §5 and leave the finding
where it is.
