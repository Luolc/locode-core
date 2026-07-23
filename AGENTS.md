# AGENTS.md

Working rules for AI coding agents in this repo. This file is **harness-neutral**
and is the single source of truth, shared across the agents used here — **Claude
Code, Codex, and Grok Build** (and any other harness). Codex and Grok Build read
`AGENTS.md` natively; Claude Code imports it via `CLAUDE.md`. Edit this file, not
`CLAUDE.md`.

## What this project is

`locode-core` is the **headless Rust core library** of a custom coding agent
("locode"): the sample→dispatch→append loop, a typed tool registry, a
provider/wire abstraction, and a single structured-output contract. The TUI
will be built **in this repo as separate crate(s)** on top of the core (ADR-0001
amendment 2026-07-21) — but the **core crates stay headless**: no TUI
dependencies and no interactive prompts inside them; interaction reaches the
engine only through its public seams (approval, cancel, events, continuity).

Read these before starting implementation work — they are the source of truth for
what we are building and in what order:

- [`SPEC.md`](SPEC.md) — objective, crate layout, tool contract, testing, boundaries, success criteria.
- [`docs/decisions/`](docs/decisions/) — ADRs: the load-bearing decisions and the alternatives rejected. **The ADRs are the source of truth and must stay trustworthy — reconcile them *before* the code (see "ADR-first" in the Working agreement).**
- [`tasks/tracker.md`](tasks/tracker.md) — the **single source of truth for task status**: an "Active / Next" section, a shipped "Archive", and reserved "Deferred" seams. Design detail per task is an immutable record under [`tasks/plans/`](tasks/plans/) (indexed by [`tasks/plans/README.md`](tasks/plans/README.md)); rationale is in the ADRs.

Design rationale and the source study behind every decision live in the separate
`coding-cli-survey` repo (referenced from the ADRs).

## Communicating with the user

- **Voice input.** The user often interacts via speech-to-text, so messages may
  contain transcription errors: wrong homophones, dropped or merged words,
  mis-split phrases, and mistranscribed proper nouns. Read for intent rather than
  literal text (e.g. "cloud" can mean "Claude", "grok build" may be "Grok Build",
  "code X" may be "Codex"). When a word looks out of place, infer the intended
  meaning from context.

- **Identifiers are especially fragile.** Speech-to-text often cannot reproduce
  the exact spelling of file paths, crate names, and function names — particularly
  separators such as dots, hyphens, slashes, underscores, and camelCase
  boundaries. Reconstruct the most plausible intended identifier and cross-check
  it against names that actually exist in the codebase.

- **Flag and confirm your guesses.** Whenever you infer an identifier or resolve
  an ambiguous term, call it out **explicitly** in your summary — list only the
  specific guesses you made — and ask the user to confirm.

- **Latest instruction wins.** The user refines ideas as they go. Follow the most
  recent instruction over earlier ones; if a request seems to contradict something
  said before, the latest wording takes precedence — the user will clarify if
  there is a genuine conflict.

- **Reply in the user's language.** Look at the language of the user's **most
  recent message** and write your chat reply in **that same language** (Chinese →
  Chinese, English → English). Check this on **every** turn — the user switches
  languages mid-conversation. The language of the repository does **not** determine
  the language of your reply.

## Language of the codebase

- The two rules are independent: **what you write into the repo** is always
  English; **how you talk to the user** follows their language (see above). A
  Chinese message still gets a Chinese reply even though any code or docs produced
  in that turn are written in English.
- All code, comments, documentation, commit messages, and README/spec content must
  be written in **English**, regardless of the language the user is speaking.

## Naming conventions

- Follow standard Rust casing: `UpperCamelCase` for types/traits/enums,
  `snake_case` for functions/modules/variables, `SCREAMING_SNAKE_CASE` for consts,
  `kebab-case` for crate names (`locode-provider`) with `snake_case` lib paths.
- **Treat acronyms and initialisms as ordinary words** — capitalize only the first
  letter (aligns with the Rust API guidelines). Write `JsonParser`, not
  `JSONParser`; `HttpClient`, not `HTTPClient`; `ToolId`, not `ToolID`. This keeps
  word boundaries mechanical and unambiguous.

## Working agreement

