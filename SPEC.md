# Spec: locode-core

> Status: **Draft (v0, Phase 1 — Specify)**. Living document; update before implementing when a decision changes.
> Rationale and source study live in `~/dev/coding-cli-survey` — primarily
> `survey/06-design-lessons/minimal-headless-rust-agent.md` (the design) and `rust-ci-and-tooling.md`.
> Cross-cutting decisions are recorded as ADRs in [`docs/decisions/`](docs/decisions/).

## Assumptions

These are the assumptions this spec is built on. Correct any that are wrong before implementation begins.

1. **The core is a library, not an application.** Its deliverable is a set of `locode-*` library crates plus a *minimal* headless binary (`locode-exec`) for end-to-end exercise. The full binary — TUI, MCP, richer UX — is built **in this repo as separate crates** layered on the core (ADR-0001 amendment 2026-07-21; `locode-tui` + `locode-app`, see [`SPEC-TUI.md`](SPEC-TUI.md) and ADR-0019); the core crates themselves stay headless.
2. **Primary target model is Claude** (Anthropic Messages wire first; `cache_control` breakpoint caching from day one). OpenAI Chat Completions is the planned second wire but not in v0.
3. **v0 is the `grok` harness pack** — a faithful port of Grok Build's real tools (ADR-0012). Other packs (`codex`/`claude`/`opencode`) and our own `locode` pack are the next milestone — real per-harness implementations, not re-skins.
4. **Single-user, trusted-workspace threat model for v0.** A `workspace_root` path jail (**default**, on the first-class FS tools) + shell timeout/output caps is the security posture. The jail is a **configurable host `PathPolicy`** (`Jailed`, default / `Unrestricted`), skippable via `--dangerously-skip-permissions` (alias `--yolo`) for the harnesses' full-access behavior — the shell caps stay on regardless (ADR-0008 amendment). The shell tool is *not* path-jailed. OS sandboxing (Seatbelt/Landlock/seccomp) is a deferred extension behind the one dispatch door, not a v0 requirement.
5. **Non-streaming model calls in v0.** Buffer each assistant turn fully before dispatching tools. Streaming is an additive optimization, not a second loop.
6. **In-memory sessions.** History lives in memory on the `Session` and persists across `run()` calls — a second `run()` continues the same conversation, with `Init` emitted once per session and one per-run `Report` each (multi-turn continuity, ADR-0016). Durable JSONL session files are deferred.
7. **Rust stable, current pinned toolchain**, `tokio` async runtime, `reqwest` HTTP.

## Objective

Build the **headless engine of a coding agent**: a production-grade, robust Rust core library that owns the classic *sample → dispatch → append → re-sample* loop, exposes a typed tool registry (shell + filesystem + search) whose JSON schemas are derived from the arg types, organized into a selectable **harness pack** (a faithful per-harness toolset), talks to a model through a **provider trait** with one wire implementation, and returns a single structured result — with **no TUI and no interactive permission prompts**.

**Users:** (1) the future `locode-app` (TUI + features) as a library consumer driving the engine programmatically; (2) researchers running headless A/B comparisons of harness packs and provider wires; (3) `locode-exec` as a thin reference consumer; (4) downstream consumers who want **just the tools** — the pack's tool implementations are a reusable library that can be dropped into *their own* harness loop, without using our engine (the `locode-core` facade re-exports the tool surface for this).

**Success looks like:** a caller can drive one agent session to completion against Claude, under the `grok` harness pack, with the engine emitting exactly one machine-readable JSON report — and every architectural extension point (more harness packs, more wires, apply_patch, sandbox, MCP, streaming, compaction) is a seam, not a rewrite.

## Tech Stack

| Concern | Choice |
|---|---|
| Language / runtime | Rust (stable, pinned via `rust-toolchain.toml`), `tokio` |
| Serialization / schema | `serde`, `schemars` (JSON Schema derived from arg types) |
| Errors | `thiserror` |
| HTTP | `reqwest` (with `rustls`) |
| Prompt templating | `minijinja` (per-pack system prompts) |
| CLI (locode-exec only) | `clap` |
| First provider wire | Anthropic Messages |

## Commands

