# locode-core v0 — Task List

Detailed, ordered tasks for [`plan.md`](plan.md). Each clears the Definition of Done in the plan.
Sizes: XS=1 file · S=1–2 · M=3–5 · L=5–8 (break down if larger).

> **📍 Current state, deviations from plans, and open concerns: [`STATUS.md`](STATUS.md).**
> Read it first — the merged code is the source of truth; plans/ADRs may be legacy. Tasks 1–14
> are done (**v0 complete**, Checkpoint D reached 2026-07-18).
>
> **Next milestone — implementation order (user decision 2026-07-18/19):**
> **Task 18 (OpenAI Responses wire) → Task 17 (OpenAI Chat wire) → Task 19 (codex pack)
> → Task 20 (claude pack).** Detailed source-grounded plans exist for all four:
> [`plans/task-18-openai-responses-wire.md`](plans/task-18-openai-responses-wire.md) ·
> [`plans/task-17-openai-chat-wire.md`](plans/task-17-openai-chat-wire.md) ·
> [`plans/task-19-codex-pack.md`](plans/task-19-codex-pack.md) ·
> [`plans/task-20-claude-pack.md`](plans/task-20-claude-pack.md) — read the plan (and its
> open questions) before starting each task. Task 15 is now only the remaining packs
> (opencode + our own `locode`).

---

## Phase 0: Scaffolding

## Task 1: Cargo workspace + crate skeletons + toolchain pin ✅ done
**Description:** Create the `locode-*` workspace with empty compiling crate skeletons and the pinned toolchain + lint configs (ADR-0002, ADR-0010).

**Acceptance criteria:**
- [x] `Cargo.toml` `[workspace]` lists all 8 crates under `crates/`; each crate compiles as an empty lib (`locode-exec` as a bin).
- [x] `rust-toolchain.toml` pins current stable (1.97.1) + `rustfmt`,`clippy`; `rustfmt.toml`, `clippy.toml`, `[workspace.lints]` (`unused_must_use="deny"`) present.
- [x] Dependency directions from the plan graph are wired (no cycles).

**Verification:**
- [x] `cargo build --workspace` succeeds; `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean.

**Dependencies:** None
**Files:** `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `crates/*/Cargo.toml`, `crates/*/src/lib.rs|main.rs`
**Scope:** M

## Task 2: CI + justfile ✅ done
**Description:** Single GitHub Actions job running the mandatory triangle + a docs gate, plus developer `justfile` and strict-from-empty workspace lints (ADR-0010 amendment).

**Acceptance criteria:**
- [x] `.github/workflows/ci.yml`: checkout → pinned toolchain → `Swatinem/rust-cache` → fmt-check, clippy `-D warnings` (`--all-targets --all-features`), test, `cargo doc` with `RUSTDOCFLAGS=-D warnings`; runs on PR + push to main; `concurrency: cancel-in-progress`.
- [x] `justfile` with `fmt`, `fmt-check`, `clippy`, `fix`, `test`, `doc`, `check`.
- [x] `Cargo.lock` committed (in Task 1).
- [x] Strict workspace lints enabled while empty: `unsafe_code=forbid`, `missing_docs=warn`, `rust_2018_idioms=warn`, clippy `pedantic` + `unwrap_used`/`expect_used`/`dbg_macro` deny; `allow-{unwrap,expect}-in-tests`.

**Verification:**
- [x] The full gate (`fmt-check` / `clippy -D warnings` / `test` / `doc`) is green locally on the scaffold; CI green on the pushed branch (PR).

**Dependencies:** Task 1
**Files:** `.github/workflows/ci.yml`, `justfile`, `Cargo.lock`
**Scope:** S

### Checkpoint A — empty workspace compiles; `just check` green in CI. Review before Phase 1.

---

## Phase 1: Core spine (mock provider, zero API spend)

## Task 3: `locode-protocol` types + report envelope ✅ done
**Description:** Pure types shared by all crates: the 4-role conversation model (ADR-0013), tool call/result, and the JSON report envelope (ADR-0009). Provider-neutral, Anthropic-shaped; no wire (de)serialization here (that lives in each `Provider` impl).

**Acceptance criteria:**
- [x] `Conversation { messages: Vec<Message> }`; `Message { role, content: Vec<ContentBlock> }`; `Role ∈ {System, Developer, User, Assistant}` (ADR-0013).
- [x] `#[non_exhaustive] ContentBlock`: `Text`, `Image(ImageSource)`, `Thinking{text,signature?}`, `ToolUse{id,name,input:Value}`, `ToolResult{tool_use_id,content:Vec<ResultChunk>,is_error}`; `ResultChunk ∈ {Text, Image}`. Only `Text`/`ToolUse`/`ToolResult` exercised in v0. (Per-block cache placement deferred to the Anthropic wire via `CacheHint` — ADR-0007/Task 12 — not baked into the block types.)
- [x] Report envelope with `schema_version:1`, `status`, `harness`, `provider`, `final_message`, `structured_output`, `turns`, `tool_calls[]`, `usage`, `session_id`, `error`.
- [x] `status ∈ {completed,max_turns,model_error,error}` serializes to the exact strings in ADR-0009.

**Verification:**
- [x] Golden test: a fixed report serializes to a committed JSON snapshot (freezes the envelope shape).
- [x] Round-trip test: a `Conversation` covering all four roles + `ToolUse`/`ToolResult` pairing serializes/deserializes losslessly (native serde, not a wire format).

**Dependencies:** Task 1
**Files:** `crates/locode-protocol/src/*.rs`, `crates/locode-protocol/tests/envelope_golden.rs`
**Scope:** M

## Task 3b: streaming event protocol types ✅ done
**Description:** The `stream-json` foundation (ADR-0014): a JSONL `Event` enum that makes the stream a **self-sufficient** trace source, plus reconstruction. Types only here; the loop emits them (Task 6) and `locode-exec` streams them (Task 14).

**Acceptance criteria:**
- [x] `#[non_exhaustive] Event` (`#[serde(tag="type")]`): `init{session_id,harness,provider,model,cwd,max_turns,preamble:Vec<Message>,tools:Vec<Value>}`, `message{message:Message}`, `result{report:Report}`, `error{message}`.
- [x] `reconstruct_conversation(&[Event]) -> Conversation` = `init.preamble` ++ every `message` event.
- [x] The terminal `result` event carries the same `Report` as `--output-format json`.

**Verification:**
- [x] JSONL round-trip test (one object per line) + a reconstruction test proving the full history (System/Developer included) rebuilds from the stream alone.

**Dependencies:** Task 3
**Files:** `crates/locode-protocol/src/lib.rs`, tests
**Scope:** S

## Task 4: `locode-tools` contract + registry + dispatch door ✅ done
**Description:** The most important type in the system: the typed `Tool` trait, the `ToolKind` classification tag, error taxonomy, dyn-erasure, and the single `dispatch` door (ADR-0003, ADR-0004, ADR-0008).

**Acceptance criteria:**
- [x] `Tool` trait with `Args: DeserializeOwned+JsonSchema`, `Output: Serialize+ToolOutput`, `kind()`, `description()`, derived `parameters_schema()`, async `run()`.
- [x] `ToolError{Respond,Fatal}`; `ToolCtx{cwd,call_id,workspace_root,cancel}`; `ToolOutput::to_prompt_text()`.
- [x] `DynTool` erasure (JSON decode → run → re-serialize); `Registry` with `dispatch(name,raw_args,ctx)` returning both a history `tool_result` and a report record (`Dispatched{tool_result,record,fatal}`).
- [x] Duplicate-name registration panics at startup; unknown tool + bad args are **soft** (`Respond`).

**Verification:**
- [x] Unit tests: schema derived matches `Args`; bad-args → `Respond`; a trivial echo tool round-trips output/prompt_text; duplicate registration panics; unknown tool soft; fatal sets flag + still pairs; MCP-style `register_dyn` works.

