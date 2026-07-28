# Spec: locode-core

> Status: **v0 delivered** (0.1.x shipped) — living document. The core architecture below still
> holds; features layered on since (the TUI, a second wire, streaming) are noted inline, and live
> task status is in [`tasks/tracker.md`](tasks/tracker.md). Update before implementing when a decision changes.
> Rationale and source study live in `~/dev/coding-cli-survey` — primarily
> `survey/06-design-lessons/minimal-headless-rust-agent.md` (the design) and `rust-ci-and-tooling.md`.
> Cross-cutting decisions are recorded as ADRs in [`docs/decisions/`](docs/decisions/).

## Assumptions

These are the assumptions this spec is built on. Correct any that are wrong before implementation begins.

1. **The core is a library, not an application.** Its deliverable is a set of `locode-*` library crates. The full agent binary (`locode` = `locode-app` → `locode-tui`) is built **in this repo** layered on the core (ADR-0001/0019 amendments): the interactive TUI by default and a headless one-shot under `-p`/`--print` (Task 28). The standalone `locode-exec` binary has been **removed** (retired as a shipped binary 2026-07-22; its trivial binary target deleted 2026-07-23): `locode -p` is the shipped headless path, and `locode-exec` remains a headless-runner **library** (`run_headless` + `main_with`) feeding it — kept **standalone** (deliberately *not* collapsed into `locode-tui`, ADR-0019 amendment) so headless-only consumers can depend on it without pulling in the TUI. The core crates themselves stay headless.
2. **Primary target model is Claude** (Anthropic Messages wire first; `cache_control` breakpoint caching from day one). The **second wire shipped is OpenAI Responses** (`openai-responses`; stateless, freeform tools, encrypted-reasoning replay); an OpenAI Chat Completions wire remains deferred.
3. **v0 is the `grok` harness pack** — a faithful port of Grok Build's real tools (ADR-0012). The **`claude` pack has since landed** (Task 20: `Bash`/`Read`/`Edit`/`Write`/`Glob`/`Grep` + the read-before-edit/staleness gate + byte-exact prompt, ADR-0012 amendment 2026-07-24). The remaining packs (`codex`/`opencode`) and our own `locode` pack are the next milestone — real per-harness implementations, not re-skins.
4. **Single-user, trusted-workspace threat model for v0.** A `workspace_root` path jail (**default**, on the first-class FS tools) + shell timeout/output caps is the security posture. The jail is a **configurable host `PathPolicy`** (`Jailed`, default / `Unrestricted`), skippable via `--dangerously-skip-permissions` (alias `--yolo`) for the harnesses' full-access behavior — the shell caps stay on regardless (ADR-0008 amendment). The shell tool is *not* path-jailed. OS sandboxing (Seatbelt/Landlock/seccomp) is a deferred extension behind the one dispatch door, not a v0 requirement.
5. **Streaming is an additive layer over the loop, not a second loop.** v0 buffered each assistant turn fully; live token streaming has since **shipped** ([ADR-0021](docs/decisions/ADR-0021-live-token-streaming.md), Task 29 complete 2026-07-22) as an opt-in layer — tool dispatch still gates on the finalized whole completion, and the headless path defaults to non-streaming (opt in with `--stream`).
6. **In-memory sessions.** History lives in memory on the `Session` and persists across `run()` calls — a second `run()` continues the same conversation, with `Init` emitted once per session and one per-run `Report` each (multi-turn continuity, ADR-0016). Durable JSONL session files are deferred.
7. **Rust stable, current pinned toolchain**, `tokio` async runtime, `reqwest` HTTP.

## Objective

Build the **headless engine of a coding agent**: a production-grade, robust Rust core library that owns the classic *sample → dispatch → append → re-sample* loop, exposes a typed tool registry (shell + filesystem + search) whose JSON schemas are derived from the arg types, organized into a selectable **harness pack** (a faithful per-harness toolset), talks to a model through a **provider trait** with pluggable wire implementations, and returns a single structured result — with **no TUI and no interactive permission prompts**.

**Users:** (1) the future `locode-app` (TUI + features) as a library consumer driving the engine programmatically; (2) researchers running headless A/B comparisons of harness packs and provider wires; (3) headless-only consumers who depend on `locode-exec` (the `run_headless`/`main_with` library entry) or `locode-core` **without** the TUI; (4) downstream consumers who want **just the tools** — the pack's tool implementations are a reusable library that can be dropped into *their own* harness loop, without using our engine (the `locode-core` facade re-exports the tool surface for this).

