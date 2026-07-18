# Per-task implementation plans

Detailed, **source-grounded** plans, one file per task. Each was written by re-reading
the actual harness source in the `coding-cli-survey` submodules (per the AGENTS.md "read
the source before planning" rule) and cites concrete `file:line`. They expand
[`../todo.md`](../todo.md) from a checklist into a reviewable design.

Two kinds live here:
- **Retrospective (tasks 1, 3, 3b, 4, 5)** — written *after* the code merged, documenting
  what is built and **why**, grounded in source, with exhaustive open-questions sections
  as an interview surface. (The CI/`justfile` task 2 is intentionally not covered.)
- **Pre-implementation (tasks 6–14)** — written *before* code. Task 6 is now built (see
  its banner); 7–14 are still forward-looking drafts with open questions to sign off.

| Plan | Task | Kind |
|---|---|---|
| [task-01-workspace-scaffold.md](task-01-workspace-scaffold.md) | Cargo workspace + crate boundaries + toolchain/lints | retrospective |
| [task-03-protocol-conversation-report.md](task-03-protocol-conversation-report.md) | `locode-protocol` — conversation model + report envelope | retrospective |
| [task-03b-streaming-events.md](task-03b-streaming-events.md) | `locode-protocol` — `stream-json` events + reconstruction | retrospective |
| [task-04-tools-contract-registry.md](task-04-tools-contract-registry.md) | `locode-tools` — `Tool` contract + registry + dispatch | retrospective |
| [task-05-provider-mock.md](task-05-provider-mock.md) | `locode-provider` — trait + `Completion` + mock + repair | retrospective |
| [task-06-engine-loop.md](task-06-engine-loop.md) | `locode-engine` — the sample→dispatch→append loop + `Session` | built |
| [task-07-host.md](task-07-host.md) | `locode-host` — path jail, shell exec (timeout/caps), truncation | draft |
| [task-08-packs.md](task-08-packs.md) | `locode-packs` — pack framework + grok pack wiring | draft |
| [task-09-grok-read-terminal.md](task-09-grok-read-terminal.md) | grok `run_terminal_cmd` + `read_file` | draft |
| [task-10-grok-edit.md](task-10-grok-edit.md) | grok `search_replace` (edit invariants; no standalone `write`) | draft |
| [task-11-grok-search.md](task-11-grok-search.md) | grok `grep` + `glob` (ripgrep-backed) | draft |
| [task-12-anthropic-wire.md](task-12-anthropic-wire.md) | Anthropic Messages wire (the live `Provider`) | draft |
| [task-13-grok-prompt.md](task-13-grok-prompt.md) | grok pack system prompt (minijinja) | draft |
| [task-14-facade-exec.md](task-14-facade-exec.md) | `locode` facade + `locode-exec` binary | draft |

## Already resolved since these were written

- **`Completion` / `StopReason` / `ConversationRequest` shapes** — settled and shipped
  in Task 5 (`Completion` carries `Vec<ContentBlock>`; no `system` field on the
  request; `StopReason` is `#[non_exhaustive]` + `Unknown(String)`). Some plans list
  these as open — they are not.

## Cross-cutting decisions — RESOLVED

- **`repair_pairing` home → `locode-provider`.** Provider-layer concern (ADR-0004);
  the engine depends on provider, so it calls it each iteration. Landed in Task 6.
- **`provider` → `api_schema` rename → done** in the report envelope, `Event::Init`,
  ADR-0009, and the golden snapshot (Task 6). The CLI flag becomes `--api-schema` in
  Task 14. It names the wire *schema*, not a gateway.
- **`run_terminal_cmd`** — use grok's real name (the SPEC/todo `run_terminal_command`
  was a voice-input artifact, not a real discrepancy). Applies at Task 9.
- **Standalone `write` tool → skip in the grok pack.** Grok creates files via
  `search_replace` with empty `old_string`; a dedicated `write` is not grok's. Revisit
  when implementing other harness packs. Applies at Task 10.
- **Faithfully mimic Grok Build** for the grok pack tools' behavior/details — this
  governs the edit-invariant question (implement grok's real `search_replace`
  semantics). Applies at Tasks 9–11.

## Interview decisions (2026-07-18)

A working-through of the plans' open questions. Full context in the git history; the
load-bearing outcomes:

- **Pack faithfulness is a rule** (AGENTS.md): ported packs mimic their harness's real
  tools; custom choices apply only to our own `locode` pack. Consequence: the grok pack
  ships `list_dir` (grok's fs walker), **not** an rg-glob — ADR-0011's rg-glob is scoped to
  the `locode` pack (ADR-0011 amendment).
- **Edit invariants (grok pack): mimic grok** — runtime #2 (exact+unique) + #4 (reject
  no-op); #1 read-before-edit via contract; **no #3 mtime store** (grok has none). SPEC
  success-criterion 3 reworded.
- **`repair_pairing` stays in `locode-provider`; `reconstruct_conversation` stays in
  `locode-protocol`.** No `locode-transcript` crate.
- **Dispatch-door policy / OS sandbox → deferred post-v0.**
- **v0 scope holds:** structured-output/`--json-schema`, cost/`total_cost_usd`, streaming,
  parallel dispatch all deferred.
- **Facade goal:** `locode` re-exports the driving API **and the full tool surface** so
  downstream can use our tools in *their own* loop (SPEC Users #4).
- **Wire config (Task 12/14):** a modest extensible record `{api_schema, base_url, api_key,
  model}` (env + `--api-schema`), growable to per-model `{extra_headers, auth}`.
- **Tool schema:** assume APIs share one JSON Schema → keep `specs()` + a shared
  normalization helper; **verify Anthropic/OpenAI compatibility** before the wire relies
  on it (SPEC Open Q3).
- **Lints:** keep clippy `pedantic`; **added workspace `print_stdout`/`print_stderr`
  deny** (exec allows narrowly).
- **Host:** `nix` + dev-`tempfile` approved; shell `bash -lc` + configurable-shell seam;
  10s timeout (configurable); middle-truncation with marker.
- **grok prompt:** cwd/OS/shell/date as a `Role::Developer` message (keeps the
  system-prompt cache boundary).
