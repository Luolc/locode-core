# locode-core v0 — Task List

Detailed, ordered tasks for [`plan.md`](plan.md). Each clears the Definition of Done in the plan.
Sizes: XS=1 file · S=1–2 · M=3–5 · L=5–8 (break down if larger).

---

## Phase 0: Scaffolding

## Task 1: Cargo workspace + crate skeletons + toolchain pin
**Description:** Create the `locode-*` workspace with empty compiling crate skeletons and the pinned toolchain + lint configs (ADR-0002, ADR-0010).

**Acceptance criteria:**
- [ ] `Cargo.toml` `[workspace]` lists all 8 crates under `crates/`; each crate compiles as an empty lib (`locode-exec` as a bin).
- [ ] `rust-toolchain.toml` pins current stable + `rustfmt`,`clippy`; `rustfmt.toml`, `clippy.toml`, `[workspace.lints]` (`unused_must_use="deny"`) present.
- [ ] Dependency directions from the plan graph are wired (no cycles).

**Verification:**
- [ ] `cargo build --workspace` succeeds; `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean.

**Dependencies:** None
**Files:** `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `crates/*/Cargo.toml`, `crates/*/src/lib.rs|main.rs`
**Scope:** M

## Task 2: CI + justfile
**Description:** Single GitHub Actions job running the mandatory triangle, plus developer `justfile` (ADR-0010).

**Acceptance criteria:**
- [ ] `.github/workflows/ci.yml`: checkout → toolchain from file → `Swatinem/rust-cache` → fmt-check, clippy `-D warnings`, test; runs on PR + push to main.
- [ ] `justfile` with `fmt`, `fmt-check`, `clippy`, `fix`, `test`, `check`.
- [ ] `Cargo.lock` committed.

**Verification:**
- [ ] `just check` green locally; CI green on a pushed branch.

**Dependencies:** Task 1
**Files:** `.github/workflows/ci.yml`, `justfile`, `Cargo.lock`
**Scope:** S

### Checkpoint A — empty workspace compiles; `just check` green in CI. Review before Phase 1.

---

## Phase 1: Core spine (mock provider, zero API spend)

## Task 3: `locode-protocol` types + report envelope
**Description:** Pure types shared by all crates: the minimal history model, tool call/result, and the JSON report envelope (ADR-0009).

**Acceptance criteria:**
- [ ] History enum: `System`/`User`/`Assistant{text,tool_calls}`/`Tool{call_id,content,is_error}`.
- [ ] `ToolCall{id,name,args}`, report envelope with `schema_version:1`, `status`, `dialect`, `provider`, `final_message`, `structured_output`, `turns`, `tool_calls[]`, `usage`, `session_id`, `error`.
- [ ] `status ∈ {completed,max_turns,model_error,error}` serializes to the exact strings in ADR-0009.

**Verification:**
- [ ] Golden test: a fixed report serializes to a committed JSON snapshot (freezes the envelope shape).

**Dependencies:** Task 1
**Files:** `crates/locode-protocol/src/*.rs`, `crates/locode-protocol/tests/envelope_golden.rs`
**Scope:** M

## Task 4: `locode-tools` contract + registry + dispatch door
**Description:** The most important type in the system: the typed `Tool` trait, canonical `ToolKind`, error taxonomy, dyn-erasure, and the single `dispatch` door (ADR-0003, ADR-0004, ADR-0008).

**Acceptance criteria:**
- [ ] `Tool` trait with `Args: DeserializeOwned+JsonSchema`, `Output: Serialize+ToolOutput`, `kind()`, `description()`, derived `parameters_schema()`, async `run()`.
- [ ] `ToolError{Respond,Fatal}`; `ToolCtx{cwd,call_id,workspace_root,cancel}`; `ToolOutput::to_prompt_text()`.
- [ ] `DynTool` erasure (JSON decode → run → re-serialize); `Registry` with `dispatch(name,raw_args,ctx)` returning both a history `tool_result` and a report record.
- [ ] Duplicate-name registration panics at startup; unknown tool + bad args are **soft** (`Respond`).

**Verification:**
- [ ] Unit tests: schema derived matches `Args`; bad-args → `Respond`; a trivial echo tool round-trips output/prompt_text; duplicate registration panics.

**Dependencies:** Task 3
**Files:** `crates/locode-tools/src/{tool,registry,error,ctx}.rs`, tests
**Scope:** M

## Task 5: `locode-provider` trait + MockProvider
**Description:** The API-agnostic request/response types and a scripted mock provider — the zero-spend test seam for the loop (ADR-0007).

