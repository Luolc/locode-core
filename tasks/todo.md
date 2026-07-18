# locode-core v0 — Task List

Detailed, ordered tasks for [`plan.md`](plan.md). Each clears the Definition of Done in the plan.
Sizes: XS=1 file · S=1–2 · M=3–5 · L=5–8 (break down if larger).

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

## Task 6: `locode-engine` loop + Session API
**Description:** The sample→dispatch→append loop with all terminal conditions and transcript hygiene, driven by MockProvider + trivial tools (ADR-0005, ADR-0004). Highest-leverage test surface.

**Acceptance criteria:**
- [ ] `Session`/`Engine` library API drives one run: sample → dispatch (serial) → append → re-sample; returns a report and emits `stream-json` `Event`s (ADR-0014) via a sink.
- [ ] Terminal states: `Completed` (no tool calls), `MaxTurns`, `ModelError` (after bounded retry), `Error` (`Fatal`).
- [ ] Pre-send pass guarantees every `tool_use` id has exactly one `tool_result`; abort/mid-batch synthesizes `is_error` results.
- [ ] `Respond` errors become `tool_result{is_error}`; the loop keeps iterating.

**Verification:**
- [ ] Unit tests hitting **each** terminal state via MockProvider scripts; a test asserting transcript validity after a simulated mid-batch abort; a max-turns test.

**Dependencies:** Tasks 3, 4, 5
**Files:** `crates/locode-engine/src/{loop,session,terminal}.rs`, tests
**Scope:** M

### Checkpoint B — full loop reaches every terminal state under MockProvider, zero network. Review before Phase 2.

---

## Phase 2: The grok harness pack + host

## Task 7: `locode-host` side-effect seam
**Description:** The injectable host: path jail, shell exec with limits, fs helpers, shared truncation (ADR-0008).

**Acceptance criteria:**
- [ ] Path resolution under `workspace_root`; `..`/absolute escapes rejected with a soft error.
- [ ] Shell exec (`bash -lc`/`sh -c`) captures stdout+stderr+exit, hard timeout, max-output-byte cap with a truncation marker.
- [ ] Shared `truncate_for_model` post-process applied to tool output before model re-entry.

**Verification:**
- [ ] Unit tests: jail rejects `../etc/passwd`; shell timeout kills a sleeper; output over cap is truncated with marker.

**Dependencies:** Task 3
**Files:** `crates/locode-host/src/{path,shell,fs,truncate}.rs`, tests
**Scope:** M

## Task 8: `locode-packs` — pack framework + grok pack wiring
**Description:** The harness-pack layer (ADR-0012). A `Pack` = a named set of `Tool`s + a system prompt + registration; `--harness` selects one. No re-skin machinery — each pack holds real tools. v0 wires the grok pack.

**Acceptance criteria:**
- [ ] `Pack` abstraction: `name`, a tool set registered into a `Registry`, and the pack's system prompt; `--harness <name>` resolves to a pack.
- [ ] `grok` pack module scaffolded; its tools declare a `ToolKind` tag (for cross-pack A/B alignment) alongside their real grok names.
- [ ] `dispatch` routes the pack's real tool names to its real impls; duplicate-name registration panics at startup.