- **Read the source before planning — every time.** This project is a distilled
  re-implementation of four studied harnesses, so **planning is a research task, not
  a from-memory task.** Before designing *any* task (not just the first), re-read
  the relevant `SPEC.md`/ADRs/`tasks/` **and go back to the actual harness source
  in the `coding-cli-survey` submodules** (`claude-code`, `codex`, `grok-build`,
  `opencode`) — plus the survey write-ups. Do this again and again as the design
  takes shape; do not trust memory or a single earlier pass. For each reference,
  study **why they do it, why they *don't* do the obvious alternative, and how the
  harnesses differ** — then distil the best practice. **Grok Build is the primary
  model for how to *unify* multiple providers/wires/tools behind one abstraction**;
  read how it does the unification before proposing our own. Ground every non-obvious
  design decision in a concrete source citation (`file:line`), and surface the "why"
  in the plan so it can be reviewed.
- **Faithful mimicry for harness packs.** When implementing a **ported** harness pack
  (grok, codex, claude, opencode), **faithfully reproduce that harness's real tools and
  behavior** — names, arg schemas, caps, guardrails, quirks — even where a "better" choice
  exists, because the whole point is an honest A/B. **Custom/best-of decisions apply only to
  our own `locode` pack.** Example: ripgrep-for-glob is our choice for the `locode` pack,
  but the grok pack ports grok's real `list_dir` walker (ADR-0011 amendment, ADR-0012).
  When a repo default (an ADR) and faithfulness collide for a ported tool, faithfulness wins
  for that pack — note it explicitly.
  - **The fidelity boundary — what mimicry covers, and where it stops.** A pack reproduces the
    harness's **tools, prompts, and static preamble** faithfully. It does **not** reproduce
    **loop-adjacent machinery** — reminder injection, TodoWrite/plan feedback loops, context
    compaction, subagent orchestration — which lives on our *shared engine* and is the same
    for every pack (an honest A/B varies the pack, not the loop). So the codex pack ports
    `update_plan` as a plain tool but not codex's plan-reminder loop; the claude pack ports
    `Read`/`Edit` but not Claude Code's `<system-reminder>` re-injection. When a ported tool is
    inseparable from loop machinery, port the tool's surface and leave the machinery to the
    engine — note the seam. (This was "STATUS #9" before `tasks/STATUS.md` was retired.)