**Acceptance criteria:**
- [ ] `Provider` trait `async fn complete(&self,&ConversationRequest)->Result<Completion,ProviderError>`.
- [ ] `ConversationRequest{system,messages,tools,sampling,cache_hint}`; `Completion{text,tool_calls,usage,stop}`.
- [ ] `MockProvider` returns a scripted sequence of `Completion`s (incl. tool_calls then a final text turn).
- [ ] Reusable partial-JSON tool-arg accumulation helper (raw string per index, parse at stop), unit-tested standalone.

**Verification:**
- [ ] Unit tests: mock emits scripted turns in order; partial-JSON helper assembles fragmented args correctly.

**Dependencies:** Task 3
**Files:** `crates/locode-provider/src/{trait,request,mock,assemble}.rs`, tests
**Scope:** M

## Task 6: `locode-engine` loop + Session API
**Description:** The sample→dispatch→append loop with all terminal conditions and transcript hygiene, driven by MockProvider + trivial tools (ADR-0005, ADR-0004). Highest-leverage test surface.

**Acceptance criteria:**
- [ ] `Session`/`Engine` library API drives one run: sample → dispatch (serial) → append → re-sample; returns a report.
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

## Phase 2: Real tools + grok dialect + host

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

## Task 8: `locode-dialects` + EditEncoding + grok table
**Description:** The re-skin layer over one registry; grok house dialect; `EditEncoding` enum with only `ExactString` built (ADR-0006).

**Acceptance criteria:**
- [ ] `Dialect{enabled,tool_name,param_rename,edit_encoding,describe}`; `EditEncoding{ExactString, /*ApplyPatchFreeform,AnchorEdits reserved*/}`.
- [ ] grok table: `run_terminal_command`/`read_file`/`write`/`search_replace`/`glob`/`grep`, snake_case, `ExactString`.
- [ ] `list_specs(dialect)` re-skins derived schemas; `dispatch` reverse-maps client name+params → `ToolKind` + canonical schema.

**Verification:**
- [ ] Unit tests: grok `list_specs` emits expected names/params; a client call under grok names round-trips to the canonical impl.

**Dependencies:** Task 4
**Files:** `crates/locode-dialects/src/{dialect,encoding,grok}.rs`, tests
**Scope:** M

## Task 9: `shell` + `read` tools
**Description:** First two canonical tool impls over the host, registered under grok.

**Acceptance criteria:**
- [ ] `Shell{command,timeout?}` and `Read{path,offset?,limit?}` implement `Tool`; go through `locode-host` only.
- [ ] `Read` records freshness (path+mtime) for later `edit`; dual output (structured `{path,lines,truncated}` + file-body prompt_text).

**Verification:**
- [ ] Unit tests: shell runs `echo`; read returns body + truncation note; a mock-provider engine run invokes both and produces a valid report.

**Dependencies:** Tasks 6, 7, 8
**Files:** `crates/locode-tools/src/impls/{shell,read}.rs`, tests
**Scope:** M

## Task 10: `write` + `edit` (ExactString) with the four invariants
**Description:** The edit slice — where real bugs live. Enforce every guardrail (ADR-0006, SPEC §Testing).

**Acceptance criteria:**
- [ ] `Write{path,content}` create/overwrite via host; updates freshness.
- [ ] `Edit` (ExactString: `old_string`/`new_string`/`replace_all`) enforces: (1) read-before-edit (except new-file empty `old_string`), (2) exact + unique match (soft-error with match count otherwise), (3) mtime freshness re-check, (4) reject `old_string==new_string`. Updates freshness after write.

**Verification:**
- [ ] One unit test **per** invariant (each violation → the correct soft error); a happy-path chained edit test.

**Dependencies:** Task 9
**Files:** `crates/locode-tools/src/impls/{write,edit}.rs`, tests
**Scope:** M

## Task 11: `glob` + `grep` (ripgrep-backed)
**Description:** Search tools backed by ripgrep, resolved through the host (ADR-0011). No hand-rolled walker.

**Acceptance criteria:**
- [ ] `locode-host` exposes a cached `rg` resolver: `LOCODE_RG_PATH` override → host-provided bundled path → bare `rg` on PATH (invoked by name, not a cwd-relative absolute path).
- [ ] `Glob{pattern,path?}` (via `rg --files` + glob filter) and `Grep{pattern,path?,glob?}` implement `Tool` over the resolved `rg`; results respect the path jail and truncation.
- [ ] If `rg` can't be resolved, both tools return a soft `Respond` error (no silent divergent fallback).

