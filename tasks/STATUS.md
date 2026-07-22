# Repo status & handoff — snapshot 2026-07-22 (v0.1.5 released)

> **This is a dated snapshot, not a live tracker.** In this rapidly-moving repo the
> living sources of truth are, in order: the **ADRs** (`docs/decisions/`, the intended
> authority — reconcile them *before* code, AGENTS.md "ADR-first"), **`SPEC.md`** /
> **`SPEC-TUI.md`**, **`tasks/todo.md`** + the per-task **plan `Result` addenda**
> (`tasks/plans/*`), and finally **merged code as the tie-breaker** for current state.
> Refresh this file when a milestone lands; don't trust an old date.

## Where we are

The full agent is **shipped and installable**. `locode` = interactive TUI by default,
headless one-shot under `-p`. Installed via `install.sh` (macOS + Linux) or built from
source. All core crates published to crates.io at **0.1.5**; CI green on `main`.

### Core library (Tasks 1–26) — done
Eight crates, all published: `locode-protocol`, `locode-tools`, `locode-packs`,
`locode-provider`, `locode-host`, `locode-engine`, `locode-core` (facade), `locode-exec`.
- **Wires:** Anthropic (Task 12) + OpenAI-Responses (Task 18), both live-smoked via
  OpenRouter; `MockProvider` for keyless CI. Chat-Completions (Task 17) deferred.
- **Packs:** `grok` faithful to source (Task 26 fidelity audit). codex + claude + our
  own `locode` pack per their plans. Faithful-mimicry rule: a ported pack reproduces its
  harness's real tools/prompts/caps; custom choices apply only to the `locode` pack.
- **Interactive seams (0.1.4):** ADR-0016 session continuity, ADR-0017 approval seam,
  ADR-0018 cancellation (`Status::Cancelled`, exec SIGTERM) — the headless core stays
  headless; interaction reaches the engine only through these seams.

### TUI (Task 27, six slices; Task 28 unified binary) — done
- `locode-tui` (library) + `locode-app` (the `locode` binary), both `publish = false`.
  ratatui + crossterm, `scrolling-regions` feature (stock `insert_before`'s CPR query
  deadlocks the input thread; DECSTBM path avoids it). Inline viewport + print-once
  transcript, sans-IO `Msg → update → Cmd` reducer, dedicated input thread, one biased
  `select!` loop, zero idle wakeups.
- Working: runs, cancel (Esc/Ctrl+C), tool approvals (overlay + `--yolo`), queued
  prompts, prompt history, `/new` `/quit`, markdown rendering, error surfacing.
- Grounded in `docs/research/tui-harness-study.md`, `SPEC-TUI.md`, ADR-0019.
- **Task 28:** `-p`/`--print` headless reuses `locode_exec::run_headless`; a bare
  positional prompt pre-fills the composer.

### Release / installer
v0.1.5 GitHub Release; `install.sh` (fetched from `main` — script fixes go live on merge,
no re-tag) installs `locode` to `~/.locode/bin`, checksum-verified. As of 2026-07-22 the
release ships **only** `locode` — the old `locode-exec` binary is retired (ADR-0010 /
ADR-0019 amendments); `locode-exec` remains a published *library* crate.

## What's next (see `tasks/todo.md` for the live list)

Core-touching features come **off** the vibe-coding autopilot and get careful ADR-first
design (user decision, 2026-07-22): **streaming**, background bash commands, subagents,
skills, `AGENTS.md`/`CLAUDE.md` session-start loading, plugins, slash-commands. UI feel /
polish stays on the autonomous slice loop (`docs/tui-dev-process.md`), screenshot-driven,
mimicking the reference harnesses.

Near-term housekeeping: markdown rendering upgrade (research in
`docs/research/`); collapse the `locode-exec` crate into `locode-tui`/a shared lib and drop
the `locode-tui → locode-exec` edge (mechanical, ADR-0019 follow-up).

## Standing open concern (weigh at every pack/milestone decision)

**Where does faithful mimicry stop?** (user-raised 2026-07-18, still open.) Harnesses
diverge beyond tools + system prompt: each has its own runtime context-injection machinery
and ultimately its own agent-loop policy (compaction, reminder scheduling, queued-message
handling). Mimicking those per pack would mean per-harness loop variants — an extreme cost
against ADR-0005's single loop and the "no second loop" boundary. **For now:** packs
reproduce tools + prompts + static preamble; loop-adjacent behavior stays on the one shared
engine as a controlled variable. Decide (likely a pack-owned "turn hooks" seam vs. accepting
the shared loop) when A/B evidence shows it actually matters — write the ADR then.

Deferred seams (not scheduled): parallel tool batches, compaction, OS sandbox, MCP,
streaming events (SSE seams reserved per wire), `--json-schema` answers, JSONL session
durability, multi-platform `rg` bundle matrix, per-pack multimodal `read_file`, background
commands. Full list in `tasks/todo.md` → Deferred.