**Design notes (decided during implementation):**
- **Three names, don't conflate.** A tool has a Rust type name (`GrokReadFile`, incidental), a **wire name** (`read_file`, the model-facing name = the registry key, assigned by the pack — hence `Tool` has no `name()`), and a `ToolKind` tag (cross-pack A/B only). One pack is active per run, so no cross-pack key collision — only duplicates within a pack panic. Wire-name assignment is the pack's job (Task 8).
- **MCP / dynamic tools.** `Registry` has two doors: `register<T: Tool>` (typed, schema derived) and `register_dyn(Box<dyn DynTool>)` (raw, for MCP tools with no compile-time `Args`). `ToolKind` is closed + an `Other` catch-all (mirrors Grok Build). A `TypedTool<T>` adapter (not a blanket impl) keeps the `impl DynTool for McpTool` seam open under Rust coherence.
- **Deps added** (ADR-0003 alignment + Codex/Grok precedent): `async-trait`, `schemars` 1, `thiserror` 2, `tokio-util` (`CancellationToken` for `ToolCtx.cancel`).

**Dependencies:** Task 3
**Files:** `crates/locode-tools/src/{tool,registry,error,ctx,lib}.rs` (tests inline)
**Scope:** M

## Task 5: `locode-provider` trait + MockProvider ✅ done
**Description:** The API-agnostic request/response types and a scripted mock provider — the zero-spend test seam for the loop (ADR-0007).

**Acceptance criteria:**
- [x] `Provider` trait: `api_schema()->&str` + `async fn complete(&self,&ConversationRequest)->Result<Completion,ProviderError>`.
- [x] `ConversationRequest{messages,tools: Vec<ToolSpec>,sampling_args,cache_hint}` (no separate `system` — the wire hoists leading System messages, ADR-0013); `SamplingArgs{max_tokens,temperature,top_p,reasoning_effort}`; `Completion{content: Vec<ContentBlock>,usage,stop: StopReason}`.
- [x] `MockProvider` returns a scripted sequence of results (tool-call turn then final text; can inject errors); panics if over-consumed.
- [x] Reusable partial-JSON tool-arg accumulation helper (`ToolCallAssembler`: raw string per index, parse at stop → `ContentBlock::ToolUse`), unit-tested standalone.

**Verification:**
- [x] 11 unit tests: mock emits scripted turns in order; scripts errors; panics when over-consumed; `retryable()` classification; thinking blocks preserved; assembler stitches fragments / empty→`{}` / index order / missing-start + invalid-JSON errors.

