# locode-core — task tracker

**This file is the single source of truth for task status.** A task's checkbox lives here
and *nowhere else*. Design detail for each task is an immutable, source-grounded record
under [`plans/`](plans/) (indexed in [`plans/README.md`](plans/README.md)); the *why*
behind each decision lives in the ADRs ([`../docs/decisions/`](../docs/decisions/)).

> **Single-source invariant.** Status is tracked in exactly one place — this file. The
> `plans/*.md` are point-in-time design records and do **not** carry live checkboxes;
> `plans/README.md` is just an index. There is no `plan.md`, `todo.md`, or `STATUS.md`
> anymore: they were merged into this tracker (2026-07-22) precisely because keeping status
> in three files let the least-edited copy rot. Keep it to one.

Current release: **0.1.19**. Sizes: XS = 1 file · S = 1–2 · M = 3–5 · L = 5–8 (split if larger).

## Architecture at a glance

```
locode-protocol   (pure types)
    ├── locode-host       (fs / shell / path-jail / truncation / rg)
    │       ├── locode-instructions  (AGENTS.md discovery + its <system-reminder>)
    │       └── locode-skills        (SKILL.md discovery + the skills listing)
    ├── locode-tools      (Tool trait, registry, dispatch door — framework)
    │       └── locode-packs   (grok pack: real per-harness tools, over tools + host)
    ├── locode-provider   (Provider trait + wires: mock · anthropic · openai-responses)
    └── locode-engine     (sample→dispatch→append loop + Session)  ← composes the above
            └── locode-core (facade) ── locode-exec (headless runner lib: run_headless)
                    └── locode-tui (components + TUI + `-p` headless) ── locode-app (binary)
```

Full crate layout and rationale: [`../SPEC.md`](../SPEC.md), ADR-0002 (workspace), ADR-0019 (TUI crates).

---

## Active / Next — the only open work