**Success looks like:** a caller can drive one agent session to completion against Claude, under the `grok` harness pack, with the engine emitting exactly one machine-readable JSON report — and every architectural extension point (more harness packs, more wires, apply_patch, sandbox, MCP, streaming, compaction) is a seam, not a rewrite.

## Tech Stack

| Concern | Choice |
|---|---|
| Language / runtime | Rust (stable, pinned via `rust-toolchain.toml`), `tokio` |
| Serialization / schema | `serde`, `schemars` (JSON Schema derived from arg types) |
| Errors | `thiserror` |
| HTTP | `reqwest` (with `rustls`) |
| Prompt templating | `minijinja` (per-pack system prompts) |
| CLI (`locode-app`/`-tui` + `locode-exec`) | `clap` |
| First provider wire | Anthropic Messages |

## Commands

```sh
# Format / lint / test / doc — the mandatory four-part gate (fail on warnings)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps   # catches broken intra-doc links

# Auto-fix loop
cargo fmt --all
cargo clippy --workspace --all-targets --fix --allow-dirty -- -D warnings

# Run the headless path (one-shot)
cargo run -p locode-app -- -p "summarize this repo" --harness grok --api-schema anthropic

# Convenience (justfile)
just check      # fmt-check + clippy + test + doc
just fix        # fmt + clippy --fix
```

## Project Structure

A Cargo workspace of small `locode-*` crates (see [ADR-0002](docs/decisions/ADR-0002-multi-crate-workspace.md)). `locode-core` is the **workspace/repo name**, not a crate.

```
Cargo.toml               → [workspace] + [workspace.lints]
rust-toolchain.toml      → pinned stable + rustfmt + clippy
rustfmt.toml, clippy.toml, deny.toml (later)
justfile                 → dev commands
docs/decisions/          → ADRs
docs/research/           → source studies of the four harnesses (each stamped with when it was last verified)
docs/autonomous-workflow.md → the loop an autonomous workstream runs; per-workstream companions beside it
SPEC.md                  → this file
META-AGENTS.md           → which document answers which question, and the findings that changed how we work
tasks/                   → tracker.md (status) + plans/ (per-task design records) + audits/
crates/
├── locode-protocol/     → conversation model (4-role, ADR-0013), tool call/result, report envelope (pure types, no I/O)
├── locode-tools/        → Tool trait + registry + dispatch + shared primitives (framework; host-agnostic)
├── locode-packs/        → harness packs (faithful per-harness toolsets); one module per harness
├── locode-provider/     → Provider trait + API-agnostic ConversationRequest + Anthropic wire impl
├── locode-host/         → fs/shell/path-jail/truncation/rg-resolution (injectable side-effect seam)
├── locode-instructions/ → project instructions (`AGENTS.md`): discovery + the injected reminder
├── locode-skills/      → agent skills (`SKILL.md`): discovery + the skills listing
├── locode-engine/       → the sample→dispatch→append loop + Session driving API
├── locode-core/         → thin facade re-exporting the public surface (the bare name `locode` is taken on crates.io)
├── locode-exec/         → headless runner **library** (`run_headless` + `main_with`, Codex-exec-style stdout discipline); binary target removed 2026-07-23 (library only; not collapsed into `locode-tui`)
├── locode-tui/          → TUI components + interactive app + `-p` headless dispatch (ADR-0019)
└── locode-app/          → flag-free product binary — the shipped `locode`
```

Dependency direction: `protocol` ← everything; `tools` → `protocol`; `host` → `protocol`; `packs` → `tools` + `host` + `protocol`; `provider` → `protocol`; `engine` → `packs` + `tools` + `provider` + `host` + `protocol`; `locode-core` → all; `locode-exec` → `locode-core`; `locode-tui` → `locode-exec` + `locode-core`; `locode-app` → `locode-tui`.

## Code Style

Standard `rustfmt`; `clippy -D warnings`. Author tools with concrete types; erase to `dyn` only at the registry boundary. The single most important type — the tool contract ([ADR-0003](docs/decisions/ADR-0003-typed-tool-contract.md)):