```sh
# Format / lint / test — the mandatory triangle (fail on warnings)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Auto-fix loop
cargo fmt --all
cargo clippy --workspace --all-targets --fix --allow-dirty -- -D warnings

# Run the minimal headless binary (v0)
cargo run -p locode-exec -- "summarize this repo" --harness grok --provider anthropic

# Convenience (justfile)
just check      # fmt-check + clippy + test
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
SPEC.md                  → this file
tasks/                   → plan.md + todo.md (Phase 2/3 output)
crates/
├── locode-protocol/     → conversation model (4-role, ADR-0013), tool call/result, report envelope (pure types, no I/O)
├── locode-tools/        → Tool trait + registry + dispatch + shared primitives (framework; host-agnostic)
├── locode-packs/        → harness packs (faithful per-harness toolsets); one module per harness
├── locode-provider/     → Provider trait + API-agnostic ConversationRequest + Anthropic wire impl
├── locode-host/         → fs/shell/path-jail/truncation/rg-resolution (injectable side-effect seam)
├── locode-engine/       → the sample→dispatch→append loop + Session driving API
├── locode-core/         → thin facade re-exporting the public surface (the bare name `locode` is taken on crates.io)
└── locode-exec/          → minimal headless binary (Codex-exec-style stdout discipline)
```

Dependency direction: `protocol` ← everything; `tools` → `protocol`; `host` → `protocol`; `packs` → `tools` + `host` + `protocol`; `provider` → `protocol`; `engine` → `packs` + `tools` + `provider` + `host` + `protocol`; `locode-core` → all; `locode-exec` → `locode-core`.

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
- **One end-to-end integration test** driving `locode-engine` through a real or recorded Anthropic wire on a trivial task.
- Tests live inline (`#[cfg(test)]`) for unit scope; cross-crate integration under each crate's `tests/`.

## Boundaries

- **Always:** run the fmt+clippy+test triangle before commit; derive tool schemas from types; route every side effect through the one `dispatch` door and the `locode-host` seam (never call `std::fs`/`Command` from a tool body); guarantee every `tool_use` id gets exactly one `tool_result` before the next sample; keep stdout to exactly one JSON document.
- **Ask first:** adding a dependency; changing the report envelope `schema_version` or any public trait signature (`Tool`, `Provider`); changing the crate boundaries; enabling new `[workspace.lints]` denies.
- **Never:** commit secrets/API keys; `println!` from library crates or non-report paths (stdout is sacred — enforce with `#![deny(clippy::print_stdout)]` in `locode-exec`); bury allow/deny policy inside individual tools; leave a `tool_use` unpaired; introduce a second, throwaway loop for headless.

## Success Criteria (v0)

1. `locode-engine` runs a full session (sample → dispatch → append → terminal) against Claude via the Anthropic Messages wire, driven as a library API.
2. The `grok` pack's tools — `run_terminal_cmd`, `read_file`, `search_replace`, `grep`, and `list_dir` — ported **faithfully** from `xai-grok-tools` (real names + behavior; **no standalone `write`** — grok creates files via `search_replace` with an empty `old_string`; **`list_dir` is grok's fs walker**, not an rg-glob — ADR-0011 amendment), schemas derived from arg types, selected via `--harness grok`.
3. The grok pack enforces grok's **real `search_replace` guardrails**: exact + unique match (soft error with match count otherwise) and reject-no-op at runtime; read-before-edit is grok's prompt/contract expectation (grok does **not** do a runtime mtime-freshness check, so neither do we — faithful mimicry). Path jail rejects `..` escapes; shell honors a hard timeout + output byte cap with a truncation marker.
4. Every tool failure is a soft `tool_result{is_error}`; a pre-send pass guarantees transcript validity (no dangling/duplicate tool results); an explicit `max_turns` ceiling terminates cleanly.
5. `locode-exec` emits **exactly one** JSON report on stdout (stamping `harness` + `api_schema`), all diagnostics on stderr, exit 0 on clean terminal state / non-zero on fatal.
6. The mandatory CI triangle is green; the loop is covered by mock-provider unit tests.
7. Extension seams exist but are unimplemented: additional harness packs (`codex`/`claude`/`opencode`/`locode`), `apply_patch` (JSON-string framing), a second `Provider` wire, parallel tools, compaction, sandbox, MCP.