**Verification:**
- [ ] Unit tests: `--harness grok` builds the expected tool specs (grok's real names/schemas); a client call routes to the grok impl; an unknown `--harness` errors clearly.

**Dependencies:** Task 4
**Files:** `crates/locode-packs/src/{lib,pack,grok/mod}.rs`, tests
**Scope:** M

## Task 9: grok pack — `run_terminal_command` + `read_file`
**Description:** Port Grok Build's terminal + read tools from `xai-grok-tools` onto our `Tool` trait, over the host (behavior P0, exact names/descriptions P1).

**Acceptance criteria:**
- [ ] `run_terminal_command` and `read_file` implement `Tool` with grok's real arg schemas/behavior; go through `locode-host` only.
- [ ] `read_file` records freshness (path+mtime) for later edits; dual output (structured `{path,lines,truncated}` + file-body prompt_text), matching grok's shaping.

**Verification:**
- [ ] Unit tests: the terminal tool runs `echo`; read returns body + truncation note; a mock-provider engine run under `--harness grok` invokes both and produces a valid report.

**Dependencies:** Tasks 6, 7, 8
**Files:** `crates/locode-packs/src/grok/{terminal,read}.rs`, tests
**Scope:** M

## Task 10: grok pack — `write` + `search_replace` (grok's real edit)
**Description:** Port grok's `write` + `search_replace` (exact-string edit). The edit slice — where real bugs live; replicate grok's guardrails faithfully (SPEC §Testing).

**Acceptance criteria:**
- [ ] `write` create/overwrite via host; updates freshness.
- [ ] `search_replace` replicates grok's real behavior: exact + unique match (soft-error with match count otherwise), read-before-edit (except new-file), mtime freshness re-check, reject no-op. Updates freshness after write.

**Verification:**
- [ ] One unit test **per** invariant (each violation → the correct soft error); a happy-path chained edit test.

**Dependencies:** Task 9
**Files:** `crates/locode-packs/src/grok/{write,search_replace}.rs`, tests
**Scope:** M

## Task 11: grok pack — `grep` + dir/glob (ripgrep-backed)
**Description:** Port grok's search tools; ripgrep-backed, resolved through the host (ADR-0011). No hand-rolled walker.

**Acceptance criteria:**
- [ ] `locode-host` exposes a cached `rg` resolver: `LOCODE_RG_PATH` override → host-provided bundled path → bare `rg` on PATH (invoked by name, not a cwd-relative absolute path).
- [ ] grok's `grep` and dir/glob tools implement `Tool` over the resolved `rg` (glob via `rg --files` + filter); results respect the path jail and truncation.
- [ ] If `rg` can't be resolved, both tools return a soft `Respond` error (no silent divergent fallback).

**Verification:**
- [ ] Unit tests with a temp tree: glob finds expected paths; grep matches lines; the resolver honors `LOCODE_RG_PATH` (pointed at a stub); soft-error path when `rg` is unresolvable.

**Dependencies:** Tasks 6, 7, 8
**Files:** `crates/locode-host/src/rg.rs`, `crates/locode-packs/src/grok/{grep,glob}.rs`, tests
**Scope:** M

### Checkpoint C — the grok pack's tools work under the mock provider; edit invariants + jail tested. Review before Phase 3.

---

## Phase 3: Live Anthropic wire + minimal CLI

## Task 12: Anthropic Messages wire impl
**Description:** The one live `Provider` wire (ADR-0007). Correctness of caching/retry/pairing matters most here.

**Acceptance criteria:**
- [ ] Builds the Messages request from `ConversationRequest`; parses response; preserves tool-call ids verbatim; extracts usage.
- [ ] `cache_control` breakpoints: exactly one on last message + ≤4 on system blocks; temperature omitted when thinking is on.
- [ ] Two-tier retry (transport backoff+jitter honoring `Retry-After`; bounded loop-level resample); **429 surfaced** not hammered; context-overflow/quota terminal; 401 → refresh once → retry.
- [ ] Pre-send transcript repair/dedup runs before every request; `LOCODE_BASE_URL`/`LOCODE_API_KEY` env + per-model `{base_url,api_backend,extra_headers}` record honored.

**Verification:**
- [ ] Tests against recorded/fixture responses (no live key in CI): request shape asserts cache-marker count; retry classifies 5xx vs 429 vs terminal; id preservation checked.

**Dependencies:** Task 5
**Files:** `crates/locode-provider/src/anthropic/*.rs`, tests/fixtures
**Scope:** L

## Task 13: grok pack system prompt
**Description:** The grok pack's system prompt, ported from grok's real prompt (minijinja-rendered, grok-sized), with identity branched on headless (design doc §8).

**Acceptance criteria:**
- [ ] Renders grok's identity (autonomous vs interactive branch), cwd/OS/shell/date, and tool guidance referring to the grok pack's real tool names.
- [ ] Rendered length ≈ grok-sized (short); placeholders resolve for the grok pack.

**Verification:**
- [ ] Snapshot test of the rendered grok prompt; headless branch toggles the identity line.

**Dependencies:** Task 8
**Files:** `crates/locode-packs/src/grok/prompt.rs`, templates, tests
**Scope:** S

## Task 14: `locode` facade + `locode-exec` minimal binary
**Description:** Public facade and the minimal headless binary with strict stdout discipline (ADR-0009).

**Acceptance criteria:**
- [ ] `locode` re-exports the driving API (`Session`, harness/provider selection, report types).
- [ ] `locode-exec`: clap flags `--prompt,--cwd,--harness(default grok),--provider(default anthropic),--max-turns(default 30),--output-format {json,text,stream-json}(default json)` (ADR-0014); `json` = the single `result` Report, `stream-json` = the JSONL `Event` stream, `text` = final message; logs on stderr; `#![deny(clippy::print_stdout)]`; exit codes per ADR-0009.
- [ ] Optional `bundle-rg` cargo feature (release-gated, ADR-0011): `build.rs` downloads the pinned static `rg` for the target triple (or copies from `LOCODE_BUNDLE_RG_PATH` for offline/CI), `include_bytes!` embeds it, runtime self-extracts once to a cache dir; resolver falls back to PATH.

**Verification:**
- [ ] `cargo run -p locode-exec -- --prompt "list and summarize this repo"` prints one parseable JSON report; stderr carries logs; a `--provider mock` mode runs in CI without a key.
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