```rust
/// A tool authored against concrete types; the wire schema is *derived*, never hand-written.
#[async_trait]
trait Tool: Send + Sync {
    type Args: DeserializeOwned + JsonSchema + Send;
    type Output: Serialize + ToolOutput + Send; // ToolOutput::to_prompt_text(&self) -> String

    fn kind(&self) -> ToolKind;                 // classification tag for cross-pack A/B (Shell/Read/Write/Edit/Glob/Grep)
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value { schema_for!(Self::Args) } // default

    async fn run(&self, ctx: &ToolCtx, args: Self::Args) -> Result<Self::Output, ToolError>;
}

/// Two variants only. Default everything to Respond so the model can recover.
enum ToolError {
    Respond(String), // soft: bad args, not-found, command failed, timeout → tool_result{is_error}
    Fatal(String),   // hard: transcript unrecoverable → abort turn, non-zero exit
}
```

Conventions: `kind()` is a classification tag for cross-pack A/B alignment — not the wire name (each pack tool carries its harness's real name); a tool result has two faces (`output` for the JSON report, `to_prompt_text()` for model history); `ToolCtx` stays small (`cwd`, `call_id`, `workspace_root`, `cancel`) — no god-object context.

## Testing Strategy

- **Framework:** `cargo test` (workspace). Adopt `cargo-nextest` only when subprocess tests get flaky/slow.
- **Highest-leverage surface:** a **mock `Provider`** that emits scripted `tool_calls`, so the loop is unit-tested with zero API spend. The loop — transcript pairing, soft/fatal handling, max-turns, abort repair — is where the subtle bugs live.
- **Golden test** on the report-envelope JSON shape (freeze `schema_version: 1`).
- **Per-crate unit tests**, especially the grok-faithful `edit` guards (as built: exact + unique match, reject no-op — grok has **no** runtime read-before-edit or mtime-freshness check, so we don't either) and the path jail.
- **Live wire smokes, manual and `#[ignore]`d by default** so CI never touches the network: `locode-provider/tests/anthropic_live_smoke.rs` and `responses_live_smoke.rs` prove thinking-replay, cache survival, and error classification against a real backend. They cover the **provider**, not the whole engine — an end-to-end `locode-engine` run over a *recorded* wire is still an open gap, not a shipped test. Do not read this line as coverage we have.
- Tests live inline (`#[cfg(test)]`) for unit scope; cross-crate integration under each crate's `tests/`.

## Boundaries

- **Always:** run the four-part gate (`fmt · clippy · test · doc`) before merge; derive tool schemas from types; route every side effect through the one `dispatch` door and the `locode-host` seam (never call `std::fs`/`Command` from a tool body); guarantee every `tool_use` id gets exactly one `tool_result` before the next sample; keep stdout to exactly one JSON document.
- **Ask first:** adding a dependency; changing the report envelope `schema_version` or any public trait signature (`Tool`, `Provider`); changing the crate boundaries; enabling new `[workspace.lints]` denies.
- **Never:** commit secrets/API keys; `println!` from library crates or non-report paths (stdout is sacred — enforce with `#![deny(clippy::print_stdout)]` in `locode-exec`); bury allow/deny policy inside individual tools; leave a `tool_use` unpaired; introduce a second, throwaway loop for headless.

**Fidelity boundary (what "faithful per-harness" means — [ADR-0023](docs/decisions/ADR-0023-fidelity-boundary-and-agents-md-loading.md)).** A pack faithfully reproduces exactly two surfaces of its harness: its **system prompt** and its **tool set** (the six core `ToolKind`s — names, schemas, behavior, caps, guardrails). Everything loop-adjacent — project-instruction loading (`AGENTS.md`), skills, reminder/context injection, compaction, subagents — is **shared, single-implementation engine machinery, identical for every pack**, not reproduced per harness. So "faithful per-harness toolset" throughout this spec is scoped to *tools + prompt*; the context/loop machinery is one shared best-of design. Injected framing (project instructions, reminders) is authored as **`User`**-role `<system-reminder>` content, not `Developer` (ADR-0013 amendment 2026-07-23).

## Success Criteria (v0)

1. `locode-engine` runs a full session (sample → dispatch → append → terminal) against Claude via the Anthropic Messages wire, driven as a library API.
2. The `grok` pack's tools — `run_terminal_cmd`, `read_file`, `search_replace`, `grep`, and `list_dir` — ported **faithfully** from `xai-grok-tools` (real names + behavior; **no standalone `write`** — grok creates files via `search_replace` with an empty `old_string`; **`list_dir` is grok's fs walker**, not an rg-glob — ADR-0011 amendment), schemas derived from arg types, selected via `--harness grok`.
3. The grok pack enforces grok's **real `search_replace` guardrails**: exact + unique match (soft error with match count otherwise) and reject-no-op at runtime; read-before-edit is grok's prompt/contract expectation (grok does **not** do a runtime mtime-freshness check, so neither do we — faithful mimicry). Path jail rejects `..` escapes; shell honors a hard timeout + output byte cap with a truncation marker.
4. Every tool failure is a soft `tool_result{is_error}`; a pre-send pass guarantees transcript validity (no dangling/duplicate tool results); an explicit `max_turns` ceiling terminates cleanly.
5. The headless path (`locode -p`, formerly the standalone `locode-exec` binary) emits **exactly one** JSON report on stdout (stamping `harness` + `api_schema`), all diagnostics on stderr, exit 0 on clean terminal state / non-zero on fatal.
6. The mandatory four-part CI gate (`fmt · clippy · test · doc`) is green; the loop is covered by mock-provider unit tests.
7. Extension seams exist — some now filled. **Shipped since v0** (as of 2026-07-27): a second `Provider` wire (OpenAI Responses) and streaming (ADR-0021); the **`claude`** (Task 20) and **`codex`** (Task 19) harness packs, the latter carrying `apply_patch`; the interactive app (ADR-0019/0022); the `~/.locode` home with settings + resumable traces (ADR-0024); agent skills (ADR-0025); slash commands (ADR-0026); mid-run user input (ADR-0028). **Still unimplemented**: our own **`locode` best-of pack** (the last pack — the `opencode` port was cancelled 2026-07-24), background tasks + subagents, parallel tools (ADR-0027, draft), compaction, sandbox, MCP.

## Open Questions

Carried from the design doc §12, minus what we've now decided (wire = Anthropic; v0 harness = the `grok` pack per ADR-0012; workspace layout; in-repo minimal binary; search = ripgrep per ADR-0011). Still genuinely undecided:

1. ~~**Edit strictness**~~ — **Resolved (faithful mimicry):** the grok pack reproduces grok's real `search_replace` — exact + unique match + reject-no-op at runtime, read-before-edit via contract, **no** runtime mtime-freshness store (grok has none). Tolerant replacers are an OpenCode-pack concern; exact-string is grok's model.
2. ~~**When to add `apply_patch`**~~ — **Resolved (shipped 2026-07-24, Task 19 slice 2):** it landed with the `codex` pack as a JSON-string patch arg on the Anthropic wire; freeform-grammar delivery over the Responses wire remains deferred.
3. **Schema-constrained task answers** (`--json-schema`) — native `response_format` first with a `StructuredOutput`-tool fallback; **envelope-only for v0 (deferred, confirmed).** Also open: verifying whether Anthropic and OpenAI accept the *same* derived JSON Schema (we assume yes → a single shared normalization helper, not per-wire); needs a verification pass before the wire relies on it.
4. ~~**Session durability**~~ — **Resolved (shipped 2026-07-24, Task 31, [ADR-0024](docs/decisions/ADR-0024-locode-home-settings-and-traces.md)):** every run appends a JSONL trace under `~/.locode/sessions/`, with `--continue`/`--resume` reading them and `--no-session-persistence` opting out.
5. ~~**facade surface**~~ — **Resolved:** `locode-core` re-exports the driving API — including custom-provider
   injection via `ProviderRegistry` (ADR-0015; `locode-exec` is a **library** — downstream
   binaries call `main_with(registry)` to register their own wires) — (`Session`, `EngineConfig`, report/event types, provider + pack selection) **and the full tool surface** (`Tool`, `Registry`, `dispatch`, `ToolCtx`, `ToolOutput`, `ToolSpec`, and the pack's concrete tools). A **first-class goal**: downstream consumers can use our tools inside *their own* harness loop without our engine (see Users #4). Widen further as `locode-app` needs.

## Decisions of record

The canonical, always-current index of every ADR — with **status** and supersession notes —
is [`docs/decisions/README.md`](docs/decisions/README.md). It is the single list; this spec
does not duplicate it.
