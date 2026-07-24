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

Current release: **0.1.8**. Sizes: XS = 1 file · S = 1–2 · M = 3–5 · L = 5–8 (split if larger).

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

### More harness packs + wires (the A/B bed)
Planning is a research task (AGENTS.md): re-read the harness source before starting each,
and resolve the plan's open-questions section first.

- [ ] **Task 19 — codex pack** (`--harness codex`): `shell_command` + freeform `apply_patch`
  + `update_plan` + base prompt; shared `apply_patch` parser. Native delivery uses Task 18
  (done); degrades via `{input: string}` elsewhere. Plan:
  [`plans/task-19-codex-pack.md`](plans/task-19-codex-pack.md). Scope L.
- [ ] **Task 20 — claude pack** (`--harness claude`): Bash/Read/Edit/Write/Glob/Grep with
  verbatim schemas + the read-before-edit / modified-since-read freshness gate + static
  prompt. No wire dependency. Plan: [`plans/task-20-claude-pack.md`](plans/task-20-claude-pack.md). Scope L.
- [ ] **Task 15 — remaining packs: `opencode` + our own `locode`** (after 19/20). `opencode`
  is a faithful port; `locode` is our best-of pack (grok-build-style naming; ADR-0011's
  rg-glob is scoped here). Plan from source when scheduled. Scope L (split per pack).
- [ ] **Task 17 — OpenAI Chat Completions wire** (`openai-chat`) — **DEFERRED** (Responses
  covers GPT + Grok natively). The reasoning-blind LCD/control wire; revisit when a target
  model only speaks chat-completions. Plan: [`plans/task-17-openai-chat-wire.md`](plans/task-17-openai-chat-wire.md). Scope L.

### TUI backlog
- [ ] **OSC-8 hyperlinks** (P2) — clickable links (iTerm2 etc.).
- [ ] **Built-in slash commands** — deferred pending a *holistic* design pass
  (discovery/registry, syntax, pure-UI vs. seam- or persistence-backed), not piecemeal.
  Current commands: `/new` `/quit` `/exit`.
- [ ] **`/model` switching** — blocked on two seams: a model-selection seam on the ADR-0015
  `ProviderRegistry`/factory (public surface → ask-first) and config-file persistence (a new
  XDG `~/.config/locode/` ADR). Read-only `/model` becomes Tier-A once the seam exists.

### Tier B/C future capability (short ADR, then mostly-autonomous)
- [ ] Background bash commands · custom slash-command files · subagents · plugins ·
  config-file persistence (XDG). *(Shared AGENTS.md loading promoted to Task 30, above.)*

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