- **ADR-first: keep the ADRs authoritative — reconcile them *before* changing code.**
  When a new finding or the user's latest instruction conflicts with an accepted ADR,
  do **not** just change the code (that is exactly what causes ADR-vs-code drift). Instead,
  in the **same change**: **minor** delta → amend/add a dated note to the ADR; **large**
  delta → write a **new ADR that supersedes** the old one; then make the code edit. A
  reader must be able to trust an ADR without checking the code. (If drift is discovered
  after the fact, reconcile the ADR in the fix — don't leave a "code is truth, ADR is
  legacy" gap.) The same holds for `SPEC.md`.
- **Spec before code.** For any non-trivial change, work from `SPEC.md` and the
  task tracker; if a task is missing, add it to `tasks/tracker.md` first. Deliver in
  thin, verifiable vertical slices — implement, test, verify, then expand.
- **One place for status: `tasks/tracker.md`.** The `tasks/` tree is deliberately flat:
  `tracker.md` (the *only* live status — Active/Next · Archive · Deferred), `plans/`
  (immutable, source-grounded per-task design records — no live checkboxes), and `audits/`.
  A task's checkbox exists in exactly one file — the tracker. Do **not** recreate
  `plan.md`/`todo.md`/`STATUS.md`: they were consolidated into `tracker.md` (2026-07-22)
  because three status copies drifted (a task marked done in one, open in another). When a
  task ships, flip its box in the tracker and add a plan "Result" addendum — never a second
  status list. (Note: the generic `/plan` and `/build` skills still write `tasks/plan.md` +
  `tasks/todo.md`; this repo does not use that path — it develops via the tracker and the
  autonomous TUI loop in `docs/tui-dev-process.md`.)
- **Route side effects through the seams the architecture defines** (see the ADRs):
  tools never touch the filesystem/shell directly, every side effect goes through
  the one dispatch door, and every `tool_use` is paired with exactly one
  `tool_result`. These are correctness invariants, not style preferences.

## TUI workstream: the autonomous slice loop

The TUI (Task 27: `locode-tui` + `locode-app`) is developed **near-fully
autonomously** under the binding process in
[`docs/tui-dev-process.md`](docs/tui-dev-process.md) (user decision,
2026-07-21). In one line per phase: (0) written status analysis — minimal
next unit, why, prereqs, what it unblocks; (1) mandatory re-read of how the
four harnesses did *this unit's area* (fresh citations; decide
implement-now / deferred / rejected; flag-don't-block user questions);
(2) plan doc `tasks/plans/task-27-slice-N-*.md` with test matrix + binary
preset targets; (3) implement + test until every target passes, full quality
gates + self-review; (4) PR → auto-merge → same-PR bookkeeping (checkboxes,
plan Result addendum, spec reconciliation); (5) loop without waiting. The
hard-stop list (new deps, core public surface, crate boundaries, releases,
scope past SPEC-TUI non-goals) is in the process doc and overrides autonomy.

## Git & GitHub workflow (agents drive this — no manual clicking)

This is a solo, automation-first project. Agents own the full git/GitHub flow via
the platform CLI (`gh` or equivalent); do not ask the user to push or click.

- **Branch for ALL work:** `type/short-desc` (`docs/…`, `feat/…`, `chore/…`,
  `fix/…`). `main` is branch-protected (since 2026-07-21, public repo): required
  status check `fmt · clippy · test · doc`, `enforce_admins`, linear history —
  **direct pushes to `main` are rejected by the platform, for everyone,
  including trivial fixes.** Check the current branch (`git branch
  --show-current`) BEFORE the first edit of any change, not at commit time.
- **Open a PR** with a real title and body describing what and why, then
  immediately arm auto-merge: `gh pr merge --auto --squash --delete-branch`.
  GitHub merges on green — no watcher process needed; a red check simply never
  merges. **Because a red check never merges,** run the full four-part gate above
  locally first — and if you *do* watch a PR (e.g. an autonomous serialize-then-wait
  loop), the waiter must exit on **CI-failure** too (`statusCheckRollup`
  conclusion `FAILURE`/`CANCELLED`/`TIMED_OUT`), never poll only for `MERGED` — a
  merged-only waiter loops forever on red and looks "stuck". Never report a PR as
  "on track / auto-merge armed" without confirming its checks aren't already red.
- **Tidy local branches after merging.** Deleting the branch (above) removes only
  the *remote* one; the local branch lingers, and since we **squash**-merge it
  isn't an ancestor of `main`, so `git branch -d` refuses it. Prune stale tracking
  refs and force-delete the gone ones:
  `git fetch -p && git branch -vv | awk '/: gone]/{print $1}' | xargs -r git branch -D`.
  Keep the local branch list ≈ just `main`.
- **`main` stays always-green:** enforced by the required check — nothing
  merges without `fmt · clippy · test · doc` passing. There is no
  direct-to-`main` escape hatch anymore; urgent fixes ride an auto-merged PR.
- **Commit messages:** imperative mood, explain the *why*. Attribute the agent that
  authored the change using that harness's own convention (e.g. a
  `Co-Authored-By:` trailer for the model that wrote it) — do not hardcode another
  harness's attribution.

## Quality bar (the mandatory gate)

Every change must pass, before merge (see [`docs/decisions/ADR-0010`](docs/decisions/ADR-0010-rust-tooling-baseline.md)).
These **four** commands are exactly the branch-protection required check
(`fmt · clippy · test · doc`) — run **all four locally before every push/PR**, not
just the first three:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps   # NOT optional — CI runs it
cargo test --workspace
```

The `doc` step is easy to forget and catches **broken intra-doc links** (a public
item linking `[`Foo::bar`]` from a module where `Foo` isn't in scope needs a fully
qualified path — `crate::…` / `locode_provider::…` / `Self::…`; a public doc
linking a private item needs a plain `` `code` `` span, not a `[link]`). Skipping
it red-CI'd a PR and stalled a whole task once — don't repeat that.

Prefer scoping to a crate while iterating (`-p <crate>`); run the full workspace
(all four) before merge. The canonical shortcut is `just check` (`fmt-check ·
clippy · test · doc`); `SPEC.md` → Commands lists the raw commands.

## Boundaries

- **Always:** run the mandatory four-part gate (`fmt · clippy · test · doc`) before merge; derive tool schemas from
  types; keep stdout to exactly one JSON report in the binary; guarantee transcript
  validity (every `tool_use` → one `tool_result`).
- **Ask first:** adding a dependency; changing the report envelope `schema_version`
  or a public trait signature (`Tool`, `Provider`); changing crate boundaries;
  enabling new `[workspace.lints]` denies.
- **Never:** commit secrets or API keys; print to stdout from library crates or any
  non-report path; bury allow/deny policy inside individual tools; leave a
  `tool_use` unpaired; introduce a second, throwaway loop for headless mode.
