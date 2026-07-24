# The claude-pack development process — autonomous slice loop

**This is the entry point for the Claude Code pack workstream (Task 20).** It is
written to be **self-contained**: a fresh agent context should be able to start
here, read the linked grounding docs, and execute the whole workstream without
re-deriving the decisions below. Mode: **near-fully autonomous** (same contract as
the TUI workstream — [`docs/tui-dev-process.md`](tui-dev-process.md)) — the agent
drives every phase; the human reviews merged PRs asynchronously and answers batched
questions.

**What this builds:** `--harness claude`, a **faithful port of Claude Code's
headless-relevant toolset + static system prompt + static preamble** — the second
studied-harness pack after `grok`, the highest-value A/B counterpart (same engine,
same wire, genuinely different tool surface). Wire-independent (runs on
`anthropic | openai-responses | openai-chat`).

> **The one rule that overrides everything: faithful reproduction.** Reproduce
> Claude Code's real behavior — names, arg schemas, verbatim descriptions, caps,
> guardrails, result formatting, prompt bytes. **No unmotivated omissions, no extra
> features.** Deviate only where a decision below explicitly says so (a deferred
> feature, an accepted gap). When in doubt, go back to the source and match it.

---

## Grounding documents (authority order)

1. **Accepted ADRs** — [`ADR-0012`](decisions/ADR-0012-harness-packs.md) (harness
   packs: fidelity beats DRY; each pack = real tools + its own prompt),
   [`ADR-0023`](decisions/ADR-0023-fidelity-boundary-and-agents-md-loading.md) (the
   **fidelity boundary** — pack = tools + prompt + static preamble; loop-adjacent
   machinery is shared engine),
   [`ADR-0013`](decisions/ADR-0013-conversation-protocol.md) (4-role protocol;
   injected framing is `User`, not `Developer`),
   [`ADR-0003`](decisions/ADR-0003-typed-tool-contract.md) (typed `Tool` contract),
   [`ADR-0008`](decisions/ADR-0008-dispatch-door-and-path-jail.md) (host seam + jail
   + central truncation), [`ADR-0011`](decisions/ADR-0011-search-ripgrep-bundling.md)
   (rg resolution).
2. **[`SPEC.md`](../SPEC.md)** — crate layout, boundaries, the fidelity-boundary
   statement.
