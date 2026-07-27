# The codex-pack development process — autonomous slice loop

**This is the entry point for the Codex pack workstream (Task 19).** It is written
to be **self-contained**: a fresh agent context should be able to start here, read
the linked grounding docs, and execute the whole workstream without re-deriving the
decisions below. Mode: **near-fully autonomous** — the loop, gates, and hard stops
are the shared ones in [`autonomous-workflow.md`](autonomous-workflow.md); read that
first, then this. "Self-contained" means the *decisions* below are complete, not that
the loop is restated here.

**What this builds:** `--harness codex`, a **faithful port of Codex CLI's stock
headless tool surface + the gpt-5.6-sol base prompt + static preamble** — the third
studied-harness pack (after grok, claude). Codex is the **minimal-tool extreme**:
**no read/grep/glob/write/edit tools at all** — the shell is the read path and **all
editing goes through one patch-format tool**. Comparing that against grok's 5, claude's
6, and opencode's 7 is exactly the tool-surface A/B this repo exists to run.

> **The one rule that overrides everything: faithful reproduction, subject to truth
> (AGENTS.md "Fidelity vs. truth").** Reproduce codex's real behavior — names, arg
> schemas, verbatim descriptions, caps, guardrails, prompt bytes, output framing —
> from the source. **No unmotivated omissions, no extra features.** Deviate only where
> a decision below says so.

---

## Grounding documents (authority order)

1. **Accepted ADRs** — [`ADR-0012`](decisions/ADR-0012-harness-packs.md) (harness
   packs: fidelity beats DRY), [`ADR-0023`](decisions/ADR-0023-fidelity-boundary-and-agents-md-loading.md)
   (fidelity boundary; AGENTS.md loaded by the **shared engine**, not the pack),
   [`ADR-0013`](decisions/ADR-0013-conversation-protocol.md) (4-role protocol),
   [`ADR-0003`](decisions/ADR-0003-typed-tool-contract.md) (typed `Tool` contract +
   the freeform `ToolInputFormat` amendment, Task 18), [`ADR-0008`](decisions/ADR-0008-dispatch-door-and-path-jail.md)
   (host seam + path jail + central truncation), the **OpenAI Responses wire**
   (Task 18, shipped — freeform custom-tool delivery).
