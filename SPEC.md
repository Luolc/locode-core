# Spec: locode-core

> Status: **Draft (v0, Phase 1 — Specify)**. Living document; update before implementing when a decision changes.
> Rationale and source study live in `~/dev/coding-cli-survey` — primarily
> `survey/06-design-lessons/minimal-headless-rust-agent.md` (the design) and `rust-ci-and-tooling.md`.
> Cross-cutting decisions are recorded as ADRs in [`docs/decisions/`](docs/decisions/).

## Assumptions

These are the assumptions this spec is built on. Correct any that are wrong before implementation begins.

1. **locode-core is a library, not an application.** Its deliverable is a set of `locode-*` library crates plus a *minimal* headless binary (`locode-cli`) for end-to-end exercise. The full binary — TUI, MCP, richer UX — lives in a separate future repo (`locode-app`) that depends on these crates.
2. **Primary target model is Claude** (Anthropic Messages wire first; `cache_control` breakpoint caching from day one). OpenAI Chat Completions is the planned second wire but not in v0.
3. **House dialect is `grok`** (snake_case, first-class FS tools). `claude`/`codex`/`opencode` are re-skins added after the house dialect works.
4. **Single-user, trusted-workspace threat model for v0.** A `workspace_root` path jail + shell timeout/output caps is the security posture; OS sandboxing (Seatbelt/Landlock/seccomp) is a deferred extension behind the one dispatch door, not a v0 requirement.
5. **Non-streaming model calls in v0.** Buffer each assistant turn fully before dispatching tools. Streaming is an additive optimization, not a second loop.
6. **Ephemeral sessions in v0.** History lives in memory for one run; durable JSONL session files are deferred.
7. **Rust stable, current pinned toolchain**, `tokio` async runtime, `reqwest` HTTP.

## Objective

Build the **headless engine of a coding agent**: a production-grade, robust Rust core library that owns the classic *sample → dispatch → append → re-sample* loop, exposes a typed tool registry (shell + filesystem + search) whose JSON schemas are derived from the arg types, presents those tools through a selectable **dialect**, talks to a model through a **provider trait** with one wire implementation, and returns a single structured result — with **no TUI and no interactive permission prompts**.

**Users:** (1) the future `locode-app` (TUI + features) as a library consumer driving the engine programmatically; (2) researchers running headless A/B comparisons of tool-harness dialects and provider wires; (3) `locode-cli` as a thin reference consumer.

**Success looks like:** a caller can drive one agent session to completion against Claude, under the `grok` dialect, with the engine emitting exactly one machine-readable JSON report — and every architectural extension point (more dialects, more wires, apply_patch, sandbox, MCP, streaming, compaction) is a seam, not a rewrite.

## Tech Stack

| Concern | Choice |
|---|---|
| Language / runtime | Rust (stable, pinned via `rust-toolchain.toml`), `tokio` |
| Serialization / schema | `serde`, `schemars` (JSON Schema derived from arg types) |
| Errors | `thiserror` |
| HTTP | `reqwest` (with `rustls`) |
| Prompt templating | `minijinja` (tool names track the active dialect) |
| CLI (locode-cli only) | `clap` |
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
cargo run -p locode-cli -- --prompt "summarize this repo" --dialect grok --provider anthropic

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
├── locode-protocol/     → history model, tool call/result, report envelope (pure types, no I/O)
├── locode-tools/        → canonical Tool trait + registry + 6 tool impls (host-agnostic contracts)
├── locode-dialects/     → dialect packs: name/param/desc overrides + EditEncoding per pack
├── locode-provider/     → Provider trait + API-agnostic ConversationRequest + Anthropic wire impl
├── locode-host/         → fs/shell/path-jail/truncation (injectable side-effect seam)
├── locode-engine/       → the sample→dispatch→append loop + Session driving API
├── locode/              → thin facade re-exporting the public surface
└── locode-cli/          → minimal headless binary (Codex-exec-style stdout discipline)
```

Dependency direction: `protocol` ← everything; `tools` → `host` + `protocol`; `dialects` → `tools`; `provider` → `protocol`; `engine` → `tools` + `dialects` + `provider` + `host` + `protocol`; `locode` → all; `locode-cli` → `locode`.

## Code Style

Standard `rustfmt`; `clippy -D warnings`. Author tools with concrete types; erase to `dyn` only at the registry boundary. The single most important type — the tool contract ([ADR-0003](docs/decisions/ADR-0003-typed-tool-contract.md)):

```rust
/// A tool authored against concrete types; the wire schema is *derived*, never hand-written.
#[async_trait]
trait Tool: Send + Sync {
    type Args: DeserializeOwned + JsonSchema + Send;
    type Output: Serialize + ToolOutput + Send; // ToolOutput::to_prompt_text(&self) -> String