### Priorities (user, 2026-07-24)
The immediate queue, ahead of the remaining packs:
- **DONE — Task 31: settings.json + trace persistence (continue/resume)** — shipped
  2026-07-24 (PRs #173–#176) under
  [ADR-0024](../docs/decisions/ADR-0024-locode-home-settings-and-traces.md); see the
  Home-dotfolder section below.
- **P0 (next) — Task 32: Skills.** Full skills implementation, decided in
  [ADR-0025](../docs/decisions/ADR-0025-agent-skills.md) (Accepted, 2026-07-24):
  shared discovery in `locode-host`, five-key frontmatter, a budgeted `User`
  `<system-reminder>` listing compared as a **whole body** (change → re-send all),
  rescanned **after each run completes**, and **no new tool** — the listing carries
  absolute paths and the model reads `SKILL.md` with the pack's own read tool. Rides
  along per the same ADR: an ADR-0008 **read-only** jail exception for the locode home
  + external skill roots (§4.1), and `extends` becoming a **dotfolder** pointer whose
  `settings.json` + `skills/` + `AGENTS.md` all merge (§6) with the settings-first load
  order (§6.1). Plan (5 slices): [`plans/task-32-skills.md`](plans/task-32-skills.md) — **the immediate next task**.
- [x] **P0.5a — Mid-run user input (queue-jumping)** — **SHIPPED 2026-07-26**
  ([ADR-0028](../docs/decisions/ADR-0028-mid-run-user-input.md), Accepted; plan +
  Result: [`plans/task-33-mid-run-user-input.md`](plans/task-33-mid-run-user-input.md)). **Sequenced before background tasks**, which reuse its queue as a
  payload variant rather than building a second mechanism. Today a prompt typed mid-run waits
  for the whole loop; all three studied harnesses do better (grok included — the earlier
  "grok can't" assumption was wrong: `pending_interjections` + PTY tests). They differ only in
  drain granularity, and that alone explains the UX gap: Claude Code drains per **loop
  iteration** into the tool-result batch (fast, so its UI is quiet), Codex per **turn** (slow,
  so it ships a "Queued follow-up inputs" pane). We take Claude Code's granularity and Codex's
  visibility — independent axes. Engine: a `QueuedInput` enum, `InputQueued`/`InputDelivered`
  events, one drain step between dispatch and re-sample, a no-tool-calls fallback, cancel
  clears. UI: queued vs delivered must be visibly distinct.
- **P0.5 (the next big task) — stand up the `locode` best-of pack, and build background
  tasks + subagents *inside* it** (user decision, 2026-07-27). Two entries that were tracked
  apart — "P0.5 background Bash + subagents" and Task 15's `locode` half (was P1) — become
  **one workstream**, because the first question background raised ("which harness gets
  it?") already had its answer: our own pack. Nothing new lands in a *ported* pack;
  `claude`/`codex`/`grok` reproduce their harnesses and stop there (ADR-0023).
  - **Order of work**: the `locode` pack itself first (best-of toolset + our own prompt),
    then a **batch tool with background support**, then the task/subagent surface on top of
    it. The batch framing is the user's call (2026-07-27) and differs from the study's
    recommendation of a per-tool `is_background` flag ported per pack — reconcile the two
    explicitly when the ADR is written, don't let the delta pass silently.
  - **Already in place**: P0.5a mid-run input ([ADR-0028](../docs/decisions/ADR-0028-mid-run-user-input.md))
    shipped the queue, the drain step between dispatch and re-sample, and the queued vs
    delivered UI split. **Task notifications are a second payload kind on that queue**, not
    a second mechanism — the study's `mode:'task-notification'` alongside `mode:'prompt'`,
    with addressing (main thread vs a subagent's own `agentId`) as the new part.
  - **The fidelity boundary is unchanged** by putting the surface in `locode`: the
    *mechanism* — host task registry, the drain step, the nested engine loop — stays on the
    shared engine and is identical for every pack; what the `locode` pack owns is the
    **tool surface** over it. A ported pack that later wants background gets a skin, not
    its own machinery.
  - **Two ADRs are proposed and still unwritten** (study §Proposed ADRs): background tasks +
    the host task registry, and subagents as a nested engine loop. The pack-home decision
    above belongs in whichever lands first — ADR-first, before the code.
  - Also unblocks codex `shell_command` → unified exec (see the deferred seam below).
- [x] **Task 35 — session picker** — **SHIPPED 2026-07-29** (#259/#260/#261; small, ahead of P0.5;
  [ADR-0029](../docs/decisions/ADR-0029-session-picker.md), **Accepted** 2026-07-29; plan:
  [`plans/task-35-session-picker.md`](plans/task-35-session-picker.md)). `--resume` needs an id nobody remembers, and `-c` is the only escape
  (newest only, no choice). All three studied harnesses ship a picker and agree on its shape —
  title + one dim metadata line, newest first, current directory by default with a toggle —
  differing only in entry point (claude: optional flag value; codex: a subcommand; grok: a slash
  command). We take claude's entry (`-r [SESSION_ID]`) plus grok's (`/resume`), scan the sessions
  directory rather than building ADR-0024's reserved index, and reuse `/new`'s existing mid-run
  refusal rather than inventing a rule. Three slices: `list_sessions` in the host (pure, unit
  tested) → the picker overlay + the `-r` entry (reducer tests + the first `TestBackend` render
  snapshot) → `/resume`. v0 excludes preview panes, deep search, rename, tags, and fork-on-resume.
- **~~P1 — opencode pack~~ — CANCELLED** (user decision, 2026-07-24). The ported-harness
  reproduction workstream **ends with skills**: `claude` / `codex` / `grok` are the three
  packs we ship, and no fourth port is built. The drafted plan
  ([`plans/task-15-opencode-pack.md`](plans/task-15-opencode-pack.md)) stays as a record.
- **P1 — parallel tool dispatch** ([ADR-0027](../docs/decisions/ADR-0027-parallel-tool-dispatch.md),
  **Draft — not approved**, 2026-07-26). Source study done and captured while fresh; the
  implementation effort is real, so it waits. Two pieces, in order: (1) split approval into
  a batch phase that completes before any execution — needed regardless, and the piece with
  the actual design content; (2) per-path locking following grok (`tool_calls.rs:387-404`),
  **not** the global `RwLock` ADR-0005 prescribed — that serializes edits to unrelated
  files, the commonest multi-tool turn. `ToolKind::Shell`/`Other` stay exclusive (Claude
  Code's rule; a shell call declares no path). Model-emitted order stays an invariant so
  eval A/Bs don't acquire scheduling noise. Serial dispatch remains shipped until accepted.
- ~~**P1 — our own `locode` best-of pack**~~ — **merged into P0.5 above** (2026-07-27). It
  was always "the home for every capability after skills"; background tasks are the first
  such capability, so the pack and its first tools ship together rather than in sequence.
- **P1–P2 — tool-error text belongs behind a debug view.** Tool `is_error` results
  (`invalid arguments: …`, the truncation notice) currently render inline in the TUI
  above each tool. That is deliberate for now — it is the fastest way to see a
  malformed call while developing — but a real user should not read decode errors.
  Move it behind the debug/hidden-context surface once that exists, rather than
  deleting it. Raised 2026-07-25 alongside the `max_tokens` truncation fix.

### Fixes
- [x] **Transcript durability — three ways a session became permanently unsendable**
  (2026-07-27 → 2026-08-01; #255, #256, #263, #264). One symptom class, three causes,
  found in the wrong order:
  - **A blank text block in the history** (#255). Anthropic emits empty text blocks and
    rejects them on input; the history replays every turn, so one such block ended the
    session. The wire now drops blank text on the way out, which also *heals* an
    already-poisoned rollout on resume. Same PR: a lossy stream stopped passing as a
    short answer — a recognized-but-unparsable frame, `stop_reason: tool_use` with no
    tool call, or a delta for a block that never started all raise the retryable
    transport error truncation already raised (ADR-0007 amendment).
  - **A retried stream rendering its reply twice** (#256). Making streams retryable
    exposed it, so `Event::MessageDeltaReset` annuls a partial stream the resample is
    about to replace (ADR-0014 amendment).
  - **A `tool_use` whose result was one message too late** (#263, #264). The pre-send
    repair checked whether an id was answered *anywhere*; the API requires the result in
    the **next** message. Repair is now positional (ADR-0004 amendment). The cause,
    found only after the rollout was finally read, was **two locode processes appending
    to one file** — a live session in one terminal, `--resume` in another. Fixed at the
    source by recording lineage on every history record and replaying the chain from the
    newest leaf instead of trusting file order (ADR-0024 amendment) — Claude Code's
    `parentUuid` property, which our study had recorded as a behavior without its
    mechanism. A file lock was considered and rejected: it guards the situation rather
    than removing it, and lineage makes it redundant.
  - What this taught about *how we work* is in [`META-AGENTS.md`](../META-AGENTS.md)
    §4 F5–F7 (diagnosing without reading the artifact; guards that delete the signal;
    documents that record behavior without mechanism) and the rules §5.5–§5.7.
- [x] **`/effort` + `/add-dir` commands, and locode's own effort ladder** (2026-07-26,
  #235/#236). Effort was reachable but unwired — nothing ever set `reasoning_effort`, so
  every run took the API default. Now a `--effort` flag, an `effort` settings key, and an
  `/effort` command over **our** ladder (`low·medium·high·xhigh·max`), each wire mapping
  it; `Effort::maps_to` shows the wire value in the menu's second column so a future
  collapse is visible. Rungs verified against the live API, not the vendored source (which
  predates Fable 5 and lists no `xhigh`): `ultra`/`ultrathink`/`extreme` all 400.
  `ultracode` is a composite mode ("xhigh + workflows" in Claude Code's own UI), not a
  sixth rung. `/add-dir <path>` widens a **running** session — jail now, AGENTS.md +
  skills next turn — with the root lists behind an `Arc<RwLock<…>>` because tools already
  hold `Arc<Host>`. Not persisted (a working dir belongs to the task, not the profile).
- [x] **`reasoning_tokens` reported** (2026-07-25, #234). Parsed from
  `output_tokens_details.thinking_tokens`; deliberately **not** in `context_tokens()` —
  thinking is replayed into the next request, so it already lands in that turn's
  `input_tokens`.
- [x] **`--add-dir` (multi-root)** (2026-07-25). Lifts ADR-0023's deferred seam now that
  the ADR-0008 jail change is made. One repeatable flag on both CLIs with three effects:
  widens the path jail, adds the directory's `AGENTS.md` to the instruction walk
  (`InstructionsConfig.extra_roots`, the seam that already existed), and adds its
  `.agents/skills` (new `SkillsConfig.extra_roots`). Roots canonicalize at startup so a
  typo names the path. The jail's lexical pre-check now matches roots in **both**
  as-given and canonical form — without that, a root behind a symlink (macOS `/var`, and
  any symlinked monorepo mount) rejects absolute paths the canonical check would accept.
  Verified end-to-end under `--restricted`: read a file outside cwd, and picked up the
  added dir's AGENTS.md rule + skill with zero tool calls. **Not built:** a
  `settings.json` key (ADR-0023 flagged it unreviewed — CLI-only for now).
- [x] **Thinking was left to the serving model** (2026-07-25, #231, **breaking**). Nothing
  ever set `reasoning_effort`, so the wire sent no `thinking` field at all — which does
  **not** mean "off": Opus 4.8 read the absent field as no-thinking (verified:
  `thinking_tokens: 0`), Opus 5 ran adaptive, Fable 5 thought regardless. One codebase,
  three behaviours, visible in this repo's own traces (a session that switched models
  mid-run went from 0 reasoning blocks to 1 at the switch). The wire now always sends
  `{type:"adaptive", display:"summarized"}`; `reasoning_effort` chooses depth only, via
  `output_config.effort`. `ReasoningEncoding` is **removed** rather than re-defaulted —
  its `Budget` variant emitted `budget_tokens`, which every current model rejects, and it
  was the default, so the default config was broken for every model we run. `EFFORT_BETA`
  removed (effort is GA); `temperature` is no longer sent on this wire. Reconciled into
  ADR-0007. **Gap:** adaptive is unsupported pre-4.6 (Sonnet 4.5, Haiku 4.5) — out of
  scope by user decision, would need the deferred per-model table.
- [x] **Output budget 64k + no silent ceiling** (2026-07-25, #230, **breaking**).
  `DEFAULT_MAX_TOKENS` → 64k (Claude Code's `ESCALATED_MAX_TOKENS`; not 128k because
  `upperLimit` is per-model and Haiku 4.5 stops at 64k). `ModelConfig.max_tokens_cap`
  → `Option<u32>`, `None` by default on both wires: a `min` ceiling is silent by
  construction, which ADR-0007 already rejects for `reasoning_effort`. The budget itself
  cannot be `Option` — Anthropic requires `max_tokens` (opencode encodes the same
  asymmetry: required for Anthropic, optional for both OpenAI protocols).
- [x] **`max_tokens` truncation loop** (2026-07-25). `SamplingArgs::default()` shipped
  4096 output tokens and the Anthropic wire clamped to 8000, so an ordinary `Write` was
  cut mid-call; the wire returns an empty `input` for a truncated `tool_use`, the typed
  decode blamed the model for a missing field, and the model retried the same oversized
  call forever (reproduced live on `claude-fable-5`, whose always-on thinking shares the
  same budget). Budget → 32k, Anthropic ceiling → 64k, and the loop now answers a
  truncated call by naming the output-token limit instead of dispatching it. Grounded in
  Claude Code's per-model `{default, upperLimit}` table and its escalate-on-truncation
  path; reconciled into ADR-0004 / ADR-0005 / ADR-0007 (dated amendments).

### Home dotfolder (`~/.locode`, ADR-0024)
- [x] **Task 31 — settings + trace persistence** (P0, **complete 2026-07-24**, PRs
  #173–#176 — plan + Result:
  [`plans/task-31-locode-home-settings-and-trace.md`](plans/task-31-locode-home-settings-and-trace.md)):
  - [x] **S1** — `locode_home()` resolver + layered settings loader (merge + project
    denylist + `extends`) + `model`/`api_schema`/`harness` defaults wired into exec/tui.
  - [x] **S2** — `instructions.root_stop_pattern` activation (the approved `regex`
    dep; wakes ADR-0023's dormant seam).
  - [x] **S3** — trace writer: bijective cwd encoding + rollout JSONL as sink
    decoration (session_meta + message lines, torn-tail healing, 0600/0700).
  - [x] **S4** — `--continue`/`--resume <id>`: tolerant reader + scoped-then-global
    resolver + resume-as-preamble seeding; resumed runs append in place.
  - [x] **Followups (smoke test, shipped in 0.1.10):** TUI resume-transcript replay
    (#178); footer shows **context occupancy** + per-run `usage` records for exact
    resume (#179); first-run `settings.json` scaffold + `--model` flag +
    `claude`/`claude-sonnet-5` defaults + resume-model-from-flag/settings (#181);
    `--no-session-persistence` + the `.cargo`-redirect revert so `cargo run -c` finds
    real sessions (#180/#182). Reconciled into ADR-0024 (dated amendments).
- [ ] **Task 32 — skills** (P0, ADR-0025 accepted; plan:
  [`plans/task-32-skills.md`](plans/task-32-skills.md)):
  - [x] **S1** — `extends` becomes a dotfolder (`settings.json` + `skills/` + `AGENTS.md`)
    and settings-before-discovery becomes a load-order invariant.
  - [x] **S2** — new `locode-skills` crate: discovery, five-key frontmatter, precedence,
    three-scope collisions.
  - [x] **S3** — listing body: grok's verbatim format, 50 % budget, three-tier degrade.
  - [x] **S4** — injection: whole-body diff, previous state read off the transcript,
    removal notice.
  - [x] **S5** — post-run rescan seam (scan after the terminal `Result`; session start is
    the one synchronous scan).
  - **All five slices shipped 2026-07-24** (PRs #196–#201, plus #199 moving project
    skills to `<repo>/.agents/skills`).

- [x] **Task 33 — `--debug-show-hidden-context`: render everything we send to the model**
  (**complete** 2026-07-24). A debugging flag that prints the parts of
  the request the UI normally hides — the system prompt / preamble, every injected
  `<system-reminder>` (project instructions, and the skills listing once Task 32 lands),
  and the tool schemas — so "what did the model actually see?" is answerable without a
  proxy or a trace dump.
  - **Smaller than it looks: the data is already emitted.** `Event::Init` carries
    `preamble: Vec<Message>` and `tools: Vec<Value>`; injected reminders already ride
    `Event::Message`. The TUI simply drops them today (it reads `Init` for identity
    only). So this is a **UI-only** change — subscribe and render, gated by the flag —
    not a new engine seam or protocol variant. Scope S/M.
  - **Name (decided, user 2026-07-24): `--debug-show-hidden-context`.** The `debug-`
    prefix is deliberate — it groups this with future debugging switches and signals
    "not part of normal operation" at a glance; `show-hidden-context` then names exactly
    what it reveals, the parts of the request the UI hides. Deliberately **not**
    `--debug-ui`, which is too broad and reads like a flag for debugging the UI itself.
  - Headless already has this: `--output-format stream-json` emits `Init` verbatim. Worth
    saying so in `--help` rather than adding a second headless surface.
  - **Tool schemas print in full (decided, user 2026-07-24)** — no collapsing, no
    summarizing, no toggle. The flag exists for exactly one job: answering "what did the
    model actually see?", and a truncated schema cannot answer it. Verbosity is not a
    cost here because nothing turns this on except a debugging session.

### More harness packs + wires (the A/B bed)
Planning is a research task (AGENTS.md): re-read the harness source before starting each,
and resolve the plan's open-questions section first.

- [x] **Task 19 — codex pack** (`--harness codex`, **complete** 2026-07-24):
  faithful port of codex's stock duo **`shell_command` + freeform `apply_patch`** + the
  full **gpt-5.6-sol** base prompt + `<environment_context>` preamble; **openai-responses-only**;
  `update_plan` and unified-exec **not** ported (decisions locked). Entry point + resolved
  decisions + slice plan:
  [`../docs/codex-pack-dev-process.md`](../docs/codex-pack-dev-process.md); design detail
  [`plans/task-19-codex-pack.md`](plans/task-19-codex-pack.md) (reconciled; source re-pinned
  to codex `f201c30c`, 2026-07-24). Scope L, 3 slices — all shipped.
  - [x] **Slice 1** — pack scaffold + `shell_command` + minimal prompt + `--harness codex`
    ([`plans/task-19-slice-1-scaffold-shell.md`](plans/task-19-slice-1-scaffold-shell.md)).
  - [x] **Slice 2** — freeform `apply_patch` (V4A parser + 4-tier fuzzy apply +
    `Host::remove_file`/`create_dir`) + always-appended apply_patch instructions +
    openai-responses-only wire enforcement
    ([`plans/task-19-slice-2-apply-patch.md`](plans/task-19-slice-2-apply-patch.md)).
  - [x] **Slice 3** — full gpt-5.6-sol prompt + `strip_identity` + `<environment_context>` preamble
    ([`plans/task-19-slice-3-prompt.md`](plans/task-19-slice-3-prompt.md)).
- [ ] **grok pack fidelity gaps found by a live wire probe** (2026-07-24, decision
  pending — see [`../docs/research/harness-study-skills.md`](../docs/research/harness-study-skills.md)
  § *Live wire probe*). Grok Build 0.2.111 sends its shell tool as
  **`run_terminal_command`**, but the published source says `run_terminal_cmd`
  (`ToolId::new("run_terminal_cmd")`) and that is what our pack registers; live grok
  also ships a standalone **`write`** tool the pack lacks. Not fixed: the ported-pack
  workstream is closed, so whether to chase the shipped binary or stay pinned to the
  snapshot is a user call.
- [ ] **Task 15 — our own `locode` best-of pack** — **now P0.5, jointly with background
  tasks + subagents** (user decision, 2026-07-27 — see Priorities). The `opencode` half is
  **cancelled** (user decision, 2026-07-24); its plan
  ([`plans/task-15-opencode-pack.md`](plans/task-15-opencode-pack.md)) is kept as a record.
  `locode` is our best-of pack (grok-build-style naming; ADR-0011's rg-glob is scoped here)
  and the single home for post-skills capability — starting with the batch/background tool
  surface, which is why the two are built together rather than in sequence. Scope L.
- [ ] **Task 17 — OpenAI Chat Completions wire** (`openai-chat`) — **DEFERRED** (Responses
  covers GPT + Grok natively). The reasoning-blind LCD/control wire; revisit when a target
  model only speaks chat-completions. Plan: [`plans/task-17-openai-chat-wire.md`](plans/task-17-openai-chat-wire.md). Scope L.

### TUI backlog
- [ ] **Comprehensive wide-char wrapping upgrade** — the CJK truncation bug got a *surgical*
  display-width fix (`unicode-width`; 2026-07-23). The full upgrade adopts the codex/grok
  `textwrap` 0.16 + UAX#14 (`unicode-linebreak`) + grapheme-cluster stack for punctuation-aware
  breaks, ZWJ/emoji clusters, optimal-fit, and table clip-vs-wrap. Research + migration plan +
  TODO checklist: [`../docs/research/tui-text-wrapping-cjk.md`](../docs/research/tui-text-wrapping-cjk.md).
- [ ] **Markdown raw-markup depth (Phase 2.5)** — the `Event::Html` swallow-bug is fixed
  (#205), but three things both Rust twins do are still missing: `<br>` → line break,
  inline-vs-block newline semantics, and inline-HTML capture into table cells. That last
  one is a **prerequisite for the tables phase** — leaking raw HTML into a cell is the
  regression grok and codex each wrote tests for. Plan + citations:
  [`../docs/research/markdown-rendering-study.md`](../docs/research/markdown-rendering-study.md)
  § *Raw HTML/XML markup*.
- [ ] **OSC-8 hyperlinks** (P2) — clickable links (iTerm2 etc.).
- [ ] **Cumulative token usage + cost** (P2) — the footer now shows *context occupancy*
  (the last turn's `input + cache_read + cache_creation + output`), which is what a
  context meter needs; `Report.usage` still carries the run's **sum**, the right basis
  for a cost readout. Nothing accumulates that across runs yet, and no price table
  exists. A separate feature — deliberately not folded into the context number *(user
  decision, 2026-07-25)*.
  - **Prerequisite for resume:** the trace's `usage` record holds context occupancy, not
    a running total, so a cost readout needs its own record (or a re-read of every
    `usage` line, which only covers runs traced in this session's file).
- [ ] Custom slash-command files · plugins. *(Everything else once listed here has been
  promoted and is tracked above: background bash + subagents → **P0.5**; settings/trace →
  **Task 31**; skills → **Task 32**; shared AGENTS.md loading → shipped as Task 30.)*

### Deferred decisions (researched, not yet decided — don't lose track)
- [ ] **codex shell tool: `shell_command` now → unified exec later.** The codex pack ships
  `shell_command` (non-PTY, no background) as a substitution; codex's real mac/Linux default
  is now **unified exec** (`exec_command`/`write_stdin`, PTY/session/background). Switch when
  **background support (P0.5)** lands (see `docs/codex-pack-dev-process.md` D2). Comments in
  the codex shell tool flag it as deprecated.
- [ ] **Tool-description interface: static `&str` vs dynamic.** Our `Tool::description(&self)
  -> &str` (static, `include_str!`) matches opencode exactly and covers claude/grok/codex once
  the one runtime-model-dependent line is dropped (done — see AGENTS.md "Fidelity vs. truth").
  Analysis locked down in [`../docs/research/tool-description-interface.md`](../docs/research/tool-description-interface.md).
  **Re-open when:** a ported pack needs description text that must vary with run-time state we
  can't drop, or we want the Claude attribution line back with a truthful dynamic model name.
  Preferred path if forced: thread `PackContext` into `Pack::register` + store an owned `String`
  (not a `Tool::description` signature change).

### Tech debt
- ~~Collapse the `locode-exec` crate into `locode-tui`~~ — **rejected (user, 2026-07-23):**
  `locode-exec` stays a **standalone library** so headless-only consumers can depend on it (or
  `locode-core`) **without** pulling in the TUI. Its **binary target was removed** 2026-07-23
  (library only); the `locode-tui → locode-exec` *library* edge stays (ADR-0019 amendment).

---

## Archive — shipped

One line per task. Design detail is the matching file under [`plans/`](plans/) (see
[`plans/README.md`](plans/README.md)); rationale is in the cited ADRs.

### slash commands (Task 34 · autonomous 7-slice workstream, 2026-07-25)
- [x] **`/model` switching** — `/model <id>` swaps the provider on the live session and
  writes the id to `~/.locode/settings.json` (atomically) as the next session's default,
  matching both references, neither of which has a project-scoped model. The stale
  model line in a pack's preamble is corrected by an appended `<system-reminder>`, never
  by rewriting history (which would desync the trace). Decisions + source citations in
  [ADR-0026](../docs/decisions/ADR-0026-slash-commands-core.md) §6 amendment 2026-07-25b.
- [x] **Task 34** — slash commands, core contract + grok's dropdown. `SlashCommand` is a
  trait with a value-returning `CommandResult` (the reducer applies the effect; the loop
  owns the one awaiting step), so **every `user-invocable` skill is now reachable as
  `/<name>`** — the channel ADR-0025 left missing. UI: `nucleo-matcher` ranking with blue
  matched letters, a banded selected row with `❯`, an argument submenu from
  `suggest_args`, and both ghost-text mechanisms. Builtins: `/help` `/model` (read-only)
  `/new` `/quit`(+`/exit`). Decisions in
  [ADR-0026](../docs/decisions/ADR-0026-slash-commands-core.md) (§6 and §7 amended during
  implementation), study in
  [`../docs/research/harness-study-slash-commands.md`](../docs/research/harness-study-slash-commands.md),
  slice records in [`plans/task-34-slash-commands.md`](plans/task-34-slash-commands.md).
  PRs #211–#218.

### v0 core spine — Checkpoints A–D · **v0 complete 2026-07-18**
- [x] **Task 1** — Cargo workspace + crate skeletons + toolchain/lints (ADR-0002, ADR-0010).
- [x] **Task 2** — CI (`fmt · clippy · test · doc`) + justfile + strict-from-empty lints.
- [x] **Task 3** — `locode-protocol` types + report envelope (ADR-0009, ADR-0013).
- [x] **Task 3b** — `stream-json` `Event` types + `reconstruct_conversation` (ADR-0014).
- [x] **Task 4** — `locode-tools` `Tool` contract + registry + dispatch door (ADR-0003/0004/0008).
- [x] **Task 5** — `locode-provider` trait + `MockProvider` + `ToolCallAssembler` (ADR-0007).
- [x] **Task 6** — `locode-engine` sample→dispatch→append loop + `Session` API (ADR-0005).

### grok harness pack + host seam
- [x] **Task 7** — `locode-host` side-effect seam: path jail, shell, fs, truncation (ADR-0008).
- [x] **Task 8** — `locode-packs` framework + grok pack wiring (ADR-0012).
- [x] **Task 9** — grok pack: `run_terminal_cmd` + `read_file` (faithful mimicry).
- [x] **Task 10** — grok pack: `search_replace` (grok's real edit; no standalone `write`).
- [x] **Task 11** — grok pack: `grep` (ripgrep) + `list_dir` (grok's walker) (ADR-0011 amend).
- [x] **Task 26** — grok pack schema fidelity: restored the Task 11 "v0 seam" cuts — `grep`,
  `read_file`, `search_replace`, `run_terminal_cmd`, `list_dir` all faithful (per-tool audits
  in [`audits/`](audits/)); type-strict arg decoding. Release 0.1.3. Deferred tiers (multimodal
  `read_file`, background commands) → Deferred below.

### claude harness pack (Task 20 · autonomous 7-slice workstream, 2026-07-24)
- [x] **Task 20** — claude pack (`--harness claude`): a faithful port of Claude Code's six
  headless-relevant tools (`Bash`/`Read`/`Edit`/`Write`/`Glob`/`Grep` — real UpperCamelCase
  wire names, verbatim schemas/descriptions/caps) + the **read-before-edit / staleness gate**
  (`ClaudeSessionState`, CC's signature guardrail, absent in grok) + the **byte-exact static
  system prompt** (all D7 sections + env block + `currentDate` first-turn reminder). Wire-
  independent. Framework additions: `Pack::shape_user_prompt`; `PackContext` grew
  `is_git_repo`/`model`/`os_version`. Process + resolved decisions:
  [`../docs/claude-pack-dev-process.md`](../docs/claude-pack-dev-process.md); design detail
  [`plans/task-20-claude-pack.md`](plans/task-20-claude-pack.md) + per-slice records
  `plans/task-20-slice-{1..7}-*.md` (ADR-0012 amendment 2026-07-24). PRs #156–#162.

### Live wires + facade
- [x] **Task 12** — Anthropic Messages wire (ADR-0007): caching, two-tier retry, pairing.
- [x] **Task 13** — grok pack system prompt (MiniJinja, byte-pinned from source).
- [x] **Task 14** — `locode` facade + `locode-exec` headless binary (ADR-0009). *(`bundle-rg` deferred.)*
- [x] **Task 18** — OpenAI Responses wire (`openai-responses`; stateless, freeform tools,
  encrypted-reasoning replay, transport hoist) — 2026-07-19.
- [x] **Task 22** — custom provider injection: `ProviderRegistry` + lib-entry `locode-exec` (ADR-0015).
- [x] ~~**Task 16** — first A/B run~~ — REMOVED (A/B is just binary usage once packs/wires exist).
- [x] ~~**Task 21** — graceful SIGTERM in `locode-exec`~~ — folded into Task 24, delivered 2026-07-21.

### TUI core prerequisites (the 0.1.4 seams)
- [x] **Task 23** — session continuity (ADR-0016).
- [x] **Task 24** — cancellation + `cancelled` status + SIGTERM (ADR-0018; delivers Task 21).
- [x] **Task 25** — approval seam (ADR-0017).

### TUI + streaming
- [x] **Task 27** — `locode-tui` + `locode-app`: interactive frontend, **slices 1–9 + polish**
  (ADR-0019 architecture; ADR-0020 code highlighting; ADR-0022 vendored terminal / dynamic
  composer; shaded prompt band; box-drawing tables; two-row corner footer).
- [x] **Task 28** — unified `locode` binary: `-p` headless mode; the standalone `locode-exec`
  *binary* retired 2026-07-22 (releases ship only `locode`) and its trivial binary **target
  removed** 2026-07-23; the `locode-exec` crate remains a standalone headless library.
- [x] **Task 29** — live token streaming (ADR-0021): `Provider::stream` + `Event::MessageDelta`,
  Anthropic + Responses SSE (byte-identical assembly), TUI live cell with incremental markdown,
  headless `--stream`, whole-message trace preserved. Complete 2026-07-22.

### Shared context machinery
- [x] **Task 30** — shared `AGENTS.md` project-instruction loading (ADR-0023), **6 slices,
  PRs #146–150, complete 2026-07-23**: one shared loader in `locode-host` (cwd→git-root walk,
  `AGENTS.override.md` per-dir override, global `~/.locode/AGENTS.md`, canonical dedup +
  gitignore, reads bypass the tool jail), engine-side `User`-role `<system-reminder>` injection
  with a 64 KiB budget and per-turn rescan/replace-remove banners, default-on with
  `--no-project-instructions`. **Deferred seams (recorded, not built):** `--add-dir`/`extra_roots`
  (needs tool-jail widening, ADR-0008) and `root_stop_pattern` (needed `settings.json`; **now
  scheduled** — ADR-0024 §1.4, Task 31 S2, with the `regex` dep approved). The global file
  honors `$LOCODE_HOME` since the ADR-0023 amendment 2026-07-24. Plan + Result:
  [`plans/task-30-agents-md-project-instructions.md`](plans/task-30-agents-md-project-instructions.md).

---

## Deferred — reserved seams, not scheduled

parallel tool batches (RwLock read/write) · compaction (trace-side `compacted` record already
reserved, ADR-0024 §2.3) · OS sandbox · MCP · `--json-schema` answers · multi-platform `rg`
bundle matrix + macOS notarization (packaging, ADR-0011) · codex unified exec (PTY) /
`view_image` / hosted `web_search` · Claude Code TodoWrite/Task/WebFetch/WebSearch/NotebookEdit ·
per-model codex prompt variants · grok pack multimodal `read_file` (binary/image/PDF/PPTX) ·
grok pack background commands (`is_background` + `get_task_output`/`kill_task` — the *grok
skin* only; the **host task registry under it is no longer deferred**, it is P0.5 shared
machinery as of 2026-07-27) · `history.jsonl` input recall + trace GC (`cleanup_period_days`) + a rebuildable
sessions listing index (all reserved by ADR-0024; land on demand).

*(Removed 2026-07-24 as no longer deferred: "shared engine session-start context / AGENTS.md
loading" — shipped as Task 30; "JSONL session durability" — now scheduled as Task 31, ADR-0024.)*
