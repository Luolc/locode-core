# AGENTS.md

Working rules for AI coding agents in this repo. This file is **harness-neutral**
and is the single source of truth, shared across the agents used here — **Claude
Code, Codex, and Grok Build** (and any other harness). Codex and Grok Build read
`AGENTS.md` natively; Claude Code imports it via `CLAUDE.md`. Edit this file, not
`CLAUDE.md`.

## What this project is

`locode-core` is the **headless Rust core library** of a custom coding agent
("locode"): the sample→dispatch→append loop, a typed tool registry, a
provider/wire abstraction, and a single structured-output contract. **No TUI and
no interactive permission prompts** live here — a separate future repo
(`locode-app`) will build the TUI on top of these crates.

Read these before starting implementation work — they are the source of truth for
what we are building and in what order:

- [`SPEC.md`](SPEC.md) — objective, crate layout, tool contract, testing, boundaries, success criteria.
- [`docs/decisions/`](docs/decisions/) — ADRs: the load-bearing decisions and the alternatives rejected. **The ADRs are the source of truth and must stay trustworthy — reconcile them *before* the code (see "ADR-first" in the Working agreement).**
- [`tasks/plan.md`](tasks/plan.md) + [`tasks/todo.md`](tasks/todo.md) — phased build order and the current task list.

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
- **ADR-first: keep the ADRs authoritative — reconcile them *before* changing code.**
  When a new finding or the user's latest instruction conflicts with an accepted ADR,
  do **not** just change the code (that is exactly what causes ADR-vs-code drift). Instead,
  in the **same change**: **minor** delta → amend/add a dated note to the ADR; **large**
  delta → write a **new ADR that supersedes** the old one; then make the code edit. A
  reader must be able to trust an ADR without checking the code. (If drift is discovered
  after the fact, reconcile the ADR in the fix — don't leave a "code is truth, ADR is
  legacy" gap.) The same holds for `SPEC.md`.
- **Spec before code.** For any non-trivial change, work from `SPEC.md` and the
  task list; if a task is missing, add it to `tasks/todo.md` first. Deliver in
  thin, verifiable vertical slices — implement, test, verify, then expand.
- **Route side effects through the seams the architecture defines** (see the ADRs):
  tools never touch the filesystem/shell directly, every side effect goes through
  the one dispatch door, and every `tool_use` is paired with exactly one
  `tool_result`. These are correctness invariants, not style preferences.

## Git & GitHub workflow (agents drive this — no manual clicking)

This is a solo, automation-first project. Agents own the full git/GitHub flow via
the platform CLI (`gh` or equivalent); do not ask the user to push or click.

- **Branch for non-trivial work:** `type/short-desc` (`docs/…`, `feat/…`,
  `chore/…`, `fix/…`). Don't commit non-trivial changes straight onto `main`.
- **Open a PR** with a real title and body describing what and why.
- **Squash-merge** and delete the branch (the repo is configured for squash-only +
  auto-delete). Keep `main` linear. Once CI exists, use auto-merge gated on green
  checks instead of merging by hand.
- **Tidy local branches after merging.** Deleting the branch (above) removes only
  the *remote* one; the local branch lingers, and since we **squash**-merge it
  isn't an ancestor of `main`, so `git branch -d` refuses it. Prune stale tracking
  refs and force-delete the gone ones:
  `git fetch -p && git branch -vv | awk '/: gone]/{print $1}' | xargs -r git branch -D`.
  Keep the local branch list ≈ just `main`.
- **`main` stays always-green:** every merged change must pass the mandatory
  checks below. Direct-to-`main` is reserved for trivial or urgent fixes.
- **Commit messages:** imperative mood, explain the *why*. Attribute the agent that
  authored the change using that harness's own convention (e.g. a
  `Co-Authored-By:` trailer for the model that wrote it) — do not hardcode another
  harness's attribution.

## Quality bar (the mandatory triangle)

Every change must pass, before merge (see [`docs/decisions/ADR-0010`](docs/decisions/ADR-0010-rust-tooling-baseline.md)):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Prefer scoping to a crate while iterating (`-p <crate>`); run the full workspace
before merge. See `SPEC.md` → Commands for the full list (and `just check` once the
`justfile` lands).

## Boundaries

- **Always:** run the mandatory triangle before merge; derive tool schemas from
  types; keep stdout to exactly one JSON report in the binary; guarantee transcript
  validity (every `tool_use` → one `tool_result`).
- **Ask first:** adding a dependency; changing the report envelope `schema_version`
  or a public trait signature (`Tool`, `Provider`); changing crate boundaries;
  enabling new `[workspace.lints]` denies.
- **Never:** commit secrets or API keys; print to stdout from library crates or any
  non-report path; bury allow/deny policy inside individual tools; leave a
  `tool_use` unpaired; introduce a second, throwaway loop for headless mode.