3. **[`tasks/plans/task-20-claude-pack.md`](../tasks/plans/task-20-claude-pack.md)**
   — the **detailed per-tool/per-section design** (699 lines: full tool inventory,
   arg schemas, per-tool caps/guardrails, prompt IN/OUT split, module layout, tests).
   This working doc records the *process + resolved decisions + slice plan*; that
   plan is the *design detail*. Its §9 open questions are **now resolved** (see
   Resolved decisions below and the plan's reconciliation note).
4. **Harness study docs** — the injection/prompt studies
   ([agents-md](research/harness-study-agents-md.md),
   [skills](research/harness-study-skills.md)) and the
   `claude-code-system-surfaces` memory (three system surfaces + the
   mid-conversation-system beta).
5. **[`tasks/tracker.md`](../tasks/tracker.md) Task 20** — the live status line.
6. **Repo rules — [`AGENTS.md`](../AGENTS.md)** apply unchanged (ADR-first,
   faithful-vs-custom boundary, quality gate, git workflow, voice-input hygiene).

**The source of truth for behavior:** the reconstructed Claude Code source under
`~/dev/coding-cli-survey/submodules/claude-code` (submodule commit **`6a25909`**),
plus `survey/01-claude-code/*`. **Re-read it per slice (Phase 1)** — planning is a
research task, not a from-memory task (AGENTS.md). The grok pack
(`crates/locode-packs/src/grok/`, Tasks 9–13) is the **template** for tool ports,
provenance-pinned descriptions/prompt, byte-pin tests, and the `strip_identity` knob.

---

## Resolved decisions (interview 2026-07-24 + the task-20 plan)

These are settled. Do not re-litigate; implement to them. (Rationale for each is in
the task-20 plan §5 and the ADRs.)

### Scope & fidelity
- **D1 — Tool set = the six.** `Bash, Read, Edit, Write, Glob, Grep` (Claude Code's
  headless-relevant core; 1:1 with our `ToolKind`s). Everything else **deferred** —
  see the Gap log.
- **D2 — Fidelity boundary (ADR-0023).** The pack reproduces **tools + system prompt
  + static preamble ONLY**. Loop-adjacent machinery — project-instruction loading,
  reminder re-injection, compaction, subagents — is **shared engine**, never in the
  pack.
- **D3 — Read-before-edit + staleness gate is PORTED.** A per-run
  `ClaudeSessionState` freshness store (path → mtime at last Read); Edit/Write
  consult it (unread → soft error; modified-since-read → soft error), Read + success
  update it. This is Claude Code's signature guardrail and the deliberate behavioral
  divergence from the grok pack (which faithfully has none). **P0.**
- **D4 — Result output text is P0.** Reproduce Claude Code's *model-facing* result
  format as closely as the source shows (Read `cat -n`, per-tool caps, truncation
  markers). It is A/B behavior, not cosmetic. (Not the interactive UI rendering —
  the *tool_result text the model sees*.)
- **D5 — `deny_unknown_fields` on every Args** (mirrors CC's `z.strictObject` →
  `additionalProperties:false`); verbatim field descriptions via
  `#[schemars(description = …)]`.

### Prompt & identity
- **D6 — Both identities + strip.** Headless (`ctx.headless == true`) →
  `AGENT_SDK_PREFIX` ("You are a Claude agent, built on Anthropic's Claude Agent
  SDK."); interactive → `DEFAULT_PREFIX` ("You are Claude Code, Anthropic's official
  CLI for Claude."). `strip_identity` removes the identity prefix (**both variants**)
  and must stay compatible — mirror grok's knob; pin it with a test so a section
  refresh can't silently no-op it.
- **D7 — System prompt: byte-exact, §4.7 IN/OUT split.** Verbatim Rust constants for
  the sections CC produces **for our exact 6-tool pool** (identity + intro + system +
  doing-tasks + actions + using-your-tools + tone + output-efficiency + env);
  byte-pinned snapshots. **EXCLUDE** the loop-adjacent dynamic content
  (`SYSTEM_PROMPT_DYNAMIC_BOUNDARY`: memory/CLAUDE.md, output-style, language, MCP;
  all `<system-reminder>` attachments). Render exactly the branch matching
  `{Bash,Read,Edit,Write,Glob,Grep}` (drops TodoWrite/Task/Skill bullets).
- **D8 — Descriptions: verbatim, minus CC's own off-branches.** Store as
  provenance-pinned `descriptions/*.md` (`include_str!` + sha256 + commit + length
  pin — grok template). Where CC's source **conditionally** omits a paragraph when a
  feature is off (e.g. background/sandbox), render that off-branch; where a mention
  is **unconditional**, keep it verbatim and **log it as a gap** in the file's
  provenance header. No blanket stripping.

### Context organization (the 2026-07-24 brainstorm — "Model C")
- **D9 — env block in the pack's System prompt.** Faithful (CC puts env in system).
  `PackContext` grows **`is_git_repo: bool`** and **`model: Option<String>`** (cheap,
  computed by the exec/tui layer — a `.git` probe + `EngineConfig.model`; **no host
  handle in `preamble()`**). Skip the "You are powered by the model named X" line
  when `model` is absent (don't guess).
- **D10 — Preamble = `[System(prompt+env), User(<system-reminder> currentDate)]`.**
  The User message is CC's first-turn context reminder (`prependUserContext`),
  verbatim wrapper, **`currentDate` entry only**. `Role::User` (not `Developer`) —
  it *is* a user-message system-reminder on CC's real wire, and keeps bytes identical
  (ADR-0013 / ADR-0023).
- **D11 — AGENTS.md comes from the shared engine, not the pack.** The pack does **not**
  load or inject project instructions; the engine's shared loader (ADR-0023, Task 30)
  injects them as a separate `User` `<system-reminder>`. **Do not duplicate** in the
  system prompt or the pack preamble.
- **D12 — CLAUDE.md not read (accepted gap).** The shared loader reads `AGENTS.md`
  only (ADR-0023). The claude pack inherits that; it does **not** add CLAUDE.md
  loading. Logged in the Gap log.
- **D13 — git-status tail DEFERRED (Model C).** Claude Code's `appendSystemContext`
  git snapshot (branch / changed files / recent commits) is **Claude-specific** and
  **dynamic** (runs `git`, would need host-in-`preamble()`). Deferred as a gap;
  revisit as a **dedicated "session-start context organization" decision** once the
  codex/opencode packs' env/context needs are visible (designing that seam for one
  pack now is premature). The **three-layer model** behind this (① shared engine
  machinery held constant for a clean A/B → AGENTS.md; ② pack faithful surface → the
  experiment → prompt/env/first-turn-reminder/[git-status]; ③ the user's task) is the
  organizing principle to carry into that later decision.

### Per-tool caps (from the plan §4; confirm against source per slice)
- **Bash:** `bash -lc` combined output (`Host::exec`); timeout default 120 000 ms,
  hard max 600 000 ms; output cap 30 000 chars (middle-truncate w/ marker). Drop
  `run_in_background`, `dangerouslyDisableSandbox`, `_simulatedSedEdit` (the last one
  CC itself `.omit()`s — dropping it is *faithful*).
- **Read:** absolute path; default 2000-line window; `offset`/`limit`; **`cat -n`**
  output (1-based, tab sep); per-line 2000-char truncation; records freshness. Drop
  `pages` (PDF, text-only v0).
- **Edit:** the D3 gate order — unread → stale → `old==new` → not-found → multi-match
  (needs `replace_all`). No file creation (that's Write). 1 GiB size cap.
- **Write:** create-or-overwrite; mkdir parents (CC writes through; our
  `Host::write_file` doesn't auto-create — add it); existing-file must-read-first via
  the store; records new mtime.
- **Glob:** `rg --files -g <pattern>` under the root, sort by mtime desc, cap **100**
  with a truncated note; no-match → soft-ok.
- **Grep:** full ripgrep passthrough (`-A/-B/-C`, `-n`, `-i`, `type`, `glob`,
  `multiline`); `output_mode` default `files_with_matches`; `head_limit` default
  **250** (`0`=unlimited); 20 000-char result cap; rg exit 1 → "No matches found",
  ≥2 → soft error w/ stderr.
- Central dispatch-door truncation (ADR-0008 amendment) stays **on top** as the
  engine-side belt.

---

## Gap log (accepted, documented fidelity gaps — keep current in the pack module docs)

Faithful mimicry with **explicit** gaps (never silent). Each is either a decided
deferral or a standing locode substitution. When a slice touches one, restate it in
that PR.

- **Loop-adjacent (fidelity boundary, D2):** TodoWrite + its reminder cadence; `Task`
  subagents; all `<system-reminder>` attachment machinery; compaction. Live on the
  shared engine, not the pack.
- **Infra-gated:** WebFetch/WebSearch (no HTTP/search host seam); persistent shell
  session (host is per-call `bash -lc`); background Bash + OS sandbox (SPEC assumption
  4); PDF/image reads (`Read` text-only); ant-internal / feature-flag tools.
- **Context (D11–D13):** CLAUDE.md not read (shared loader is AGENTS.md-only);
  git-status tail deferred (Model C).
- **Substitutions:** path jail = our `PathPolicy` (ADR-0008), not CC's permission
  system; result *rendering* approximated where UI-coupled (but result *text* is P0,
  D4).

---

## The loop (per slice — same five phases as the TUI process)

A "unit" is one of the slices in the plan below; subdivide if a PR would exceed
reviewability (agent's call, recorded in the plan).

- **Phase 0 — Status analysis (written, top of the slice plan doc):** re-read this
  doc's Resolved decisions + the task-20 plan + the previous slice's Result addendum;
  inspect the merged code (git log, crate tree, tests). State: minimal next unit; why
  now / what it unblocks; prerequisites (+ a check they hold); 2–4 risks.
- **Phase 1 — Source revisit (mandatory, per slice):** go back to the **Claude Code
  source** (`submodules/claude-code`, commit `6a25909`) for *this slice's* tool/section
  — fresh `file:line` citations, not memory. Record what CC does, the faithful
  target, and any gap discovered (add to the Gap log). Cross-check the grok pack's
  handling of the analogous piece.
- **Phase 2 — Plan doc:** `tasks/plans/task-20-slice-N-<name>.md` — Phase 0 + Phase 1
  records; design (module touch points, arg struct, caps, guardrails, output shape);
  edge cases; **test matrix** (schema golden, description provenance pin, behavior via
  `build_registry`+`dispatch` over a tempdir host, prompt snapshot where relevant);
  preset targets (binary, testable).
- **Phase 3 — Implement + test:** branch `feat/task-20-slice-N-<name>`; test-first
  where shape allows; iterate until every preset target passes; then the **four-part
  gate** (fmt · clippy · **test** — confirm no `FAILED`/`panicked` lines directly ·
  doc) + a self-review pass over the diff (correctness, dead code, naming, comment
  discipline, simplification). Bounded-resource audit for any new buffer/cap.
- **Phase 4 — Ship:** PR (what/why, test evidence, **deviations from the plan**,
  batched open questions); `gh pr merge --auto --squash --delete-branch`; prune local
  branches. **Same-PR bookkeeping:** Task 20 checkbox, plan **Result** addendum,
  ADR/SPEC reconciliation if a decision drifted (ADR-first). If CI reddens on
  something local gates passed, fix forward on the branch.
- **Phase 5 — Continue:** loop to Phase 0 for the next slice without waiting, unless a
  hard-stop is pending. A later-found defect is a new small slice through the loop.

---

## Slice plan (proposed order — agent's call, revise per Phase 0)

Each tool is its own slice; the system prompt is its own slice; `ClaudeSessionState`
rides with the first tool that needs it (Read). Order picks the shippable/testable
path (a minimal runnable prompt exists from Slice 1 so `--harness claude` runs
end-to-end; the full byte-pinned prompt lands last).

1. **S1 — pack scaffold + `Bash` + minimal prompt.** `ClaudePack` + `Pack` impl +
   `register` + `resolve("claude")` + `--harness claude` wired through exec/tui; a
   minimal-but-real prompt (identity prefix by D6 + intro) so the pack runs against
   `--api-schema mock`. `Bash` (simplest tool, no shared state). Proves the pack
   end-to-end.
2. **S2 — `Read` + `ClaudeSessionState`.** The freshness store; `cat -n` output;
   2000-line window; records freshness.
3. **S3 — `Edit`.** The read-before-edit + staleness gate (D3), in CC's check order.
4. **S4 — `Write`.** Must-read-first for existing files; mkdir parents; records mtime.
5. **S5 — `Glob`.** `rg --files -g`, mtime sort, 100-cap.
6. **S6 — `Grep`.** Full rg passthrough surface + caps.
7. **S7 — full system prompt + preamble.** All D7 sections as byte-pinned verbatim
   constants; identity both-variants + `strip_identity` (D6); env block (D9) with the
   `PackContext` growth (`is_git_repo`, `model`) + exec/tui plumbing; preamble = the
   `currentDate` User reminder (D10). Replaces S1's minimal prompt.

(git-status tail is **not** a slice — deferred, D13.)

---

## Autonomy contract (this workstream)

**Decide alone (record in plan/PR):** everything inside `locode-packs` (the claude
pack modules, `ClaudeSessionState`, descriptions, prompt constants, snapshots);
**pack-framework + `PackContext` + `Pack` additions + exec/tui plumbing** needed to
carry pack context (e.g. the `is_git_repo`/`model` fields, D9); module/test/naming
design; slice subdivision; reversible in-scope trade-offs; choosing the reversible
default for a flagged open question.

**Hard stops (batched where possible):**
1. **Core public surface** — `locode-protocol` types, the `Provider`/`Tool` trait
   signatures, the report envelope / `schema_version`, `locode-core` facade exports.
2. **Crate boundary changes** (new crate, moved crate).
3. **Publishing / releases / version bumps / tags** (always the user's call).
4. **Heavy / niche / security-sensitive dependencies.** Reasonable, well-justified
   deps may be added *with the justification in the plan/PR* — e.g. `globset` **only**
   if `rg --glob` genuinely diverges from CC's globber on some pattern class
   (document the divergence). Nothing heavy without asking.
5. **Reopening a Resolved decision (D1–D13)** or expanding scope past the six tools /
   the §4.7 prompt split.
6. Anything destructive or outward-facing beyond branch → PR → merge.

Questions never block the next slice unless they're a hard-stop on its critical path;
they accumulate for batched review, with the reversible default taken meanwhile.

---

## Standing constraints
- **Core crates stay headless** (ADR-0001). The pack reaches the OS only through
  `locode-host` (exec, fs, rg resolution, jail) — never `std::fs`/`Command` directly.
- **Faithful mimicry wins on conflict** with a repo default, *for a ported pack*
  (AGENTS.md) — note it explicitly (e.g. `deny_unknown_fields` even though grok is
  permissive; `cat -n` numbering even though grok differs).
- **Every `tool_use` → exactly one `tool_result`**; tool failures are soft
  `tool_result{is_error}`, not fatals, unless CC itself hard-fails the turn.
- All repo writing in **English**; user-facing chat follows the user's language.
- The study/plan docs are living: a Phase-1 revisit that finds the plan wrong amends
  the plan in the same PR with a dated note.

---

## First action after a context reset
Read this doc top-to-bottom, then the task-20 plan, then open the Claude Code source
at commit `6a25909` for **Slice 1** (the tool registry `src/tools.ts:193-251`, the
`BashTool`, and the prompt prefix `constants/system.ts`) and run **Phase 0** for S1.
