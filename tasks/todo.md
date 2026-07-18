# locode-core v0 — Task List

Detailed, ordered tasks for [`plan.md`](plan.md). Each clears the Definition of Done in the plan.
Sizes: XS=1 file · S=1–2 · M=3–5 · L=5–8 (break down if larger).

> **📍 Current state, deviations from plans, and open concerns: [`STATUS.md`](STATUS.md).**
> Read it first — the merged code is the source of truth; plans/ADRs may be legacy. Tasks 1–13
> are done (Checkpoints A/B/C + the live wire + the grok prompt); **Task 14 (facade + exec) is next.**

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

## Task 14: `locode` facade + `locode-exec` minimal binary
**Description:** Public facade and the minimal headless binary with strict stdout discipline (ADR-0009).

**Acceptance criteria:**
- [ ] `locode` re-exports the driving API (`Session`, `EngineConfig`, harness/`api_schema` selection, report/event types) **and the full tool surface** (`Tool`, `Registry`, `dispatch`, `ToolCtx`, `ToolOutput`, `ToolSpec`, the pack's concrete tools) so downstream can use our tools in their own loop (SPEC Users #4).
- [ ] `locode-exec`: clap flags `--prompt,--cwd,--harness(default grok),--api-schema(default anthropic),--max-turns(default 30),--output-format {json,text,stream-json}(default json),--dangerously-skip-permissions(alias --yolo)` (ADR-0014, ADR-0008 amendment); `--dangerously-skip-permissions`/`--yolo` sets `PathPolicy::Unrestricted` (default `Jailed`); `json` = the single `result` Report, `stream-json` = the JSONL `Event` stream, `text` = final message; logs on stderr; narrow `#[allow(clippy::print_stdout)]` on the report/stream writers (the workspace denies it); exit codes per ADR-0009.
- [ ] Optional `bundle-rg` cargo feature (release-gated, ADR-0011): `build.rs` downloads the pinned static `rg` for the target triple (or copies from `LOCODE_BUNDLE_RG_PATH` for offline/CI), `include_bytes!` embeds it, runtime self-extracts once to a cache dir; resolver falls back to PATH.

**Verification:**
- [ ] `cargo run -p locode-exec -- --prompt "list and summarize this repo"` prints one parseable JSON report; stderr carries logs; a `--api-schema mock` mode runs in CI without a key.
- [ ] `cargo build -p locode-exec --features bundle-rg --release` yields a binary that resolves `rg` with an empty PATH.

**Dependencies:** Tasks 6, 12, 13
**Files:** `crates/locode/src/lib.rs`, `crates/locode-exec/src/main.rs`, `crates/locode-exec/build.rs`, tests
**Scope:** M (L with `bundle-rg`)

### Checkpoint D — end-to-end run against Claude prints one JSON report. **v0 success criteria met.** Review.

---

## Next milestone (post-v0): more harness packs → first A/B

## Task 15: additional packs (`codex` / `claude` / `opencode`) + `locode`
**Description:** Faithful ports of the other studied harnesses' real toolsets, plus our own `locode` best-of pack (grok-build-style snake_case naming). Real per-harness implementations, not re-skins (ADR-0012). The `codex` pack introduces `apply_patch` (JSON-string framing on the Anthropic wire).

**Acceptance criteria:**
- [ ] Each pack registers its harness's real tools (names, schemas, descriptions, behavior) and system prompt; selectable via `--harness`.
- [ ] Tools carry `ToolKind` tags so comparable tools align across packs.
- [ ] `codex` pack: `apply_patch` via a shared parser, delivered as a JSON string arg.

**Verification:**
- [ ] Per-pack unit tests: real tool specs + behavior; `--harness <pack>` routes to that pack's impls.

**Dependencies:** Task 8 (+ Task 12 for live runs)
**Files:** `crates/locode-packs/src/{codex,claude,opencode,locode}/…`, shared `apply_patch` parser, tests
**Scope:** L (multiple packs — split per pack when implementing)

## Task 16: first A/B run
**Description:** The payoff — run one task under two packs and compare their genuinely different behavior.

**Acceptance criteria:**
- [ ] Same prompt runs under `--harness grok` and another pack; both reports stamp their `harness`.
- [ ] A short doc note captures the trajectory/token/edit-success diff (aligned by `ToolKind`).

**Verification:**
- [ ] Two reports produced; diff recorded in `docs/` or a scratch note.

**Dependencies:** Tasks 14, 15
**Files:** `docs/ab-notes.md` (or scratch), no core code changes required
**Scope:** XS

### Milestone goal — two packs, one task, genuinely different tool behavior; the A/B is honest and mechanical.

---

## Deferred (reserved seams, not v0)
freeform-grammar `apply_patch` (OpenAI Responses wire) · OpenAI Chat Completions wire · parallel tool
batches (RwLock read/write) · compaction · OS sandbox · MCP · streaming events · `--json-schema`
answers · JSONL session durability · multi-platform `rg` bundle matrix + macOS notarization/sidecar (packaging, ADR-0011).
