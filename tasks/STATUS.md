# Repo status & handoff — as of Task 14 / **Checkpoint D: v0 complete** (2026-07-18)

> **ADRs (and SPEC) are the intended source of truth and should stay trustworthy** — going
> forward, reconcile them *before* changing code (AGENTS.md "ADR-first"). This doc exists
> because earlier in development some decisions changed in code first, leaving ADR-vs-code
> drift; those ADRs have now been amended (0002/0003/0007/0008/0009/0011). Where any
> residual gap remains, **the merged code is the tie-breaker for current state**, and this
> file records the deltas. Written before a fresh session for **Task 12** (the Anthropic wire).

## Where we are

Phases 0–2 complete (Checkpoints A/B/C) **plus Task 12 — the live Anthropic wire,
smoke-tested end-to-end against OpenRouter**. Tasks 13–14 remain in Phase 3. Every
merged crate passes the full gate (`fmt` · `clippy --all-targets --all-features -D warnings`
· `test` · `doc` with `RUSTDOCFLAGS=-D warnings`). CI is green on `main`.

| Crate | State | What's in it | Tests |
|---|---|---|---|
| `locode-protocol` | done | 4-role conversation model; report envelope (`schema_version:1`; wire-id field is **`api_schema`**, not `provider`); `stream-json` `Event` + `reconstruct_conversation`; `ToolSpec`; `Usage` (+`AddAssign`) | 7 |
| `locode-tools` | done | `Tool` (schemars-derived args), `ToolKind`(+`Other`), `ToolError{Respond,Fatal}`, `ToolCtx`, `DynTool` + `TypedTool` adapter, `Registry` (`register`/`register_dyn`/`dispatch`/`specs`), `ToolSpec` re-export | 8 |
| `locode-provider` | done | `Provider` (`api_schema`+`complete`), `ConversationRequest`, `SamplingArgs`, `Completion`(`Vec<ContentBlock>`), `StopReason`(`#[non_exhaustive]`), `ProviderError`(exhaustive+`retryable`; `Api` retryable = 408/409/5xx), `MockProvider`, `ToolCallAssembler`, `repair_pairing`; **`AnthropicProvider`** (Task 12): wire DTOs (+`is_error`, +`RedactedThinking`), `ModelConfig`/`ApiBackend{Native,OpenRouter,Proxy}`, build (system hoist, 2-marker `cache_control` +≤4 assert, temp-omit, `reasoning_effort`→budget w/ interleaved waiver, `$schema`-stripped tool schemas), parse (verbatim ids, signatures, `Unknown` stop catch-all, empty-overflow→terminal), classify+retry (429 cap-2 surfaced, `Retry-After`, `x-should-retry:false`), client (beta mirroring, prefs injection), 401 refresh-once seam | 60 |
| `locode-engine` | done | `Session` + the loop (4 terminals, mid-batch abort synthesis, thinking replay, `stream-json` events); `run()` **infallible** → `Report` | 10 |
| `locode-host` | done | `Host` + `PathPolicy{Jailed,Unrestricted}`; shell `exec` (`bash -lc`, timeout, tail byte-cap, `unsafe`-free group-kill via `nix`, cancel); `run_capture` (argv); `read_dir`; `read_file`/`write_file`/`stat`; `truncate_for_model`; `rg_program` | 15 |
| `locode-packs` | done | `Pack` (`name`/`register(&Arc<Host>,…)`/`preamble→Vec<Message>`/`build_registry`), `resolve`/`available`, `GrokPack` with 5 real tools + **the real grok prompt** (Task 13): verbatim template copy (provenance-pinned) rendered via minijinja custom `${{ }}` syntax, `preamble = [System(prompt), User(<user_info>)]`, `strip_identity` knob (default faithful), `user_query()` for Task 14 | 34 |
| `locode` (facade) | done | curated re-exports: the driving API (`Session`/`EngineConfig`/sinks, packs, providers, host) + the full tool surface (SPEC Users #4) | – |
| `locode-exec` (binary) | done | positional prompt (stdin via `-`), `--cwd` (canonicalized → jail/engine/pack agree), `--harness`, `--api-schema {anthropic,mock}` (+env), `--max-turns` (unlimited default), `--output-format {json,text,stream-json}`, `--yolo`, `--strip-identity`; stdout discipline (audited writers, EPIPE-safe), tracing→stderr, ADR-0009 exit codes | 10 |

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

1. ~~`truncate_for_model` is not wired~~ **RESOLVED (Task 14, ADR-0008 amendment):** moved to `locode-tools` and applied centrally inside `Registry::dispatch` (the door) on both ok and error results; `HostConfig.model_output_budget` removed. Original text: **`truncate_for_model` is not wired anywhere yet.** It exists in `locode-host` but nothing applies it — `locode-engine/src/run.rs` has a `// TODO(Task 7/9)` where it should run post-dispatch, and the engine **doesn't even depend on `locode-host`/`locode-packs`** (removed in Task 6). Today only each tool's own cap applies, not the shared middle-truncation. **Decide at Task 14:** the facade constructs the `Host` and either (a) the engine gains a `locode-host` dep + a budget field and truncates `tool_result` text chunks, or (b) the facade wraps the registry. This is the one real cross-crate wiring gap.
2. ~~Tool-schema cross-API compatibility~~ **RESOLVED (Task 12 spike + live smoke):** our flat `Args` structs emit no `$defs`/`$ref`; `parameters_schema()` now inlines subschemas at the source (grok's `generate_schema` precedent) and the wire strips only the top-level `$schema`. Live tool calls worked against the real API.
3. **`ToolKind::Glob` tags `list_dir`** — a semantic stretch (it's a directory walk, not a glob). For honest cross-pack A/B alignment, consider adding a `ListDir`/`Dir` kind, or accept `Glob`. Low priority.
4. **`Fatal`-on-output-serialize** (`locode-tools/src/registry.rs`, `TypedTool::call`) is untested and arguably too harsh — a serialize failure could be `Respond`. Realistically unreachable, but flagged.
5. ~~Wire config record~~ **BUILT (Task 12):** `ModelConfig` with env `LOCODE_BASE_URL`/`LOCODE_API_KEY`/`LOCODE_MODEL` (default model `claude-sonnet-5`), `ApiBackend` auto-detection incl. OpenRouter, betas defaulting to interleaved thinking, `extra_headers`/`provider_prefs`. `LOCODE_API_SCHEMA` + `--api-schema` land with the exec (Task 14).
6. **No end-to-end integration test** composes engine+packs+provider yet. It lands at Task 14 (mock in CI, real against Claude → Checkpoint D). The engine's own tests use trivial tools; the packs' tests use `dispatch` directly.
7. ~~Jail root vs cwd~~ **RESOLVED (Task 14):** `locode-exec` canonicalizes `--cwd` once and hands the same canonical path to host/engine/pack. Original text: **Jail root vs cwd must agree.** `Host` canonicalizes `workspace_root`; the caller **must** set `EngineConfig.cwd`/`ToolCtx.cwd` to the same canonical path (on macOS `/var` → `/private/var`), or the jail rejects. Bake this into the Task 14 exec wiring (canonicalize the CLI `--cwd` and build the `Host` from it).
8. **`read_file` double-resolves** the path (`resolve_in_jail` then `read_file`, which resolves again) — minor; could thread the resolved path through `FileRead`.
9. **Where does faithful mimicry stop? (user-raised, 2026-07-18 — defer, but weigh at every pack/milestone decision.)** Harnesses diverge beyond tools + system prompt: each has its own **runtime context-injection machinery** (grok: `<user_info>` prefix, AGENTS.md `<system-reminder>`s, date-rollover reminders, TodoGate; Claude Code: mid-conversation system surfaces, its own reminder set) and ultimately **its own agent loop** (compaction triggers, reminder scheduling, queued-message handling). Mimicking those per pack would eventually mean per-harness loop variants — an extreme complexity cost against ADR-0005's single loop and the "no second loop" boundary. **No decision yet.** For now packs faithfully reproduce tools + prompts + static preamble (Tasks 13/15); loop-adjacent behaviors (reminders, injection cadence, compaction policy) stay OUT of the fidelity contract and on the one shared engine. When the A/B evidence shows loop-adjacent divergence actually matters, that is the moment to decide (likely a pack-owned "turn hooks" seam vs. accepting the shared loop as a controlled variable) — write the ADR then, not now.

## Next milestone starting point (post-v0)

v0 is done — every crate is real, the binary runs end-to-end against Claude
(via OpenRouter), and CI runs the keyless `--api-schema mock` path. What remains
deferred: `bundle-rg` packaging (ADR-0011), streaming, compaction, parallel
dispatch, OS sandbox, MCP, `--json-schema` (see `tasks/todo.md` Deferred).

Next (reprioritized 2026-07-18/19; **plans written 2026-07-19**): implementation
order **Task 18 → Task 19 → Task 20**; **Task 17 (Chat Completions) is DEFERRED**
(Responses covers GPT + Grok natively — enough for the native-pair evals; its plan
stays on file). Each task has a full source-grounded plan doc (read the plan + its
addenda before starting):

1. **Task 18 — OpenAI Responses wire** —
   [`plans/task-18-openai-responses-wire.md`](plans/task-18-openai-responses-wire.md).
   Stateless `store:false`; drives BOTH OpenAI models (incl. the freeform+grammar
   `apply_patch` codex needs) and xAI grok models (function tools +
   `encrypted_content` reasoning replay) — all verified live through OpenRouter's
   beta `/v1/responses` (plan §0 probe log; xAI 422s `custom` tools → degradation
   knob). Owns the ask-first `ToolSpec`/`ToolInputFormat` protocol change and the
   shared-transport hoist (`locode-provider::http`).
2. *(deferred)* **Task 17 — OpenAI Chat Completions wire** —
   [`plans/task-17-openai-chat-wire.md`](plans/task-17-openai-chat-wire.md).
   Revisit when a target model/provider only speaks chat completions.
3. **Task 19 — codex pack** —
   [`plans/task-19-codex-pack.md`](plans/task-19-codex-pack.md). `shell_command` +
   freeform `apply_patch` (shared parser module) + `update_plan`; native delivery
   via Task 18, `{input: string}` degradation elsewhere.
4. **Task 20 — claude pack** —
   [`plans/task-20-claude-pack.md`](plans/task-20-claude-pack.md). Six core tools
   (`Bash`/`Read`/`Edit`/`Write`/`Glob`/`Grep`) with the read-before-edit
   freshness gate + the static system prompt; wire-independent.

Task 16 was removed — A/B runs are plain binary usage. Task 15 is re-scoped to
the remaining packs (opencode + our own `locode`). The fidelity boundary
(concern #9 above) governed both pack plans: packs reproduce tools + prompts +
static preamble; loop-adjacent behaviors (TodoWrite reminders, CLAUDE.md/
AGENTS.md injection, git-status snapshots) stay on the shared engine until A/B
evidence forces the ADR — each plan lists its resulting exclusions explicitly.
