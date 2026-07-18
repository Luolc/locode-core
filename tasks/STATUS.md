# Repo status & handoff — as of Task 11 / Checkpoint C (2026-07-18)

> **The merged code is the only source of truth for current state.** Plans, ADRs, and the
> design doc may be legacy/outdated — that's fine. This file + the code reconcile them.
> When a plan/ADR disagrees with the code, the code wins; this doc records the deltas.
> Written before a fresh session for **Task 12** (the Anthropic wire).

## Where we are

Phases 0–2 complete; Checkpoints A/B/C reached. Phase 3 (Tasks 12–14) remains. Every
merged crate passes the full gate (`fmt` · `clippy --all-targets --all-features -D warnings`
· `test` · `doc` with `RUSTDOCFLAGS=-D warnings`). CI is green on `main`.

| Crate | State | What's in it | Tests |
|---|---|---|---|
| `locode-protocol` | done | 4-role conversation model; report envelope (`schema_version:1`; wire-id field is **`api_schema`**, not `provider`); `stream-json` `Event` + `reconstruct_conversation`; `ToolSpec`; `Usage` (+`AddAssign`) | 7 |
| `locode-tools` | done | `Tool` (schemars-derived args), `ToolKind`(+`Other`), `ToolError{Respond,Fatal}`, `ToolCtx`, `DynTool` + `TypedTool` adapter, `Registry` (`register`/`register_dyn`/`dispatch`/`specs`), `ToolSpec` re-export | 8 |
| `locode-provider` | done | `Provider` (`api_schema`+`complete`), `ConversationRequest`, `SamplingArgs`, `Completion`(`Vec<ContentBlock>`), `StopReason`(`#[non_exhaustive]`), `ProviderError`(exhaustive+`retryable`), `MockProvider`, `ToolCallAssembler`, `repair_pairing` | 15 |
| `locode-engine` | done | `Session` + the loop (4 terminals, mid-batch abort synthesis, thinking replay, `stream-json` events); `run()` **infallible** → `Report` | 10 |
| `locode-host` | done | `Host` + `PathPolicy{Jailed,Unrestricted}`; shell `exec` (`bash -lc`, timeout, tail byte-cap, `unsafe`-free group-kill via `nix`, cancel); `run_capture` (argv); `read_dir`; `read_file`/`write_file`/`stat`; `truncate_for_model`; `rg_program` | 15 |
| `locode-packs` | done | `Pack` (`name`/`register(&Arc<Host>,…)`/`preamble→Vec<Message>`/`build_registry`), `resolve`/`available`, `GrokPack` with 5 real tools | 23 |
| `locode` (facade) | **SKELETON** | Task 14 | – |
| `locode-exec` (binary) | **SKELETON** | Task 14 | – |

## Decisions of record made during development (authoritative)

Consolidated; full context in `tasks/plans/README.md` ("Interview decisions", "RESOLVED")
and the ADR amendments (0002/0008/0009/0011).

- **Faithful mimicry** (AGENTS.md): a ported pack reproduces its harness's real tools; custom choices apply only to our own `locode` pack.
- **Path jail = configurable `PathPolicy`** (default `Jailed`) with a `--dangerously-skip-permissions` / `--yolo` opt-out (ADR-0008 amendment). Shell is never path-jailed; its timeout/output caps stay on under `--yolo`.
- **`provider` → `api_schema`** rename across report envelope + `Event::Init` + ADR-0009 + golden. Trait method `api_schema()`. CLI flag will be `--api-schema` (Task 14).
- **`repair_pairing` lives in `locode-provider`** (ADR-0004); `reconstruct_conversation` in protocol. **No `locode-transcript` crate.**
- **`Completion` carries `Vec<ContentBlock>`** (thinking + signature preserved), not `text`+`tool_calls`.
- **Pack prompt = `preamble()->Vec<Message>`** (role-tagged), not `system_prompt()->String`. grok's System-vs-Developer/User split is **deferred to Task 13** (grok has no Developer role — base = `System` item, injected context = `User` `<system-reminder>`s).
- **grok pack = 5 real tools**: `run_terminal_cmd`, `read_file`, `search_replace`, `grep`, `list_dir`. **No `write`** (grok has none; empty `old_string` creates). **No `glob`**; `list_dir` = self-implemented walk; `grep` = ripgrep.
- **No mtime/freshness store** (grok has none — faithful mimicry). SPEC's "four edit invariants" reduce to grok's two runtime guards.
- **Tool Args field descriptions use `#[schemars(description = "…")]`** (verbatim from the harness), not `///`.
- Workspace lints: clippy `pedantic` + `print_stdout`/`print_stderr` deny. Shell default `bash -lc` (login, configurable); 10s timeout; middle-truncation for the shared post-process.
- **Deferred, all confirmed:** structured-output/`--json-schema`, cost/`total_cost_usd`, streaming, parallel tool dispatch, OS sandbox, MCP.