**Design notes (decided during planning, grounded in Grok/Codex source):**
- **`Completion` carries `Vec<ContentBlock>`, not `text`+`tool_calls`.** It is the *normalized response* (not any wire's raw shape); an ordered block list preserves thinking/text/tool_use order and **keeps `Thinking{signature}` for replay** (grok `conversation.rs` reasoning replay; codex `encrypted_content`). Thinking is **not** deferred.
- **"Provider" = a wire schema, not a gateway.** `api_schema()` (renamed from `name()`) returns the protocol-shape id (`anthropic`/`mock`). We implement ~3 schemas total; gateways (OpenRouter/Bedrock/proxy) are config (`base_url`/auth/headers) pointed at a schema — grok `ApiBackend` vs un-enumerated base_url; codex `WireApi` + `ModelProviderInfo`. ADR-0007's `{base_url,api_backend,extra_headers}` record already encodes this.
- **`SamplingArgs` = neutral common core only.** Per-wire params (Anthropic `top_k`/thinking budget, OpenAI `frequency_penalty`) live in each wire's builder (Task 12); `reasoning_effort` is a neutral enum mapped per-wire (grok `to_messages_api`/`to_responses_api`). Not a grand superset.
- **`ProviderError` is exhaustive** (not `#[non_exhaustive]`) with `retryable()` matching every variant (grok/codex both do this); distinct terminal variants (context/quota/auth) + a general `Api{status}` escape. `StopReason` *is* `#[non_exhaustive]` + `Unknown(String)` (mirrors an open wire enum).
- **`ToolSpec` hoisted to `locode-protocol`** so both `locode-tools` (builds it) and `locode-provider` (consumes it) share it without violating `provider ↛ tools`.
- Two-tier retry: transport tier is the wire's (Task 12); bounded loop resample is the engine's (Task 6). Task 5 only fixes the taxonomy + `retryable()`.

**Dependencies:** Task 3, Task 4 (ToolSpec)
**Files:** `crates/locode-provider/src/{provider,request,completion,mock,assemble,lib}.rs` (tests inline); `ToolSpec` moved to `crates/locode-protocol/src/lib.rs`
**Scope:** M

## Task 6: `locode-engine` loop + Session API ✅ done
**Description:** The sample→dispatch→append loop with all terminal conditions and transcript hygiene, driven by MockProvider + trivial tools (ADR-0005, ADR-0004). Highest-leverage test surface.

**Acceptance criteria:**
- [x] `Session` library API drives one run: sample → dispatch (serial) → append → re-sample; returns a `Report` and emits `stream-json` `Event`s (ADR-0014) via an `EventSink`.
- [x] Terminal states: `Completed` (no tool calls), `MaxTurns` (post-dispatch check), `ModelError` (after bounded resample keyed on `ProviderError::retryable()`), `Error` (`Fatal`).
- [x] Pre-send `repair_pairing` (in `locode-provider`) guarantees every `tool_use` id has exactly one `tool_result`; abort/mid-batch synthesizes `is_error` results.
- [x] `Respond` errors become `tool_result{is_error}` (via `Registry::dispatch`); the loop keeps iterating. Assistant `content` (incl. `Thinking{signature}`) appended verbatim for replay.

**Verification:**
- [x] 10 unit tests: **each** terminal state via MockProvider scripts; mid-batch-abort synthesis; max-turns; thinking preserved; `reconstruct_conversation` round-trip; usage summed; non-retryable = immediate.

**Design notes (per confirmed decisions):**
- `run() -> Report` is **infallible** — every terminal (incl. provider/Fatal errors) is captured in `Report.status`/`error`; exec maps status → exit code.
- **`repair_pairing` lives in `locode-provider`** (provider-layer concern per ADR-0004; engine depends on provider, so it calls it each iteration), not `locode-protocol`.
- **`provider` → `api_schema`** renamed across the report envelope, `Event::Init`, ADR-0009, and the golden snapshot (the field names the wire *schema*, not a gateway).
- `Provider`/`EventSink` are trait objects (`Arc`/`Box`) for runtime `--api-schema`/`--output-format` selection. Module `run.rs` (not `loop.rs` — keyword). `resample_retries` default 2; usage plain-summed.
- **Deferred:** `truncate_for_model` on tool results lands when `locode-host` does (Task 7) — the loop has the seam marked; parallel batches, compaction, streaming, live cancellation all reserved.

**Dependencies:** Tasks 3, 4, 5
**Files:** `crates/locode-engine/src/{config,sink,terminal,session,run,lib}.rs`; `repair.rs` + `AddAssign for Usage` support in provider/protocol
**Scope:** M

### Checkpoint B — full loop reaches every terminal state under MockProvider, zero network. ✅ reached.

---

## Phase 2: The grok harness pack + host

## Task 7: `locode-host` side-effect seam ✅ done
**Description:** The injectable host: path jail, shell exec with limits, fs helpers, shared truncation (ADR-0008).

**Acceptance criteria:**
- [x] Configurable `PathPolicy` (ADR-0008 amendment): `Jailed` (**default**) resolves FS-tool paths under the root and soft-rejects `..`/absolute/**symlink** escapes (hybrid lexical-normalize + canonical-ancestor check; allows not-yet-existing leaves); `Unrestricted` resolves relative against `cwd` and allows escapes (the `--dangerously-skip-permissions`/`--yolo` behavior). Shell tool is not path-jailed.
- [x] Shell exec via **`bash -lc`** (login shell; `shell_program` + `login_shell` are configurable `HostConfig` fields for later shell-detection), captures stdout+stderr+exit, hard timeout (10s default, clamped to max), byte cap with tail-retention during read. Group-kill via `nix::killpg` (SIGTERM→grace→SIGKILL), `unsafe`-free; cooperative cancel via `CancellationToken`; a failed/timed-out/cancelled command is a *successful capture*, not an error.
- [x] Shared `truncate_for_model` (middle-truncation, head+tail + byte marker, UTF-8-safe) — a pure fn + `MODEL_OUTPUT_BUDGET`; the engine applies it centrally post-dispatch when packs land (Task 9 wiring).

**Verification:**
- [x] 15 unit tests: jail rejects parent/absolute/**symlink** escapes, allows in-jail + nonexistent leaves, `Unrestricted` allows escapes; shell captures stdout/stderr/exit, timeout kills a sleeper, cancellation kills a running command, output over cap truncated, null stdin doesn't hang; fs read/write/stat roundtrip + jail rejection; truncate head+tail + UTF-8 seam safety.

**Design notes (as built):**
- Concrete `Host` struct (not a trait) with `HostConfig`/`ExecLimits`; the OS-sandbox seam is a later alternative construction.
- Capture uses **tail-retention** during read (peak memory O(cap)); the shared `truncate_for_model` (middle) runs on top centrally — belt-and-suspenders.
- `write_file` does **not** auto-create parents (footgun); revisit for the grok `search_replace` create path (Task 10).
- `rg` resolver reserved for Task 11 (ADR-0011). `locode-host` depends on no other `locode-*` crate (defines its own `PathError`/`ExecError`/`FsError`; the pack maps them to `ToolError::Respond`).

**Dependencies:** Task 3
**Files:** `crates/locode-host/src/{lib,path,shell,fs,truncate}.rs` (tests inline)
**Scope:** M
**Deps added:** `nix` (Unix, signal+process — safe `killpg`); dev-only `tempfile`; `tokio` features (process/io-util/fs/time/rt/macros).

## Task 8: `locode-packs` — pack framework + grok pack wiring ✅ done
**Description:** The harness-pack layer (ADR-0012). A `Pack` = a named tool set + a base preamble + registration; `--harness` selects one. No re-skin machinery — each pack holds real tools. v0 wires the grok pack (scaffold).

**Acceptance criteria:**
- [x] `Pack` abstraction: `name()`, `register(&mut Registry)` (real wire names), `preamble(&PackContext) -> Vec<Message>` (role-tagged System/Developer — **renamed from `system_prompt()->String`** so a pack expresses its own role split; user decision), provided `build_registry()`; `resolve(name)`/`available()` resolver.
- [x] `grok` pack module scaffolded (`GrokPack`, `&'static` singleton); real tools carry a `ToolKind` tag via `Tool::kind()` when they land (Tasks 9-11), so no per-pack A/B machinery is needed. `register` is empty until Task 9; `preamble` is a scaffold (single `System` message, headless-branched).
- [x] `dispatch` routes a pack's tool names to its impls (proven via a test-local fake pack); duplicate-name registration panics at startup.

**Verification:**
- [x] 7 unit tests: `resolve("grok")` + `available()`; `unknown_harness` Display names the requested + available; fake pack builds expected specs (derived schemas) + routes to impl; duplicate registration panics; grok scaffold wired (empty registry, headless-branched preamble).

**Design notes:**
- `preamble()` returns role-tagged `Vec<Message>` (not a bare system-prompt string) — each pack maps its harness onto our System/Developer roles; the wire (Task 12) places each role. grok's real System-vs-Developer/User split is deferred to Task 13 (grok has no Developer role — its base prompt is a System item; env is injected as User system-reminders).
- Pack identity = the module/struct (not a per-tool namespace tag, unlike grok's multi-tenant server); fresh `Registry` per pack, so no name collision across packs. No name/param override layer (ADR-0012 drops the re-skin).

**Dependencies:** Task 4
**Files:** `crates/locode-packs/src/{lib,pack,grok/mod}.rs` (tests inline). Deps: `thiserror`; dev `async-trait`/`schemars`/`serde`/`serde_json`/`tokio`/`tokio-util`.
**Scope:** M

## Task 9: grok pack — `run_terminal_cmd` + `read_file` ✅ done
**Description:** Port Grok Build's terminal + read tools from `xai-grok-tools` onto our `Tool` trait, over the host. **Faithful mimicry** (AGENTS.md): grok's real names/schemas (incl. `#[schemars(description=…)]` strings verbatim)/behavior/caps.

**Acceptance criteria:**
- [x] `run_terminal_cmd` (grok's real name) + `read_file` implement `Tool`, holding `Arc<Host>`; go through `locode-host` only. Grok's real arg schema (`command`/`timeout`/`description`; `target_file`/`offset`/`limit`) with verbatim `#[schemars(description)]`; read caps 1000-line/25k-token; `is_background`/`pages`/`format` dropped (reserved seams).
- [x] `read_file` dual output — structured `{path,lines,truncated}` + numbered-body prompt_text (`N→content`). **No freshness store** (grok has none — faithful mimicry; interview decision supersedes the plan's freshness plumbing).
- [x] Errors soft (`ToolError::Respond`): non-zero exit / timeout are a *successful capture* (not an error); jail escape / not-found / too-large map to soft results.
- [x] **Pack host-threading:** `Pack::register`/`build_registry` now take `&Arc<Host>` (tools need the OS seam at construction; `ToolCtx` is too small to carry it) — refines the Task 8 signature.

**Verification:**
- [x] 6 grok tests (via `build_registry` + `dispatch`): echo (`exit: 0` + `hi`), non-zero exit is soft-ok, read numbers lines, 1500-line read truncates at 1000, not-found soft error, outside-jail soft error. (The full mock-provider engine run under `--harness grok` is deferred to Task 14, where the facade composes engine+packs+provider.)

**Dependencies:** Tasks 6, 7, 8
**Files:** `crates/locode-packs/src/grok/{mod,terminal,read}.rs`; `pack.rs` (host-threaded `register`). Deps moved to non-dev: `async-trait`/`schemars`/`serde`; dev `tempfile`.
**Scope:** M

## Task 10: grok pack — `search_replace` (grok's real edit; no standalone `write`) ✅ done
**Description:** Port grok's `search_replace` (exact-string edit **and** file creation via empty `old_string`). The edit slice — where real bugs live; replicate grok's guardrails **faithfully**. **No standalone `write` tool** — grok has none (verified: no `write` module in `implementations/grok_build/`).

**Acceptance criteria:**
- [x] `search_replace` replicates grok's **real** behavior: `old==new` → soft "same string"; **empty `old_string` → create the file** (`handle_new_file_creation`; refuses to clobber a non-empty existing file); not-found → soft "use read_file"; multiple matches without `replace_all` → soft "found multiple times"; `replace_all` replaces all; unique match → replace one. Verbatim grok `#[schemars(description)]`. **No runtime mtime-freshness check** (grok has none — faithful mimicry).

**Verification:**
- [x] 6 unit tests (via `dispatch`): create-on-empty-`old_string`, unique edit, no-op soft error, not-found soft error, multiple-matches soft error, `replace_all`.

**Design notes:** exact (byte) matching in v0 — grok's Unicode-normalization matching is a deferred faithfulness detail. Failure cases map to `ToolError::Respond` (is_error), functionally equivalent to grok's `Ok(SearchReplaceOutput::X)` guidance outputs.

**Dependencies:** Task 9
**Files:** `crates/locode-packs/src/grok/search_replace.rs` (tests in `grok/mod.rs`)
**Scope:** M

## Task 11: grok pack — `grep` (ripgrep) + `list_dir` (grok's walker) ✅ done
**Description:** Port grok's search + directory tools **faithfully** (AGENTS.md; ADR-0011 amendment). grok's `grep` is ripgrep-backed (resolved through the host); grok's directory tool is **`list_dir`, a real fs tree walker** — ported as-is, **not** an rg-glob (the rg-glob is the `locode` pack's choice, next milestone).

**Acceptance criteria:**
- [x] `locode-host` exposes `rg_program()` (`LOCODE_RG_PATH` override → bare `rg` on PATH by name; bundled path deferred to Task 14) + a `run_capture(program, args, cwd, …)` argv process-runner (shares `exec`'s capture/timeout/kill/cancel) + a jailed `read_dir`.
- [x] grok's `grep` (`pattern`/`path`/`glob`/`case_insensitive`; context/type/multiline/output-mode dropped) over the resolved `rg` with grok's flag set (`--heading --with-filename --line-number --color=never --max-columns 1000 --max-columns-preview`, `--ignore-case`, `--glob`, `--regexp … -- PATH`). rg exit 0 = matched, 1 = no-match (soft-ok), 2+ = error; spawn failure → soft `Respond`.
- [x] grok's `list_dir` as a **self-implemented recursive walk** over `Host::read_dir` (jailed), indented tree, char/item budget (grok's 10k `DEFAULT_MAX_OUTPUT_CHARS`). Simplifications flagged: no gitignore filter; simpler budget than grok's seed+deep-walk.

**Verification:**
- [x] 4 tests: `grep` finds matches (+ filename), no-match is soft-ok (both gated on `rg` present); `list_dir` walks a temp tree; missing dir → soft error. (Host: 15 tests still green after the `run_capture` refactor.)

**Dependencies:** Tasks 6, 7, 8
**Files:** `crates/locode-host/src/{rg,shell,fs,lib}.rs`, `crates/locode-packs/src/grok/{grep,list_dir}.rs` (tests in `grok/mod.rs`)
**Scope:** M

### Checkpoint C — the grok pack's tools work end-to-end (via `dispatch`); edit + jail tested. ✅ reached. (The full mock-provider engine run under `--harness grok` composes at Task 14.)

---

## Phase 3: Live Anthropic wire + minimal CLI

## Task 12: Anthropic Messages wire impl ✅ done
**Description:** The one live `Provider` wire (ADR-0007). Correctness of caching/retry/pairing matters most here.

**Acceptance criteria:**
- [x] Builds the Messages request from `ConversationRequest`; parses response; preserves tool-call ids verbatim; extracts usage.
- [x] **Tool schemas:** a shared normalization helper turns `Registry::specs()` (schemars draft-2020-12: `$defs`/`$ref`/`$schema`) into the tool `input_schema` the API accepts. **First verify** whether Anthropic (and OpenAI) accept the same derived schema (we assume yes → one shared helper, not per-wire); adjust if not.
- [x] `cache_control` breakpoints: exactly one on last message + ≤4 on system blocks (assert count); temperature omitted when thinking is on.
- [x] **Betas default to `["interleaved-thinking-2025-05-14"]`** (plan §9.3, ADR-0007 amendment): interleaved thinking blocks replay in order with verbatim signatures; budget clamp waived when the beta is on; other betas opt-in.
- [x] **`ApiBackend { Native, OpenRouter, Proxy }`** (plan §9.2): `OpenRouter` auto-detected from the base-URL host — Bearer auth, beta list mirrored to `x-anthropic-beta`, default `provider` preferences injected (`require_parameters:true` etc., overridable).
- [x] Two-tier retry (transport backoff+jitter honoring `Retry-After`; bounded loop-level resample); **429 surfaced** not hammered; context-overflow/quota terminal; 401 → refresh once → retry.
- [x] Pre-send `repair_pairing` runs before every request; config = the modest record `{api_schema, base_url, api_key, model}` via env (`LOCODE_API_SCHEMA`/`LOCODE_BASE_URL`/`LOCODE_API_KEY`/`LOCODE_MODEL`) + `--api-schema`, designed to grow to per-model `{extra_headers, auth}`. Default model `claude-sonnet-5`; `api_schema` string is plain `"anthropic"`.

**Verification:**
- [x] Tests against recorded/fixture responses (no live key in CI): request shape asserts cache-marker count; retry classifies 5xx vs 429 vs terminal; id preservation checked; the normalized tool schema matches the API's expected `input_schema` shape; an interleaved-thinking fixture (thinking→tool_use→thinking→tool_use) round-trips signatures.
- [x] Manual live smoke at task end against **OpenRouter** (plan §9.4): interleaved-thinking replay across turns, cache tokens non-zero on request 2, one real error body through `classify`. Never in CI.

**Design notes (as built, see plan §9.5):**
- Live smoke passed against OpenRouter (interleaved-thinking + signature + `redacted_thinking` replay, full cache read on turn 2, real error body classified terminal).
- `ContentBlock::RedactedThinking` added to the protocol (ADR-0013 amendment) — observed live; must replay verbatim.
- Default OpenRouter `provider` prefs keep **Vertex allowed** (only Bedrock ignored — user decision); pin `only: ["anthropic"]` per-config when cache-hit determinism matters (cross-provider routing re-writes instead of reads the cache).
- End-to-end provider tests run against a canned local `TcpListener` server (no network in CI); the live smoke is `#[ignore]`d and manual.

**Dependencies:** Task 5
**Files:** `crates/locode-provider/src/anthropic/*.rs`, tests/fixtures
**Scope:** L

## Task 13: grok pack system prompt ✅ done
**Description:** The grok pack's system prompt, ported from grok's real prompt (minijinja-rendered, grok-sized), with identity branched on headless (design doc §8).

**Acceptance criteria:**
- [x] Renders grok's identity (autonomous vs interactive branch) and tool guidance with the grok pack's real tool names. **Correction (research finding):** cwd/OS/shell/date are NOT in grok's system prompt — they ride in the first user message; `preamble()` returns `[System(rendered prompt), User(<user_info> prefix)]` using grok's own headless-minimal variant (`construct_user_message_minimal`).
- [x] Rendered length ≈ grok-sized (short; grok's own 16 KiB soft ceiling asserted); placeholders resolve; no `${{`/`${%` tokens leak.

**Verification:**
- [x] Byte-frozen golden snapshots (headless + interactive, `UPDATE_SNAPSHOTS=1` to regenerate); headless toggles the identity line; template copy byte-pinned (length + opener + provenance sha/commit in module docs).

**Design notes (as built):**
- **Byte-exact:** `templates/prompt.md` copied verbatim from the submodule (sha256-verified; grok's `test_encrypted_templates_not_stale` proves that file == shipped bytes). Renderer = grok's own MiniJinja with the custom `${{ }}`/`${% %}` delimiters (new dep `minijinja`, grok's `make_desc_env` ported).
- **`strip_identity` knob** on `PackContext` (user decision): default `false` = faithful ("You are Grok released by xAI."); `true` removes the identity sentence + the `<user_guide>` block from the RENDERED output only — the template copy is never edited. Pinning tests guard against a template refresh making the strip a silent no-op.
- `user_query()` wrapper ported for Task 14 (the system prompt references the `<user_query>` tag).

**Dependencies:** Task 8
**Files:** `crates/locode-packs/src/grok/prompt.rs`, templates, tests
**Scope:** S

## Task 14: `locode` facade + `locode-exec` minimal binary ✅ done (bundle-rg deferred)
**Description:** Public facade and the minimal headless binary with strict stdout discipline (ADR-0009).

**Acceptance criteria:**
- [x] `locode` re-exports the driving API (`Session`, `EngineConfig`, harness/`api_schema` selection, report/event types) **and the full tool surface** (`Tool`, `Registry`, `dispatch`, `ToolCtx`, `ToolOutput`, `ToolSpec`, the pack's concrete tools) so downstream can use our tools in their own loop (SPEC Users #4).
- [x] `locode-exec`: the prompt is a **positional argument** (user decision, 2026-07-18 — mirrors the field convention: Claude Code's `-p/--print` is a *mode* flag with the prompt positional, `codex exec "…"` likewise; locode-exec is always headless so no mode flag at all; absent positional or `-` → read stdin). clap flags `--cwd,--harness(default grok),--api-schema(default anthropic),--max-turns(optional; **default unlimited** — ADR-0005 amendment: no studied harness caps turns by default),--output-format {json,text,stream-json}(default json),--dangerously-skip-permissions(alias --yolo)` (ADR-0014, ADR-0008 amendment); `--dangerously-skip-permissions`/`--yolo` sets `PathPolicy::Unrestricted` (default `Jailed`); `json` = the single `result` Report, `stream-json` = the JSONL `Event` stream, `text` = final message; logs on stderr; narrow `#[allow(clippy::print_stdout)]` on the report/stream writers (the workspace denies it); exit codes per ADR-0009.
- [ ] **DEFERRED** (packaging; PATH/`LOCODE_RG_PATH` resolution works today): optional `bundle-rg` cargo feature (release-gated, ADR-0011): `build.rs` downloads the pinned static `rg` for the target triple (or copies from `LOCODE_BUNDLE_RG_PATH` for offline/CI), `include_bytes!` embeds it, runtime self-extracts once to a cache dir; resolver falls back to PATH.

**Verification:**
- [x] `cargo run -p locode-exec -- "list and summarize this repo"` prints one parseable JSON report; stderr carries logs; a `--api-schema mock` mode runs in CI without a key. (9 CLI integration tests + live Checkpoint D run.)
- [ ] (deferred with `bundle-rg`) `cargo build -p locode-exec --features bundle-rg --release` yields a binary that resolves `rg` with an empty PATH.

**Dependencies:** Tasks 6, 12, 13
**Files:** `crates/locode/src/lib.rs`, `crates/locode-exec/src/main.rs`, `crates/locode-exec/build.rs`, tests
**Scope:** M (L with `bundle-rg`)

### Checkpoint D — end-to-end run against Claude prints one JSON report. **v0 success criteria met.** ✅ reached (2026-07-18: live run via OpenRouter — 4 turns, 6 grok tool calls, cache hits, exit 0, one JSON report).

---

## Next milestone (post-v0): more harness packs → first A/B

## Task 15: remaining packs (`opencode` + our own `locode`) — LATER (after Tasks 19/20)
> Re-scoped (2026-07-19): the `codex` and `claude` packs graduated to their own planned
> tasks (**Task 19** and **Task 20** below, with full plan docs). What remains here is the
> `opencode` faithful port and our own `locode` best-of pack (grok-build-style snake_case
> naming; ADR-0011's rg-glob is scoped to the `locode` pack). Plan these from source when
> their turn comes (AGENTS.md: planning is a research task).

**Acceptance criteria (sketch):**
- [ ] Each pack registers its harness's real tools (names, schemas, descriptions, behavior) and system prompt; selectable via `--harness`.
- [ ] Tools carry `ToolKind` tags so comparable tools align across packs.
- [ ] `locode` pack: our opinionated best-of toolset (rg-backed glob per ADR-0011, apply_patch reuse decision, etc.) — a design deliverable, not a port.

**Verification:**
- [ ] Per-pack unit tests: real tool specs + behavior; `--harness <pack>` routes to that pack's impls.

**Dependencies:** Task 8 (+ Tasks 19/20 precedents; Task 19's shared `apply_patch` parser if opencode/locode reuse it)
**Files:** `crates/locode-packs/src/{opencode,locode}/…`, tests
**Scope:** L (split per pack when implementing)

## ~~Task 16: first A/B run~~ — REMOVED (user decision, 2026-07-18)
A/B runs are just usage of the binary once multiple packs/wires exist — no explicit task
needed. The old acceptance (two reports stamping their `harness`, a diff note) remains a
good ad-hoc exercise, not a tracked deliverable.

### Milestone goal — the three wires (anthropic · openai-chat · openai-responses) + more packs; A/B runs become plain binary usage.

---

## Task 17: OpenAI Chat Completions wire — DEFERRED (user decision 2026-07-19) · **planned: [`plans/task-17-openai-chat-wire.md`](plans/task-17-openai-chat-wire.md)**
> Deferred: the Responses wire covers GPT + Grok natively, sufficient for the eval
> pipeline's native pairs. The plan stays current (its addendum tracks the protocol
> migration); revisit when a target model/provider only speaks chat completions.
**Description:** The Chat Completions `Provider`: `api_schema = "openai-chat"`, `POST {base_url}/v1/chat/completions`, non-streaming, always-Bearer. The **broadest lowest-common-denominator schema** — OpenRouter's default surface for every model/provider (no translation layer in the path), grok's `ApiBackend` default. Motivated by testing non-Anthropic models WITHOUT OpenRouter's Messages/Responses conversion in the path (user decision, 2026-07-18). Deliberately the "reasoning-blind" control wire — reasoning replay only exists on Task 18's wire.

**Acceptance criteria (distilled from the plan — resolve its §9 open questions first):**
- [ ] Build per ADR-0013's OpenAI table: System → `role:"system"`; Developer → `role:"developer"` (default; `SystemReminder` fallback knob mirrors the Anthropic rendering); assistant `Text`+`ToolUse` blocks grouped into ONE message (`content` + `tool_calls[]`, ids verbatim, `arguments` stringified); `ToolResult` → consecutive `role:"tool"` messages (`tool_call_id`) directly after the calling turn.
- [ ] **Nested** tool defs `{type:"function", function:{…}}` (vs Responses' flat shape); freeform `ToolSpec`s always degrade via the shared `{input: string}` helper; `max_completion_tokens` default with a legacy `max_tokens` knob; `reasoning_effort` mapped (`None` → omitted).
- [ ] Parse: `choices[0]`; invalid tool-call `arguments` kept as `Value::String` (soft-error path, no silent `{}`); `refusal` → `Text` + `StopReason::Refusal`; finish_reason table with `Unknown` catch-all; usage incl. `cached_tokens`/`cache_write_tokens`.
- [ ] Reasoning **capture-only**: OpenRouter `message.reasoning` / xAI-style `reasoning_content` → `Thinking{signature: None}`; thinking never replayed on this wire (documented A/B caveat).
- [ ] Consumes Task 18's shared `http` transport + `openai/` config/classify (429-`insufficient_quota`/402 → `Quota`); OpenRouter `provider` prefs injected.
- [ ] Fixture + canned-`TcpListener` tests per plan §6; `--api-schema openai-chat` in `locode-exec`; manual `#[ignore]` live smoke (OpenAI + one non-OpenAI model via OpenRouter).

**Dependencies:** Task 18 (shared transport layer + `openai/` module + degradation helper)
**Scope:** L

## Task 19: codex harness pack — **planned: [`plans/task-19-codex-pack.md`](plans/task-19-codex-pack.md)**
**Description:** Faithful port of Codex CLI's stock headless toolset + base prompt as `--harness codex` (ADR-0012; fidelity boundary per STATUS #9). Codex is the minimal-tool extreme of the A/B bed: **no read/grep/glob/write tools** — the shell is the read path, all edits go through `apply_patch`. Native delivery needs Task 18 (freeform custom tool + Lark grammar, OpenAI-models-only); runs everywhere else via the `{input: string}` JSON degradation.

**Acceptance criteria (distilled from the plan — resolve its §9 open questions first):**
- [ ] Three tools with verbatim names/schemas/descriptions (`deny_unknown_fields`): `shell_command` (10s default timeout; codex's `Exit code:`/`Wall time:` output framing; timeout → exit 124 + "command timed out after…"), `apply_patch` (freeform `input_format()`; untagged Args decoding both raw text and `{"input": …}`), `update_plan` (≤1 `in_progress` guard; in-memory plan state, no reminder machinery).
- [ ] Shared `locode-packs::apply_patch` parser module ported from the `apply-patch` crate: verbatim markers, lenient-only mode incl. heredoc tolerance, `Hunk`/`UpdateFileChunk`, 4-level fuzzy `seek_sequence` (exact → rstrip → strip → Unicode normalize; EOF bias), move support; apply is validate-all-then-write over the Host (jail applies; all failures soft).
- [ ] Prompt: verbatim provenance-pinned copy of codex's default `prompt.md` (the compiled default; per-model catalog variants deferred); `apply_patch_tool_instructions.md` block per the plan's Q2 resolution; `strip_identity` knob; preamble = `[System(prompt…), User(<environment_context>…)]` (codex's placement; System lands in Responses `instructions`).
- [ ] Tests per plan §6: parser suite, dispatch-level tool behavior, spec/schema goldens (freeform + degraded), template byte-pins, preamble snapshots, one canned-server codex-pack-through-responses-wire round-trip.

**Dependencies:** Task 8 (pack framework), Task 18 (freeform ToolSpec + native delivery); runs degraded on Tasks 12/17 wires
**Files:** `crates/locode-packs/src/{apply_patch,codex}/…`, templates, tests
**Scope:** L

## Task 20: claude harness pack — **planned: [`plans/task-20-claude-pack.md`](plans/task-20-claude-pack.md)**
**Description:** Faithful port of Claude Code's headless-relevant toolset + static system prompt as `--harness claude` (ADR-0012; fidelity boundary per STATUS #9 — TodoWrite/Task/reminder machinery excluded). No wire dependency: all tools are JSON function tools; Claude Code's native wire IS our Task-12 Anthropic wire.

**Acceptance criteria (distilled from the plan — resolve its §9 open questions first):**
- [ ] Six tools under Claude Code's exact names `Bash`/`Read`/`Edit`/`Write`/`Glob`/`Grep` — full `ToolKind` width vs grok's 5 — with verbatim zod `.describe()` strings (`#[schemars(description)]`), `z.strictObject` mirrored as `deny_unknown_fields`, and long tool descriptions as provenance-pinned `descriptions/*.md` files.
- [ ] Claude Code's real caps/guardrails: Bash 120s default/600s max/30k output cap; Read 2000-line window + `cat -n` framing; Edit uniqueness + `old!=new` + `replace_all`; Grep = full rg passthrough surface (`output_mode`, `-A/-B/-C/-n/-i`, `type`, `glob`, `head_limit` 250, `multiline`) with 20k cap; Glob mtime-sorted, 100-file cap (via `rg --files -g`).
- [ ] The **read-before-edit + modified-since-read gate** (`ClaudeSessionState` shared by Read/Edit/Write) — Claude Code's signature guardrail, deliberately divergent from grok's no-freshness design.
- [ ] Static prompt: verbatim section constants (identity prefix branch: headless → Agent-SDK line, interactive → "You are Claude Code…"; sections rendered for OUR tool pool), runtime-shaped `# Environment` block from `PackContext`; preamble = `[System(prompt), User(<system-reminder> currentDate block)]`; reminder machinery/CLAUDE.md/git-status excluded per plan (open questions Q3/Q4); `strip_identity` knob.
- [ ] Tests per plan §6: schema goldens per tool, description byte-pins, prompt snapshots (both identity branches), freshness-gate behavior suite, dispatch-level tool tests.

**Dependencies:** Task 8 (pack framework); any wire for live runs
**Files:** `crates/locode-packs/src/claude/…`, descriptions, snapshots, tests
**Scope:** L

## Task 18: OpenAI Responses wire ✅ done (2026-07-19) · **planned: [`plans/task-18-openai-responses-wire.md`](plans/task-18-openai-responses-wire.md)**
> **Review decisions folded in (2026-07-19, plan addendum):** step 0 is a protocol migration —
> unified `Reasoning{format, text, signature, payload}` block (+`ReasoningFormat`) replacing
> `Thinking`/`RedactedThinking`; `Usage` counters become `Option<u64>` (+`reasoning_tokens`);
> `ReasoningEffort` extends to `None…XHigh + Other(String)`. ADR-0013/0003/0007 amendments
> land with the code (ADR-first).
**Description:** The OpenAI Responses `Provider`: `api_schema = "openai-responses"`, `POST {base_url}/v1/responses`, non-streaming, always-Bearer, **stateless** (`store: false` always — codex and grok both run it that way; OpenRouter's beta *rejects* stateful requests). Required for faithful codex (Responses-only; freeform+Lark `apply_patch`) and grok build's own backend for xAI models (encrypted reasoning replay, ZDR). Verified live through OpenRouter (custom-grammar tools, `instructions`, `provider` prefs, xAI encrypted reasoning — plan §0 probe log).

**Acceptance criteria (distilled from the plan — resolve its §9 open questions first, esp. Q3 reasoning-replay encoding):**
- [x] **`ToolSpec` protocol change (ask-first)**: `parameters` → `input: ToolInputFormat { JsonSchema{parameters} | Freeform{syntax, definition} }`; defaulted `input_format()` on `Tool`/`DynTool`; ADR-0003 amendment text from plan §8.1 applied.
- [x] Build: System → `instructions` hoist (default; `InputMessage` knob), Developer → `role:"developer"` item, `ToolResult` → `function_call_output` (order-preserving explosion), `ToolUse` → `function_call`/`custom_tool_call` (`call_id` verbatim, no item `id`), FLAT function-tool shape + `{type:"custom", format:{grammar,lark}}`, `store:false` + `include:["reasoning.encrypted_content"]` on every request, `previous_response_id` never serialized.
- [x] Reasoning replay **whole-item-opaque**: `reasoning` output items round-trip byte-preserved (unknown fields included) via `Thinking{text: summary concat, signature: Some(item JSON)}`; replayed in block position minus `status`; ADR-0013 amendment per plan §8.2.
- [x] Parse: output-array iteration with `#[serde(other)]` tolerance; custom calls + invalid function args → `Value::String` (soft-error path); usage mapping incl. `cached_tokens` + `cache_write_tokens`; `status`/`incomplete_details` → stop mapping; in-body `status:"failed"` classified like grok (retryable 500 for `server_error`).
- [x] **Transport hoist**: `RetryPolicy`/`run_with_retry`(generic)/`backoff`/`HttpFailure`/`parse_retry_after`/`normalize_input_schema` move to shared `locode-provider::http`; the anthropic suite passes unchanged; OpenAI-family `classify` in `openai/` (429-`insufficient_quota` and OpenRouter 402 → `Quota`; numeric-code error bodies).
- [x] Config: `OpenAiModelConfig` (Bearer always; `OpenAiBackend::{Native,OpenRouter,Proxy}` detection; `custom_tools_supported` degradation knob for xAI; OpenRouter `provider` prefs injection; same `LOCODE_*` env story).
- [x] Fixture + canned-`TcpListener` tests per plan §6; `--api-schema openai-responses` in `locode-exec`; manual `#[ignore]` live smoke: gpt-5-mini grammar-tool round-trip + x-ai/grok-4.5 encrypted-reasoning replay + cache proof.

**Dependencies:** Task 12 (patterns), Task 5 (trait)
**Scope:** L

---

## Task 21: graceful SIGTERM in `locode-exec`
**Description:** Adopted from the intent interview (2026-07-19): swe-lab timeouts kill the
process; today that loses the report. On SIGTERM: trigger the engine `CancellationToken`,
let the mid-batch abort synthesize the paired transcript (already built + tested, Task 6),
and still emit the report (partial turns/usage) before exiting — so timed-out eval runs
yield failure-case traces instead of nothing.

**Acceptance criteria:**
- [x] SIGTERM during a run → exit with the normal artifact on stdout (`json`: one report with
  a non-`completed` status; `stream-json`: the stream stays valid JSONL ending in `result`);
  transcript validity holds (every `tool_use` paired).
- [x] SIGTERM before the run starts → clean exit 1, nothing on stdout.
- [x] Integration test drives the binary, sends SIGTERM mid-run (slow mock tool), asserts the
  report parses.

> **Correction (2026-07-20):** these boxes were previously checked `[x]`, but no
> signal-handling or cancellation code existed in the tree (verified: no
> `signal`/`SIGTERM` references in `locode-exec`, no implementing commit).
> **Superseded by Task 24** (ADR-0018) — and **delivered by it (2026-07-21)**:
> the public cancel handle + `cancelled` status + exec SIGTERM handler, with
> the mid-run/pre-run integration tests (env-scripted slow mock tool).

**Dependencies:** Task 14
**Scope:** S — folded into Task 24

## Task 22: custom provider injection — `ProviderRegistry` + lib-entry `locode-exec` ✅ done
**Description:** ADR-0015. Downstream consumers need to select providers we don't ship
(custom wires in their own codebases). Add `ProviderRegistry` (name → factory) to the
`locode-core` facade with the built-ins pre-registered; move `locode-exec`'s substance into
its lib target behind `main_with(registry)`; `--api-schema` becomes a registry-validated
string instead of a closed `ValueEnum`.

**Acceptance criteria:**
- [x] `ProviderRegistry::builtin()` = `anthropic` / `openai-responses` / `mock`;
  `register` adds or replaces; unknown `--api-schema` fails pre-run (exit 1) listing
  available names.
- [x] The shipped binary is a ~5-line `main_with(ProviderRegistry::builtin())`; a
  downstream binary registering a custom factory selects it via `--api-schema <name>`.
- [x] Registry unit tests (builtin names, custom registration, replacement, unknown-name
  error) + the existing exec integration tests stay green.
- [x] README "Custom providers" section; SPEC facade note; ADR-0015.

**Dependencies:** Task 14, Task 18
**Scope:** M

## Tasks 23–25: TUI core prerequisites (Workstream A) — ADRs accepted, **DELAYED after Task 26**
> Sequencing decision (2026-07-21 plan interview): Task 26 (pack fidelity) runs first;
> these ship afterwards as **0.1.4** (0.1.3 becomes the Task 26 fidelity release).
Detailed plan: [`plans/task-23-25-tui-core-prereqs.md`](plans/task-23-25-tui-core-prereqs.md)
(all open questions resolved in the 2026-07-20 user interview — see the plan's
Resolutions section). Implementation order **23 → 25 → 24**; one 0.1.3 release
at the end. Task 25 additionally ships `Event::Approval` (+`wait_ms`),
`ToolCallRecord.denial_reason`, and `#[non_exhaustive]` on the approval types +
`Event`; Task 24 additionally ships `#[non_exhaustive]` on `Status` and the
exec wildcard exit arm.

### Task 23: session continuity (ADR-0016) ✅ done
Multi-turn conversations: `Session` owns history across `run()` calls; `Init`
once per session; per-run reports; `history()` accessor.
- [x] Two-run continuity + single-`Init` + per-run report + reconstruction golden tests
  (+ continue-after-`ModelError`/`Error` tests per the Resolution).
**Dependencies:** none · **Scope:** S

### Task 24: cancellation + `cancelled` status + SIGTERM (ADR-0018) ✅ done
`Session::cancel_handle()` (per-run token, retired at run end); loop observes
the token (iteration top, provider select, backoff select, between batch calls
with synthetic pairing); `Status::Cancelled` + `#[non_exhaustive]` + the
written additive-evolution policy; exec SIGTERM handler + wildcard exit arm
(delivers Task 21); `CancellationToken` re-exported. Enabler: keyless
env-scripted mock (`LOCODE_MOCK_SCRIPT`) so integration tests can hold a run
open.
- [x] Mid-sample + mid-batch cancel tests; idempotent double-cancel;
  cancel-then-next-run continues (fresh token); exec SIGTERM integration tests
  (stream tail valid + paired, json single report, pre-run exit 1).
**Dependencies:** Task 23 (one test only) · **Scope:** M

### Task 25: approval seam (ADR-0017) ✅ done
`Approver` trait at the engine dispatch step; deny = paired soft error;
`AllowAll` default (zero behavior change); facade re-exports. Also shipped per
the Resolutions: `Event::Approval` (+`wait_ms`), `ToolCallRecord.denial_reason`
(approver-deny path only), `Registry::kind_of`, `#[non_exhaustive]` on
`ApprovalRequest`/`Decision`, ADR-0008 dated amendment.
- [x] Deny/mixed-batch/async-approver/kind tests; golden default-approver run
  (exec integration suite untouched); denial_reason + Approval serde tests.
**Dependencies:** none · **Scope:** M

## Task 26: grok pack schema fidelity — undo the Task 11 "v0 seam" cuts
**Description:** The Task 11 port silently dropped schema fields from three grok tools
("reserved seams"), violating the faithful-mimicry rule (AGENTS.md; ADR-0012): a model
that cannot see `-C`/`head_limit`/`output_mode` cannot reproduce grok's behavior, which
defeats the A/B. Restore full wire-exact schemas + behavior.

**Per-tool audits (2026-07-20, source-verified):** [`tasks/audits/`](audits/) — one file
per tool with verbatim schema diffs, behavior gaps, quirks, and a detailed fixing task.
Plan finalized in the 2026-07-21 interview (resolutions in each audit's "Plan
finalization" section). **Sequencing: host groundwork ∥ read_file → terminal-fg →
search_replace → list_dir. Release: 0.1.3 = this task.**

**Slice 0 — host groundwork (unblocks the three tools below):**
- [ ] `ignore` crate → `locode-host` (approved 2026-07-21): gitignore-aware walk API
  (`WalkOptions`: respect_gitignore, depth/budget) + `is_path_ignored(path)`.
- [ ] Exec: combined interleaved capture + front/back char-cap retention; spill full
  output to a host-owned temp/cache path (outside workspace — jailed-mode readability
  is a recorded future TODO); optional per-request `shell` spec on `ExecRequest`
  (program, arg style, cached login-PATH probe). Host defaults unchanged.

**Acceptance criteria:**
- [x] `grep` — FAITHFUL as of PR #51 ([audit](audits/grep.md); one documented
  output-equivalent deviation: post-capture head-limit).
- [ ] `list_dir` — DRIFT, 8 behavior issues ([audit](audits/list_dir.md)): gitignore
  walker (needs `ignore` crate — **ask-first dependency**), depth-budgeted BFS +
  `[N files in subtree: …]` summaries, exact bullet/header format, truncation notices,
  four exact error texts. Scope M.
- [x] `read_file` — **DONE (PR #55)**: faithful text path (audit criteria 1–8 + 10-as-
  deviation-note; see the audit's Split section): `pages`/`format` schema fields
  (verbatim; PDF-only behavior stays deferred), bare-integer schema shape (lenient
  `offset` coercion **declined** — type-strict per user call, deviation recorded),
  description (PDF/image bullets trimmed + logged), **sparse `N→` numbering**,
  negative-offset tail-read, `total_lines` semantics, overflow messages (incl. grok's
  typo), exact error texts. Scope M, no new deps. ([audit](audits/read_file.md))
- [x] `search_replace` — **DONE: current-default grok**
  (2026-07-21): DI toggles frozen as constants at grok defaults; default-off subsystems
  omitted as recorded "unreachable at default config" deviations; user-edit +
  nearest-match hints, CRLF normalization, overwrite semantics, grok's texts;
  gitignore guard via host `is_path_ignored`. Lenient `replace_all` coercion declined
  (type-strict). Scope S–M. ([audit](audits/search_replace.md))
- [x] `list_dir` — **DONE**: walker lives in
  `locode-host` (Slice 0); pack keeps pure formatting/budgeting — BFS budget + subtree
  summaries, exact formats/notices/error texts. Scope M. ([audit](audits/list_dir.md))
- [x] `run_terminal_cmd` — **DONE (foreground slice)** (audit criteria
  1–8 + 11–12; see the audit's Split section): bg-disabled description variant
  (lenient timeout parsing **declined** — type-strict per user call, deviation
  recorded), 20k-char front/back truncation + exact markers, `exit:
  killed (…)` variants, ANSI-strip/soft-wrap, trailing-`&` rejection (active in our
  configuration **today**), + two host changes (combined capture w/ front/back cap;
  spill file). Scope M. ([audit](audits/run_terminal_cmd.md))
- [x] Sweep the pack for any remaining dropped-field comments — clean (2026-07-21): no `dropped in v0` / `reserved seam` / simplification markers remain.

**Deferred (user decision, 2026-07-20):**
- `read_file` binary/image/PDF/PPTX tier (audit criteria 9 + 11) — binary reads emit
  lossy text until then (known consequence, recorded in the audit).
- `run_terminal_cmd` background mode (audit criteria 9–10): `Host::exec_background` +
  task registry, `is_background`, `<task-id>` envelope, `get_task_output`/`kill_task`.

**Dependencies:** none (independent of Tasks 23–25)
**Scope:** grep S (done) · list_dir M · read M · search_replace M · terminal M–L

## Task 27: locode-tui + locode-app v1 — the minimal interactive frontend ✅ functionally complete (release pending user)
**Description:** Build the TUI per [`SPEC-TUI.md`](../SPEC-TUI.md) (spec grounded in
the four-harness TUI source study, [`docs/research/tui-harness-study.md`](../docs/research/tui-harness-study.md)).
Crate shape (2026-07-21): `locode-tui` = ONE library crate (components + runnable app
behind `main_with`, exec-style; module map with named split triggers); `locode-app` =
flag-free thin product binary, the future assembly point for non-TUI capability.
Six thin slices: shell (terminal lifecycle + loop + composer) → drive-a-run →
cancel → approvals → conversation polish → hardening/release. Inline viewport +
print-once transcript; `Msg → update → Cmd` reducer; TuiApprover with FIFO overlay
queue; robustness floor from slice 1.

**Acceptance criteria:** SPEC-TUI.md "Success criteria" section.
- [x] Slice 1 — shell, both crate scaffolds (+ ADR-0019, SPEC.md pointer) — PR #77
- [x] Slice 2 — drive a run (mock) — PR #78
- [x] Slice 3 — cancel — PR #79
- [x] Slice 4 — approvals — PR #80 (ADR-0017 gap handled approver-side; engine amendment still open)
- [x] Slice 5 — conversation polish (5a #81 + 5b #82)
- [x] Slice 6 — hardening (#83); release = user hard-stop

**Dependencies:** 0.1.4 seams (Tasks 23-25, done). New deps (ask-first, approved with
the spec review): ratatui, crossterm, tui-textarea, pulldown-cmark.
**Scope:** L (sliced)

## Task 28: unified `locode` binary — `-p` headless mode ✅ done
**Description:** `locode` is now the single entry: interactive TUI by default, headless
one-shot under `-p`/`--print` (Claude-Code shape). `locode_tui::main_with` dispatches;
`-p` reuses `locode-exec`'s engine via `run_headless(cli, registry)`. Unified CLI shares
`Harness`/`OutputFormat`; a bare positional prompt pre-fills the composer in TUI mode.
- [x] `run_headless` extracted from `locode-exec::main_with` (behavior-preserving);
  unified `locode-tui::cli::Cli` with `-p`/positional-prompt/`--output-format`/`--max-turns`;
  `main_with` print-dispatch; composer pre-fill; ADR-0019 amendment + SPEC reconcile.
- [x] 3 `-p` process integration tests (json/text/pre-run-fail) + `with_draft` reducer test;
  exec's own tests unchanged (346 workspace tests).
**Retire plan (user-gated):** ✅ binary retired 2026-07-22 — `release.yml` ships only
`locode` (installer already on `locode` since 0.1.5), README de-advertised (ADR-0010 +
ADR-0019 amendments). **Remaining:** collapse the `locode-exec` *crate* — migrate the
headless logic (`run.rs`/`output.rs`/`signal.rs`) into `locode-tui`/a shared lib and drop
the `locode-tui → locode-exec` edge (deferred, mechanical, no user-visible change).
**Dependencies:** Task 27 (TUI). **Scope:** M

## TUI polish backlog — Tier A (autonomous, no core surface)

Screenshot-driven mimicry of Claude Code (user vibe-checks, 2026-07-22). These are
UI-only and run on the autonomous slice loop (`docs/tui-dev-process.md`); the user
reviews PRs. Tier B/C (core-touching) live below / in ADR-0021.

- [x] **Message bullets** (#94) — `•` → filled `●`; assistant text gets a leading white `●`.
- [x] **Assistant left indent** (#94) — hanging indent under the bullet.
- [x] **`TurnEnd` separator "extra rule"** (#94) — de-ruled to subtle dim text.
- [x] **Shift+Enter newline** (#95) — kitty `DISAMBIGUATE_ESCAPE_CODES` (no
  REPORT_EVENT_TYPES → no key-doubling); Release events filtered defensively.
- [x] **P3 tables** (#96 + this PR) — now box-drawing borders (`┌─┬─┐`, bold header,
  `├─┼─┤` rule) matching Claude Code / Grok Build; proportional shrink-to-fit.
- [x] **Composer max height** (#97) — bumped `MAX_ROWS` 5→8 / `LIVE_REGION_ROWS`
  10→11 so the draft grows into the otherwise-blank viewport space. *Superseded by
  the dynamic composer (ADR-0022, #104) — the viewport is now dynamic, not fixed.*
- [x] **Dynamic composer + no idle gap** (ADR-0022, #102/#104, 0.1.7) — the
  2026-07-22 vibe-check finding below (fixed `LIVE_REGION_ROWS`, ~6-row idle gap)
  is **resolved**: a minimal vendored terminal runs a relative-frame render, the
  composer grows/shrinks with the transcript and is bottom-pinned, and the idle
  gap is gone. This revisited ADR-0019's fixed-height decision (superseding
  amendment recorded there).
- [x] **User-prompt shaded band** (#106) — the submitted prompt renders as a
  full-width `Color::DarkGray` band (theme-following), `❯ ` at col 4, vpad rows
  above/below — grok/codex's pure-bg-fill shape. SPEC-TUI + ADR-0019 reconciled.
- [x] **Footer clock** (#106/#107) — right-aligned local date + `HH:MM` behind a
  clock icon; `chrono::Local` honors `TZ`/`/etc/localtime` (no in-app tz config);
  minute precision (zero idle repaints); numeric-offset `%Z` dropped (no tz-db dep).
- [x] **Footer two-row corner layout + colors** (#107/#108) — cwd (top-left,
  bright blue), clock+time (top-right, dim), model (bottom-left, gray), `N tokens`
  (bottom-right, red), each corner bold; separators removed. cwd color matches the
  user's `ccstatusline` `current-working-dir: brightBlue`. ADR-0019 reconciled.
- [ ] **P2 OSC-8 hyperlinks** — clickable links (iTerm2 supports them).
- [ ] **Built-in slash commands** — **deferred pending a careful, holistic
  design pass** (user decision, 2026-07-22): do NOT implement piecemeal. Before
  adding any (`/help`, `/clear`, `/model`, …) beyond the current `/new` `/quit`
  `/exit`, design the command surface as a whole — discovery/registry, syntax,
  which are pure-UI vs. seam- or persistence-backed (`/model`, see the finding
  below), and how they compose. Revisit when scheduled.

### Findings from the 2026-07-22 vibe-check (flagged, NOT auto-implemented)

- **Dynamic composer height (grow to ~50% like Claude Code, and remove the idle
  gap).** ✅ **RESOLVED** by ADR-0022 (vendored terminal, #102/#104, 0.1.7) — see
  the checked item above. Original finding: our inline viewport was a **fixed**
  `LIVE_REGION_ROWS`, so the composer couldn't grow and idle rows showed as a
  ~6-row blank gap; Claude Code uses `maxHeight="50%"` of a **dynamic** viewport
  (`PromptInput.tsx:191`), and codex/grok both needed custom/forked terminal
  code. We took the same route (minimal vendored terminal, relative-frame render).
- **`/model` slash command — Tier B, NOT Tier A (blocked on two missing seams).**
  Investigated 2026-07-22. `/model` can't ride the autonomous UI loop because it
  reaches past the UI:
  1. **No model-selection seam.** The model is chosen *inside* the provider
     factory (`providers.rs:88` `AnthropicProvider::from_env()` → `config().model`);
     `ProviderInit` carries only `session_id`, and there's no way for the UI to
     request a model or enumerate a provider's models. Adding either changes the
     ADR-0015 `ProviderRegistry`/factory **public surface** → ask-first + a
     hard-stop for the autonomous loop. Switching mid-session then means rebuilding
     the session (like `/new`) with the new model.
  2. **No config persistence.** Other harnesses persist the choice (Claude Code
     `~/.claude/settings.json`); we have **no config-file design**. Remembering a
     `/model` pick across restarts needs a new "config file" ADR (XDG
     `~/.config/locode/`, mirroring how `ccstatusline` uses `~/.config/`).
  **Status (user decision, 2026-07-22):** `/model` is **deferred** — not even the
  read-only form ships yet; it's folded into the holistic slash-command design
  pass above. When taken up: read-only `/model` is Tier A; *switching* needs the
  ADR-0015 seam; *persistence* needs the config-file ADR.
- **"Intermediate model messages aren't rendered" — NOT a bug.** The engine emits
  an `Event::Message` per assistant turn (`locode-engine/src/run.rs:104`) and the
  TUI's `on_event` renders every assistant `Text` block (verified + tested,
  `app.rs:904`). The perceived difference is: (a) **no streaming** — each turn's
  text appears all-at-once after the turn buffers, not live (that live-narration
  feel is streaming, ADR-0021 / Tier C); and (b) the active **grok pack narrates
  less** than Claude Code's prompt would (faithful mimicry — the claude pack would
  narrate more). No Tier-A change.

Tier C (core, ADR-first): **streaming** → [ADR-0021](../docs/decisions/ADR-0021-live-token-streaming.md)
(unblocks markdown study Phase 4 + live intermediate narration); subagents;
plugins. Tier B (short ADR then mostly-autonomous): background bash commands,
AGENTS.md/CLAUDE.md loading, custom slash-command files, **`/model` switching**
(ADR-0015 model-selection seam) + **config-file persistence** (new ADR, XDG
`~/.config/locode/`). (The dynamic composer viewport — previously listed here —
shipped in ADR-0022.)

## Deferred (reserved seams, not scheduled)
parallel tool batches (RwLock read/write) · compaction · OS sandbox · MCP · streaming events
(SSE seams reserved in each wire plan) · `--json-schema` answers · JSONL session durability ·
multi-platform `rg` bundle matrix + macOS notarization/sidecar (packaging, ADR-0011) ·
pack session-start file context (AGENTS.md/CLAUDE.md loading — plans 19 Q4 / 20 Q3) ·
codex unified exec (PTY) / `view_image` / hosted `web_search` · Claude Code
TodoWrite/Task/WebFetch/WebSearch/NotebookEdit (plan 20 §1) · per-model codex prompt variants ·
grok pack multimodal `read_file` tier (binary/image/PDF/PPTX — Task 26 deferral) ·
grok pack background commands (`is_background` + `get_task_output`/`kill_task` + host
registry — Task 26 deferral).