    fn kind(&self) -> ToolKind;                 // canonical identity: Shell/Read/Write/Edit/Glob/Grep
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

Conventions: canonical identity is a `ToolKind`, never a wire name; a tool result has two faces (`output` for the JSON report, `to_prompt_text()` for model history); `ToolCtx` stays small (`cwd`, `call_id`, `workspace_root`, `cancel`) — no god-object context.

## Testing Strategy

- **Framework:** `cargo test` (workspace). Adopt `cargo-nextest` only when subprocess tests get flaky/slow.
- **Highest-leverage surface:** a **mock `Provider`** that emits scripted `tool_calls`, so the loop is unit-tested with zero API spend. The loop — transcript pairing, soft/fatal handling, max-turns, abort repair — is where the subtle bugs live.
- **Golden test** on the report-envelope JSON shape (freeze `schema_version: 1`).
- **Per-crate unit tests**, especially the four `edit` invariants (read-before-edit, exact+unique match, mtime freshness, reject no-op) and the path jail.
- **One end-to-end integration test** driving `locode-engine` through a real or recorded Anthropic wire on a trivial task.
- Tests live inline (`#[cfg(test)]`) for unit scope; cross-crate integration under each crate's `tests/`.

## Boundaries

- **Always:** run the fmt+clippy+test triangle before commit; derive tool schemas from types; route every side effect through the one `dispatch` door and the `locode-host` seam (never call `std::fs`/`Command` from a tool body); guarantee every `tool_use` id gets exactly one `tool_result` before the next sample; keep stdout to exactly one JSON document.
- **Ask first:** adding a dependency; changing the report envelope `schema_version` or any public trait signature (`Tool`, `Provider`); changing the crate boundaries; enabling new `[workspace.lints]` denies.
- **Never:** commit secrets/API keys; `println!` from library crates or non-report paths (stdout is sacred — enforce with `#![deny(clippy::print_stdout)]` in `locode-cli`); bury allow/deny policy inside individual tools; leave a `tool_use` unpaired; introduce a second, throwaway loop for headless.

## Success Criteria (v0)

1. `locode-engine` runs a full session (sample → dispatch → append → terminal) against Claude via the Anthropic Messages wire, driven as a library API.
2. Tools available: `shell`, `read`, `write`, `edit` (ExactString), `glob`, `grep` — one canonical impl each, presented under the `grok` dialect with schemas derived from arg types.
3. All four `edit` invariants enforced; path jail rejects `..` escapes; shell honors a hard timeout + output byte cap with a truncation marker.
4. Every tool failure is a soft `tool_result{is_error}`; a pre-send pass guarantees transcript validity (no dangling/duplicate tool results); an explicit `max_turns` ceiling terminates cleanly.
5. `locode-cli` emits **exactly one** JSON report on stdout (stamping `dialect` + `provider`), all diagnostics on stderr, exit 0 on clean terminal state / non-zero on fatal.
6. The mandatory CI triangle is green; the loop is covered by mock-provider unit tests.
7. Extension seams exist and are unit-touched but unimplemented: `EditEncoding::ApplyPatchFreeform`, a second `Provider` wire, additional dialects, parallel tools, compaction, sandbox, MCP.

## Open Questions

Carried from the design doc §12, minus what we've now decided (wire = Anthropic; dialect = grok; workspace layout; in-repo minimal CLI). Still genuinely undecided:

1. **Edit strictness** — exact-match-only in v0, or adopt one or two of OpenCode's tolerant replacers early? (Default: exact-only.)
2. **Search impl** — shell out to `rg` when present, or embed a walker for determinism/cross-platform? (Default: `rg` if on PATH, else walk.)
3. **When to add `apply_patch` (P1)** — as soon as the `codex` dialect enters the A/B, or when multi-hunk string edits get painful?
4. **Schema-constrained task answers** (`--json-schema`) — native `response_format` first with a `StructuredOutput`-tool fallback; envelope-only for v0. Needed when?
5. **Session durability** — when do ephemeral runs need JSONL transcript persistence?
6. **`Cargo.lock` + facade surface** — how much does `locode` re-export vs. keep crate-private for the future `locode-app`?

## Decisions of record

| ADR | Decision |
|---|---|
| [0001](docs/decisions/ADR-0001-headless-core-scope.md) | Headless-only core library; no TUI/interactive prompts in this repo |
| [0002](docs/decisions/ADR-0002-multi-crate-workspace.md) | Multi-crate `locode-*` Cargo workspace |
| [0003](docs/decisions/ADR-0003-typed-tool-contract.md) | Typed `Tool` contract, schemars-derived schemas, dual `output`/`prompt_text` |
| [0004](docs/decisions/ADR-0004-error-taxonomy-and-pairing.md) | Soft/fatal error taxonomy + strict tool_use/tool_result pairing |
| [0005](docs/decisions/ADR-0005-agent-loop.md) | Sample→dispatch→append loop; non-streaming, serial-first; explicit max-turns |
| [0006](docs/decisions/ADR-0006-dialects-and-edit-encoding.md) | Dialect packs over one registry; `grok` default; `EditEncoding` enum |
| [0007](docs/decisions/ADR-0007-provider-trait.md) | `Provider` trait over API-agnostic request; Anthropic Messages wire first |
| [0008](docs/decisions/ADR-0008-dispatch-door-and-path-jail.md) | One dispatch door + workspace path jail as v0 security posture |
| [0009](docs/decisions/ADR-0009-headless-io-contract.md) | Single JSON report on stdout; diagnostics on stderr |
| [0010](docs/decisions/ADR-0010-rust-tooling-baseline.md) | Rust tooling/CI baseline (pinned toolchain, fmt+clippy-deny+test) |