## Deviations from the plans (faithfulness gaps — code is truth)

- `search_replace`: exact **byte** matching (grok's Unicode-normalization matching deferred).
- `read_file`: 1000-line + 25k-token caps; **positive `offset` only** (negatives rejected); no PDF/image.
- `run_terminal_cmd`: foreground only (`is_background` dropped); combined stdout+stderr; host 30k byte cap (grok's ~20k, per-mode).
- `grep`: `pattern`/`path`/`glob`/`case_insensitive` only (context/type/multiline/output-mode + `head_limit` dropped); host 30k output cap (grok's 5MB).
- `list_dir`: self-walk, **no `.gitignore` filter**, simpler budget than grok's seed+deep-walk; 10k char cap.

## ⚠️ OPEN CONCERNS / gaps for the next session

1. **`truncate_for_model` is not wired anywhere yet.** It exists in `locode-host` but nothing applies it — `locode-engine/src/run.rs` has a `// TODO(Task 7/9)` where it should run post-dispatch, and the engine **doesn't even depend on `locode-host`/`locode-packs`** (removed in Task 6). Today only each tool's own cap applies, not the shared middle-truncation. **Decide at Task 14:** the facade constructs the `Host` and either (a) the engine gains a `locode-host` dep + a budget field and truncates `tool_result` text chunks, or (b) the facade wraps the registry. This is the one real cross-crate wiring gap.
2. **Tool-schema cross-API compatibility is UNVERIFIED — check FIRST at Task 12.** `Registry::specs()` emits schemars **draft-2020-12** (`$defs`/`$ref`/`$schema`). Decided: keep `specs()` + a **shared normalization helper**, assuming Anthropic/OpenAI accept the same schema. **Verify against the real Anthropic `input_schema` before relying on it**; add the helper where the schema is shaped for the wire.
3. **`ToolKind::Glob` tags `list_dir`** — a semantic stretch (it's a directory walk, not a glob). For honest cross-pack A/B alignment, consider adding a `ListDir`/`Dir` kind, or accept `Glob`. Low priority.
4. **`Fatal`-on-output-serialize** (`locode-tools/src/registry.rs`, `TypedTool::call`) is untested and arguably too harsh — a serialize failure could be `Respond`. Realistically unreachable, but flagged.
5. **Wire config record `{api_schema, base_url, api_key, model}`** is designed but unbuilt (Task 12/14) — env `LOCODE_API_SCHEMA`/`LOCODE_BASE_URL`/`LOCODE_API_KEY` + `--api-schema`, growable to per-model `{extra_headers, auth}`.
6. **No end-to-end integration test** composes engine+packs+provider yet. It lands at Task 14 (mock in CI, real against Claude → Checkpoint D). The engine's own tests use trivial tools; the packs' tests use `dispatch` directly.
7. **Jail root vs cwd must agree.** `Host` canonicalizes `workspace_root`; the caller **must** set `EngineConfig.cwd`/`ToolCtx.cwd` to the same canonical path (on macOS `/var` → `/private/var`), or the jail rejects. Bake this into the Task 14 exec wiring (canonicalize the CLI `--cwd` and build the `Host` from it).
8. **`read_file` double-resolves** the path (`resolve_in_jail` then `read_file`, which resolves again) — minor; could thread the resolved path through `FileRead`.

## Task 12 starting point

`tasks/plans/task-12-anthropic-wire.md` (+ its banner) is the design. Build the Anthropic
Messages wire: request build (hoist leading `System` → top-level `system`; `cache_control`
≤4 on system + 1 on last message with a count assert; omit temperature when thinking on;
`reasoning_effort`→`budget_tokens`), parse → `Completion` (preserve tool_use ids verbatim,
`Thinking{signature}`, `usage`), **two-tier retry** (transport tier here honoring
`Retry-After`; the bounded loop-resample tier is already in the engine), **429 surfaced**,
context-overflow/quota terminal, 401 refresh-once, call `repair_pairing` before send, the
modest config record. **Do concern #2 (schema compat) first.** New dep: `reqwest` (rustls) —
ask-first, get approval.