2. **[`SPEC.md`](../SPEC.md)** — crate layout, boundaries.
3. **[`tasks/plans/task-19-codex-pack.md`](../tasks/plans/task-19-codex-pack.md)** —
   the per-tool/per-section design detail, **now reconciled** (its header points here;
   the 2026-07-24 re-survey at commit `f201c30c` supersedes several of its assumptions
   — unified-exec-by-default, apply_patch's deleted JSON variant, the new prompt).
4. **Survey** — `~/dev/coding-cli-survey/survey/02-codex/*` (somewhat stale post-
   re-survey; the two subagent re-survey reports are the current truth).
5. **[`tasks/tracker.md`](../tasks/tracker.md) Task 19** — the live status line.
6. **Repo rules — [`AGENTS.md`](../AGENTS.md)** apply unchanged (ADR-first, faithful-
   vs-custom boundary, **Fidelity vs. truth**, quality gate, git workflow, voice hygiene).

**The source of truth for behavior:** the codex source under
`~/dev/coding-cli-survey/submodules/codex/codex-rs` — **re-pinned to commit
`f201c30c52a35f819262865a53df94b6f4ea7a50` (2026-07-24)** (was `1d941253`, 325 commits
stale). Note the spec builders moved into the new `codex-rs/tools/` crate (`codex_tools`);
`core/src/tools/spec_plan.rs` remains the orchestrator. **Re-read per slice (Phase 1).**
The grok + claude packs are the template for tool ports, provenance-pinned prompts,
byte-pin tests, `strip_identity`, `Pack::shape_user_prompt`, the `is_git_repo`/`model`/
`os_version` `PackContext` fields, and `Host::create_dir`.

---

## Resolved decisions (interview 2026-07-24 — do not re-litigate)

### Scope & tools
- **D1 — Tool set = the duo `{shell_command, apply_patch}`.** `update_plan` is **not
  ported** (user: not needed — deferred entirely). Codex has **no** dedicated read/
  write/edit/grep/glob tool (confirmed at `f201c30c`: `handlers/` has no such handler;
  the base prompt's "edit" mentions are prose; file editing is `apply_patch`-only,
  reading/searching is via the shell). ToolKinds: `Shell`, plus a `Patch`/`Other` kind
  for apply_patch (align with the pack registry).
- **D2 — Shell tool = `shell_command` (non-PTY), marked deprecated in code comments.**
  At `f201c30c`, mac/Linux codex **defaults to unified exec** (`exec_command` +
  `write_stdin` — a stateful PTY/session tool) and hides `shell_command`. We expose
  **`shell_command`** anyway: it is gpt-5.6-sol's own declared `shell_type`, i.e. the
  visible tool with the unified-exec feature disabled (a real codex config). Reason:
  unified exec needs session/background infra out of current scope (consistent with the
  no-background stance; grok dropped background the same way). **Code comments must state
  plainly that this is a substitution and that the real non-Windows default is unified
  exec — to be switched when background support lands (next major iteration).** Tracked
  in `tasks/tracker.md`.

### apply_patch
- **D3 — `apply_patch` = pure freeform (Lark grammar).** At `f201c30c` the JSON
  `{input}` variant was **deleted** upstream (`ApplyPatchToolType` now only `Freeform`);
  combined with D5 (responses-only) this means **one shape**: a freeform custom tool,
  arg is the raw patch string (no untagged two-shape `Args`). Native delivery on the
  OpenAI Responses wire (Task 18). Description verbatim ("The `apply_patch` tool can be
  used to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.").
  Parser: the V4A envelope (`*** Begin/End Patch`, Add/Update/Delete + `*** Move to:`,
  `@@` anchors, `*** End of File`) + the 3-tier fuzzy ladder (exact → `trim_end` →
  `trim` both sides) — hand-ported from `apply-patch/src/{parser,seek_sequence}.rs`
  (grammar shipped as bytes only, no Lark runtime). **Add creates parent dirs** via
  `Host::create_dir` (codex: `create_directory(parent, recursive:true)`). Two-phase
  apply per source (follow-the-source, D9).
- **D4 — apply_patch instructions: always append.** The 3084-byte
  `prompts/templates/apply_patch_tool_instructions.md` is shipped verbatim and appended
  as a **second System text block** after the base prompt (user decision: we mainly test
  non-OpenAI models, and an extra usage block is at worst redundant for OpenAI models).

### Wire
- **D5 — openai-responses ONLY.** The codex pack requires the OpenAI Responses wire
  (apply_patch's native freeform delivery). Running it on another real wire
  (`anthropic`/`openai-chat`) is a **clear pre-run error**, not a silent degradation.
  Mechanism: a `Pack`-declared supported-schema set (default = all) — codex declares
  `{openai-responses}` (+ `mock` allowed for CI smoke); exec/tui check `--api-schema`
  pre-run and error on mismatch. A pack-framework + exec/tui addition (autonomous).

### Prompt & preamble
- **D6 — Base prompt = gpt-5.6-sol's `base_instructions`.** The newest flagship prompt
  (17730 chars, shared by the 5.6 sol/terra/luna variants), embedded in
  `models-manager/models.json` (not a `.md`). Ported verbatim into a
  provenance-pinned file (sha256 + commit + length). Opener: *"You are Codex, an agent
  based on GPT-5. You and the user share one workspace, and your job is to collaborate
  with them until their goal is genuinely handled."* The model-independent fallback
  (`models-manager/prompt.md`, the classic "You are a coding agent running in the Codex
  CLI…") is **not** used. **Truth-first: clean** — the re-survey confirms no date/model-
  version/host value that would be untrue for a non-codex run; only the static identity
  "based on GPT-5". `strip_identity` (default off) removes the opener identity sentence.
  The prompt references `exec_command`/`cmd` (×1) and a `# Using skills` section — tools/
  features not in our pool; **kept verbatim, logged as gaps** (D8-style, per claude).
- **D7 — Preamble = `[System(prompt + apply_patch instructions block), User(<environment_context>)]`.**
  Codex sends the base prompt as `instructions` (Responses wire; our System hoists
  there, Task 18) and `<environment_context>` as a **leading user input item**
  (`context/world_state/environment.rs`). The env block is rebuilt at `f201c30c`:
  `<cwd>`/`<shell>`/`<current_date>`/`<timezone>`/`<network …>`/`<filesystem>`
  (permission profiles) — the old flat `<approval_policy>`/`<sandbox_mode>`/`<os>` tags
  are gone. Render the fields `PackContext` supplies (cwd/shell/date); permission/network
  tags → our `PathPolicy` **jail substitution** (documented); timezone added to
  `PackContext` if cheap, else omitted. Follow-the-source for exact bytes (D9).

### Contract & mechanics
- **D8 — Approval/sandbox params dropped.** The shell tool's `sandbox_permissions` enum
  + `justification` + `prefix_rule`, and any `request_permissions` flow, are **not**
  ported (no interactive permission flow — ADR-0001; our jail substitutes). A schema
  promising permissions that do nothing is worse than absence — this is the pack's **top
  faithfulness gap**, logged. `#[serde(deny_unknown_fields)]` (codex
  `additionalProperties:false`); type-strict numeric/bool decoding (repo policy).
- **D9 — Follow the source for mechanical fidelity (pre-authorized, don't stop).**
  shell timeout default **10000 ms with no max clamp** (`params.timeout_ms.into()`);
  **kill on timeout**, exit code **124** (`EXEC_TIMEOUT_EXIT_CODE`); output framing
  verbatim (`Exit code: {n}\nWall time: {s} seconds\n[Total output lines: {n}\n]Output:\n{body}`;
  timeout body prefixed `command timed out after {ms} milliseconds\n{output}`); two-phase
  apply atomicity; parser tolerances; error strings; env-context exact fields. Cite the
  `codex_tools` crate (spec builders' new home).
- **D10 — `shape_user_prompt` = default (verbatim).** Codex sends the raw user prompt
  (no `<user_query>`-style wrapper). AGENTS.md/project instructions come from the
  **shared engine** (ADR-0023), not the pack; the shared loader's format ≠ codex's
  `<user_instructions>` wrapper — a documented fidelity gap.

---

## Gap log (accepted, documented fidelity gaps — keep current in the pack module docs)
- **Loop-adjacent (D2 boundary):** `update_plan` + plan reminders; the `code_mode`
  execute/wait wrapper (sol is `tool_mode=code_mode_only` — we port the underlying
  `shell_command`/`apply_patch`, not the wrapper); multi-agent/subagents; skills; MCP;
  compaction. On the shared engine, not the pack.
- **Infra-gated / deferred:** **unified exec** (`exec_command`/`write_stdin`, PTY/
  background/session) — deferred (D2; revisit with background support); `view_image`
  (multimodal); `web_search` (sol's `use_responses_lite` suppresses hosted specs anyway);
  the tree-sitter/permission machinery.
- **Substitutions:** path jail = our `PathPolicy` (ADR-0008), not codex's sandbox/
  approval matrix (D8, top gap); the prompt names `exec_command`/`cmd` (we expose
  `shell_command`) and a skills section (skills not in pool) — kept verbatim.
- **Wire:** openai-responses only (errors on other real wires, D5).

---

## The loop

Per [`autonomous-workflow.md`](autonomous-workflow.md) — five phases, the four-part
gate, same-PR bookkeeping. Local bindings: plan docs at
`tasks/plans/task-19-slice-N-<name>.md`, branches `feat/task-19-slice-N-<name>`,
Phase 1 revisits the pinned source commit named above, and Phase 4 flips the Task 19
checkbox.

## Slice plan (proposed — agent's call, revise per Phase 0)
1. **S1 — pack scaffold + `shell_command` + minimal prompt.** `CodexPack` + `Pack` impl
   + `register` + `resolve("codex")` + `--harness codex` through exec/tui; a minimal-but-
   real prompt (identity + a first section) so it runs against `--api-schema mock`.
   `shell_command` (the deprecated-substitution note per D2) with the verbatim schema
   (`command`/`workdir`/`timeout_ms`/`login`; approval params dropped, D8), 10k timeout /
   kill-124 / `Exit code:`+`Wall time:` framing (D9).
2. **S2 — `apply_patch` (freeform) + the responses-only wire requirement.** The freeform
   `ToolSpec` (Lark grammar bytes) + the hand-ported V4A parser + 3-tier fuzzy apply over
   the host (Add → `Host::create_dir`; two-phase); append the 3084-byte instructions
   block (D4); the `Pack` supported-schema declaration + exec/tui pre-run check (D5).
3. **S3 — full gpt-5.6-sol prompt + preamble + env_context.** The pinned 17730-byte
   prompt (D6) + `strip_identity` + the apply_patch instructions System block; preamble =
   `[System, User(<environment_context>)]` with the rebuilt env renderer (D7); any
   `PackContext` growth (timezone) + exec/tui plumbing. Byte-pin snapshots.

## Autonomy — local additions

The shared contract in [`autonomous-workflow.md`](autonomous-workflow.md) applies. What
is specific here: **everything inside `locode-packs`** (this pack's modules, parsers,
descriptions, prompt constants, snapshots), **pack-framework/`Pack`/`PackContext`
additions** plus the exec/tui plumbing that carries them, and all mechanical fidelity
details taken from the pinned source — the agent decides those alone and records them.

Beyond the six shared hard stops: **reopening a resolved decision below**, or expanding
scope past this pack's stated tool set and prompt.

## Standing constraints

Shared — see [`autonomous-workflow.md`](autonomous-workflow.md). The one worth repeating
for a *ported* pack: faithful mimicry wins over a repo default, subject to truth, and
every such call is noted explicitly in the module docs and the gap log below.

## First action after a context reset
Read this doc top-to-bottom, then the reconciled task-19 plan, then open the codex source
at `f201c30c` for **Slice 1** (the `codex_tools` shell spec builder for `shell_command`
+ `core/src/exec.rs` timeouts + `core/src/tools/mod.rs` output framing) and run Phase 0.