**Verification:**
- [ ] Unit tests with a temp tree: glob finds expected paths; grep matches lines; the resolver honors `LOCODE_RG_PATH` (pointed at a stub); soft-error path when `rg` is unresolvable.

**Dependencies:** Tasks 6, 7, 8
**Files:** `crates/locode-host/src/rg.rs`, `crates/locode-tools/src/impls/{glob,grep}.rs`, tests
**Scope:** M

### Checkpoint C — six tools work under grok via mock provider; edit invariants + jail tested. Review before Phase 3.

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

## Task 13: system prompt
**Description:** Minijinja-rendered, grok-sized prompt whose tool names track the active dialect and whose identity branches on headless (design doc §8).

**Acceptance criteria:**
- [ ] Template renders identity (autonomous vs interactive branch), cwd/OS/shell/date, and tool guidance referring to tools **by dialect name**.
- [ ] Rendered length ≈ grok-sized (short); placeholders resolve for grok.

**Verification:**
- [ ] Snapshot test of the rendered grok prompt; headless branch toggles the identity line.

**Dependencies:** Task 8
**Files:** `crates/locode-engine/src/prompt/*.rs`, templates, tests
**Scope:** S

## Task 14: `locode` facade + `locode-exec` minimal binary
**Description:** Public facade and the minimal headless binary with strict stdout discipline (ADR-0009).

**Acceptance criteria:**
- [ ] `locode` re-exports the driving API (`Session`, dialect/provider selection, report types).
- [ ] `locode-exec`: clap flags `--prompt,--cwd,--dialect(default grok),--provider(default anthropic),--max-turns(default 30)`; emits exactly one JSON report on stdout; logs on stderr; `#![deny(clippy::print_stdout)]`; exit codes per ADR-0009.
- [ ] Optional `bundle-rg` cargo feature (release-gated, ADR-0011): `build.rs` downloads the pinned static `rg` for the target triple (or copies from `LOCODE_BUNDLE_RG_PATH` for offline/CI), `include_bytes!` embeds it, runtime self-extracts once to a cache dir; resolver falls back to PATH.

**Verification:**
- [ ] `cargo run -p locode-exec -- --prompt "list and summarize this repo"` prints one parseable JSON report; stderr carries logs; a `--provider mock` mode runs in CI without a key.
- [ ] `cargo build -p locode-exec --features bundle-rg --release` yields a binary that resolves `rg` with an empty PATH.

**Dependencies:** Tasks 6, 12, 13
**Files:** `crates/locode/src/lib.rs`, `crates/locode-exec/src/main.rs`, `crates/locode-exec/build.rs`, tests
**Scope:** M (L with `bundle-rg`)

### Checkpoint D — end-to-end run against Claude prints one JSON report. **v0 success criteria met.** Review.

---

## Phase 4: Remaining dialects → first A/B

## Task 15: `claude` + `opencode` dialects
**Description:** Two more re-skins over the same six impls (ADR-0006). (`codex` waits on apply_patch P1.)

**Acceptance criteria:**
- [ ] claude: `Bash`/`Read`/`Write`/`Edit`, PascalCase, ExactString (`old_string`/`new_string`/`replace_all`).
- [ ] opencode: `bash`/`read`/`write`/`edit`, lowercase, camelCase args via `param_rename` (`filePath`/`oldString`).

**Verification:**
- [ ] Unit tests: each dialect's `list_specs` names/params; a client call under each round-trips to the canonical impl.

**Dependencies:** Task 8
**Files:** `crates/locode-dialects/src/{claude,opencode}.rs`, tests
**Scope:** S

## Task 16: first A/B run
**Description:** The payoff — run one task under two dialects and compare.

**Acceptance criteria:**
- [ ] Same prompt runs under `--dialect grok` and `--dialect claude`; both reports stamp their dialect.
- [ ] A short doc note captures the trajectory/token/edit-success diff.

**Verification:**
- [ ] Two reports produced; diff recorded in `docs/` or a scratch note.

**Dependencies:** Tasks 14, 15
**Files:** `docs/ab-notes.md` (or scratch), no code changes required
**Scope:** XS

### Checkpoint E — two dialects, one task, six shared impls; A/B is mechanical. v0 complete.

---

## Deferred (reserved seams, not v0)
`EditEncoding::ApplyPatchFreeform` + `codex` dialect · OpenAI Chat Completions wire · parallel tool
batches (RwLock read/write) · compaction · OS sandbox · MCP · streaming events · `--json-schema`
answers · JSONL session durability · multi-platform `rg` bundle matrix + macOS notarization/sidecar (packaging, ADR-0011).