## Open Questions

Carried from the design doc §12, minus what we've now decided (wire = Anthropic; v0 harness = the `grok` pack per ADR-0012; workspace layout; in-repo minimal binary; search = ripgrep per ADR-0011). Still genuinely undecided:

1. ~~**Edit strictness**~~ — **Resolved (faithful mimicry):** the grok pack reproduces grok's real `search_replace` — exact + unique match + reject-no-op at runtime, read-before-edit via contract, **no** runtime mtime-freshness store (grok has none). Tolerant replacers are an OpenCode-pack concern; exact-string is grok's model.
2. **When to add `apply_patch`** — with the `codex` pack (next milestone), delivered as a JSON-string patch arg on the Anthropic wire (freeform-grammar delivery deferred to a Responses wire).
3. **Schema-constrained task answers** (`--json-schema`) — native `response_format` first with a `StructuredOutput`-tool fallback; **envelope-only for v0 (deferred, confirmed).** Also open: verifying whether Anthropic and OpenAI accept the *same* derived JSON Schema (we assume yes → a single shared normalization helper, not per-wire); needs a verification pass before the wire relies on it.
4. **Session durability** — when do ephemeral runs need JSONL transcript persistence?
5. ~~**facade surface**~~ — **Resolved:** `locode-core` re-exports the driving API — including custom-provider
   injection via `ProviderRegistry` (ADR-0015; `locode-exec` is a library + trivial binary
   so downstream binaries register their own wires) — (`Session`, `EngineConfig`, report/event types, provider + pack selection) **and the full tool surface** (`Tool`, `Registry`, `dispatch`, `ToolCtx`, `ToolOutput`, `ToolSpec`, and the pack's concrete tools). A **first-class goal**: downstream consumers can use our tools inside *their own* harness loop without our engine (see Users #4). Widen further as `locode-app` needs.

## Decisions of record

| ADR | Decision |
|---|---|
| [0001](docs/decisions/ADR-0001-headless-core-scope.md) | Headless-only core library; no TUI/interactive prompts in this repo |
| [0002](docs/decisions/ADR-0002-multi-crate-workspace.md) | Multi-crate `locode-*` Cargo workspace |
| [0003](docs/decisions/ADR-0003-typed-tool-contract.md) | Typed `Tool` contract, schemars-derived schemas, dual `output`/`prompt_text` |
| [0004](docs/decisions/ADR-0004-error-taxonomy-and-pairing.md) | Soft/fatal error taxonomy + strict tool_use/tool_result pairing |
| [0005](docs/decisions/ADR-0005-agent-loop.md) | Sample→dispatch→append loop; non-streaming, serial-first; explicit max-turns |
| [0006](docs/decisions/ADR-0006-dialects-and-edit-encoding.md) | ~~Dialect packs over one registry~~ (superseded by 0012) |
| [0012](docs/decisions/ADR-0012-harness-packs.md) | Harness packs — faithful per-harness toolsets; `grok` pack first |
| [0013](docs/decisions/ADR-0013-conversation-protocol.md) | Conversation protocol — 4-role, Anthropic-shaped content blocks |
| [0014](docs/decisions/ADR-0014-streaming-event-protocol.md) | Streaming event protocol (`stream-json`) — self-sufficient trace source |
| [0007](docs/decisions/ADR-0007-provider-trait.md) | `Provider` trait over API-agnostic request; Anthropic Messages wire first |
| [0008](docs/decisions/ADR-0008-dispatch-door-and-path-jail.md) | One dispatch door + workspace path jail as v0 security posture |
| [0009](docs/decisions/ADR-0009-headless-io-contract.md) | Single JSON report on stdout; diagnostics on stderr |
| [0010](docs/decisions/ADR-0010-rust-tooling-baseline.md) | Rust tooling/CI baseline (pinned toolchain, fmt+clippy-deny+test) |
| [0011](docs/decisions/ADR-0011-search-ripgrep-bundling.md) | Search uses ripgrep (host-resolved, bundled at packaging) |
