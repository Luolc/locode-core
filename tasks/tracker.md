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

Current release: **0.1.9**. Sizes: XS = 1 file · S = 1–2 · M = 3–5 · L = 5–8 (split if larger).

## Architecture at a glance

```
locode-protocol   (pure types)
    ├── locode-host       (fs / shell / path-jail / truncation / rg)
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
- **In flight — codex pack (Task 19):** decisions locked (interview 2026-07-24),
  running **overnight-autonomous** under [`../docs/codex-pack-dev-process.md`](../docs/codex-pack-dev-process.md).
- **P0 — Skills.** Full skills implementation (discovery + invocation + the skill markdown
  loading). Ties into the home dotfolder below. Plan from source.
- **P0 — settings.json + trace persistence (continue/resume).** Persist config
  (`settings.json`) and the session trace to a **home dotfolder** — our analog of
  `~/.claude`, `~/.grok`, `~/.codex`, `~/.opencode`; our folder is **`~/.locode`**.
  Enables `--continue`/`--resume`. Research of what those harnesses store in their dotfolders
  is under way (subagent) → a research doc will land under `docs/research/`.
- **P0.5 — Background Bash + Subagents.** Background/async bash commands, and subagents /
  agent-groups (their distinctions + implementations are hard to read off the UIs — needs a
  source dig). After the two P0s.
- **P1 — opencode pack** (the faithful-port half of Task 15) — deferred; plan drafted
  ([`plans/task-15-opencode-pack.md`](plans/task-15-opencode-pack.md)), revisit later.
- **P1 — our own `locode` best-of pack** (the other half of Task 15).

### More harness packs + wires (the A/B bed)
Planning is a research task (AGENTS.md): re-read the harness source before starting each,
and resolve the plan's open-questions section first.

- [ ] **Task 19 — codex pack** (`--harness codex`, **in flight, overnight-autonomous**):
  faithful port of codex's stock duo **`shell_command` + freeform `apply_patch`** + the
  **gpt-5.6-sol** base prompt; **openai-responses-only**; `update_plan` and unified-exec
  **not** ported (decisions locked). Entry point + resolved decisions + slice plan:
  [`../docs/codex-pack-dev-process.md`](../docs/codex-pack-dev-process.md); design detail
  [`plans/task-19-codex-pack.md`](plans/task-19-codex-pack.md) (reconciled; source re-pinned
  to codex `f201c30c`, 2026-07-24). Scope L, ~3 slices.
  - [x] **Slice 1** — pack scaffold + `shell_command` + minimal prompt + `--harness codex`
    ([`plans/task-19-slice-1-scaffold-shell.md`](plans/task-19-slice-1-scaffold-shell.md)).
  - [x] **Slice 2** — freeform `apply_patch` (V4A parser + 4-tier fuzzy apply +
    `Host::remove_file`/`create_dir`) + always-appended apply_patch instructions +
    openai-responses-only wire enforcement
    ([`plans/task-19-slice-2-apply-patch.md`](plans/task-19-slice-2-apply-patch.md)).
  - [ ] **Slice 3** — full gpt-5.6-sol prompt + `strip_identity` + `<environment_context>` preamble.
- [ ] **Task 15 — remaining packs: `opencode` (P1) + our own `locode` (P1)**. `opencode`
  is a faithful port (plan drafted: [`plans/task-15-opencode-pack.md`](plans/task-15-opencode-pack.md));
  `locode` is our best-of pack (grok-build-style naming; ADR-0011's rg-glob is scoped here).
  Both deferred behind the P0s above. Scope L (split per pack).
- [ ] **Task 17 — OpenAI Chat Completions wire** (`openai-chat`) — **DEFERRED** (Responses
  covers GPT + Grok natively). The reasoning-blind LCD/control wire; revisit when a target
  model only speaks chat-completions. Plan: [`plans/task-17-openai-chat-wire.md`](plans/task-17-openai-chat-wire.md). Scope L.

### TUI backlog
- [ ] **Comprehensive wide-char wrapping upgrade** — the CJK truncation bug got a *surgical*
  display-width fix (`unicode-width`; 2026-07-23). The full upgrade adopts the codex/grok
  `textwrap` 0.16 + UAX#14 (`unicode-linebreak`) + grapheme-cluster stack for punctuation-aware
  breaks, ZWJ/emoji clusters, optimal-fit, and table clip-vs-wrap. Research + migration plan +
  TODO checklist: [`../docs/research/tui-text-wrapping-cjk.md`](../docs/research/tui-text-wrapping-cjk.md).
- [ ] **OSC-8 hyperlinks** (P2) — clickable links (iTerm2 etc.).
- [ ] **Built-in slash commands** — deferred pending a *holistic* design pass
  (discovery/registry, syntax, pure-UI vs. seam- or persistence-backed), not piecemeal.
  Current commands: `/new` `/quit` `/exit`.
- [ ] **`/model` switching** — blocked on two seams: a model-selection seam on the ADR-0015
  `ProviderRegistry`/factory (public surface → ask-first) and config-file persistence (a new
  XDG `~/.config/locode/` ADR). Read-only `/model` becomes Tier-A once the seam exists.

### Tier B/C future capability (short ADR, then mostly-autonomous)
- [ ] Custom slash-command files · plugins. *(Background bash + subagents promoted to
  **P0.5**; config-file/trace persistence + skills promoted to **P0** — see Priorities
  above. Shared AGENTS.md loading was promoted to Task 30.)*

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
  (needs tool-jail widening, ADR-0008) and `root_stop_pattern` (needs `settings.json`; would add
  `regex`). Plan + Result: [`plans/task-30-agents-md-project-instructions.md`](plans/task-30-agents-md-project-instructions.md).

---

## Deferred — reserved seams, not scheduled

parallel tool batches (RwLock read/write) · compaction · OS sandbox · MCP · `--json-schema`
answers · JSONL session durability · multi-platform `rg` bundle matrix + macOS notarization
(packaging, ADR-0011) · shared engine session-start context (AGENTS.md loading, ADR-0023) · codex
unified exec (PTY) / `view_image` / hosted `web_search` · Claude Code
TodoWrite/Task/WebFetch/WebSearch/NotebookEdit · per-model codex prompt variants · grok pack
multimodal `read_file` (binary/image/PDF/PPTX) · grok pack background commands (`is_background`
+ `get_task_output`/`kill_task` + host task registry).
