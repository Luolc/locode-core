# Implementation Plan: locode-core v0

Source of truth: [`../SPEC.md`](../SPEC.md) and [`../docs/decisions/`](../docs/decisions/).
This plan covers the **v0** milestone: a headless engine that drives one agent session
to completion against Claude under the `grok` dialect and emits one JSON report.

## Overview

Build the agent spine in dependency order, but **prove the loop with a mock provider before
spending a single API token** — the loop (transcript pairing, soft/fatal handling, max-turns,
abort repair) is where the subtle bugs live, not the tools. Then add real tools + the grok
dialect + the host seam, then the live Anthropic wire + the minimal CLI, then the remaining
dialects to unlock the first A/B comparison.

## Architecture decisions (see ADRs)

- Multi-crate `locode-*` workspace (ADR-0002); tools are host-agnostic, side effects only via `locode-host`.
- Typed `Tool` contract, schemas derived from arg types, dual `output`/`prompt_text` result (ADR-0003).
- `ToolError { Respond, Fatal }`; every `tool_use` paired with exactly one `tool_result` (ADR-0004).
- Sample→dispatch→append loop; non-streaming, serial-first; explicit max-turns (ADR-0005).
- Dialect packs over one registry; `grok` default; `EditEncoding` enum, only `ExactString` built (ADR-0006).
- `Provider` trait over API-agnostic `ConversationRequest`; Anthropic Messages wire first (ADR-0007).
- One dispatch door + workspace path jail (ADR-0008); single JSON report on stdout (ADR-0009).

## Dependency graph

```
locode-protocol  (pure types)
    ├── locode-host        (fs/shell/path-jail/truncation)
    │       └── locode-tools    (Tool trait, registry, dispatch, 6 impls)
    │               └── locode-dialects  (re-skin over tools)
    ├── locode-provider   (Provider trait + MockProvider, then Anthropic wire)
    └── locode-engine     (loop + Session)  ← composes tools+dialects+provider+host
            └── locode (facade) ── locode-exec (minimal binary)
```

Build bottom-up; slice vertically so each checkpoint leaves a working, tested system.

## Task list

### Phase 0: Scaffolding (foundation must land green first)
- [ ] Task 1: Cargo workspace + crate skeletons + pinned toolchain + fmt/clippy configs
- [ ] Task 2: CI (single GitHub Actions job) + justfile

**Checkpoint A:** empty workspace compiles; `just check` is green in CI.

### Phase 1: Core spine, proven with a mock provider (zero API spend)
- [ ] Task 3: `locode-protocol` — history model, tool call/result, report envelope + golden test
- [ ] Task 4: `locode-tools` — `Tool` trait, `ToolKind`, `ToolError`, `ToolCtx`, `ToolOutput`, `DynTool` erasure, registry + `dispatch` door
- [ ] Task 5: `locode-provider` — `Provider` trait, `ConversationRequest`, `Completion`, `MockProvider` (scripted tool_calls) + partial-JSON assembly helper
- [ ] Task 6: `locode-engine` — the loop + `Session` API; terminal states, transcript repair/dedup, max-turns, abort synthesis; unit-tested end-to-end with mock provider + trivial tools

**Checkpoint B:** the full loop runs to every terminal state under `MockProvider` with zero network — the core is proven.

### Phase 2: Real tools + grok dialect + host seam
- [ ] Task 7: `locode-host` — path jail, shell exec (timeout + byte cap + truncation marker), fs helpers, shared truncation post-process
- [ ] Task 8: `locode-dialects` — `Dialect`, `EditEncoding` (ExactString built), grok table, `list_specs` re-skin + reverse name/param map in dispatch
- [ ] Task 9: `shell` + `read` tools over the host, registered under grok
- [ ] Task 10: `write` + `edit` (ExactString) with all four edit invariants
- [ ] Task 11: `glob` + `grep` (ripgrep, host-resolved — ADR-0011)

**Checkpoint C:** all six tools work under the grok dialect, driven by the mock provider; edit invariants and path jail unit-tested.

### Phase 3: Live Anthropic wire + minimal CLI end-to-end
- [ ] Task 12: `locode-provider` Anthropic Messages wire impl (request build, parse, tool-call id preservation, usage, cache_control breakpoints, omit-temp-when-thinking, two-tier retry, 401 refresh, 429 surface, pre-send repair)
- [ ] Task 13: system prompt (minijinja, grok-sized, headless-branched identity, tool names track dialect)
- [ ] Task 14: `locode` facade + `locode-exec` minimal headless binary (clap flags, one JSON report on stdout, `#![deny(clippy::print_stdout)]`, stderr logging; optional `bundle-rg` feature per ADR-0011)

**Checkpoint D:** `cargo run -p locode-exec -- --prompt "summarize this repo"` completes against Claude and prints exactly one JSON report. **v0 success criteria met.**

### Phase 4: Remaining dialects → first A/B (payoff of the harness)
- [ ] Task 15: `claude` + `opencode` dialects as re-skins (opencode camelCase via `param_rename`)
- [ ] Task 16: first A/B run — same task under `--dialect grok` vs `--dialect claude`, diff trajectories/token counts

**Checkpoint E:** two dialects run the same task over the same six impls; the A/B comparison is mechanical.

### Deferred (seams reserved, not v0 — see SPEC §Open Questions and ADR-0006/0007)
`EditEncoding::ApplyPatchFreeform` (+ `codex` dialect) · 2nd provider wire (OpenAI Chat Completions) ·
parallel tool batches · compaction · OS sandbox · MCP · streaming events · schema-constrained answers ·
session durability (JSONL) · multi-platform `rg` bundle matrix + macOS notarization/sidecar (packaging, ADR-0011).

## Definition of Done (every task clears this standing bar)

- `cargo fmt --all -- --check` clean · `cargo clippy --workspace --all-targets -- -D warnings` clean · `cargo test --workspace` green.
- New public items documented; new behavior has a test; no `unwrap`/`expect` in non-test library code paths that can be hit by bad input.
- The transcript-validity invariant (every `tool_use` → exactly one `tool_result`) holds for any code touching history.

## Risks and mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Transcript pairing bugs reject whole provider requests | High | Central pre-send repair/dedup pass (Task 6); mock-provider tests exercise abort/mid-batch cancel before any live wire |
| Anthropic `cache_control` over-marking → 400 | Med | Exactly one marker on last message + ≤4 on system blocks; assert marker count in a test (Task 12) |
| Edit invariants subtly wrong → silent file corruption | High | Dedicated unit tests per invariant; reject on any doubt; exact-match-only in v0 (Task 10) |
| `rg` absent on target machines | Low | Bundled `rg` guarantees availability in shipped binaries (ADR-0011); dev/CI use PATH or the `bundle-rg` feature; missing `rg` → clear soft error, never a silent divergent result |
| Loop/registry coupling makes the loop hard to test in isolation | Med | Trivial in-test tool + `MockProvider`; keep `dispatch` behind a small trait boundary (Task 4/6) |
| Scope creep from deferred seams | Med | Seams are enum variants / trait impls with reserved slots; v0 builds only the first variant |

## Open questions (from SPEC; resolve during their phase)

1. Edit strictness — exact-only vs one tolerant replacer (Task 10). Default: exact-only.
2. When `apply_patch` (P1) lands — with the `codex` dialect A/B, or when multi-hunk edits hurt.
3. Schema-constrained answers (`--json-schema`) — envelope-only for v0; native-first + tool-fallback later.
4. Session durability — when ephemeral runs need JSONL persistence.
5. Facade surface — how much `locode` re-exports vs keeps crate-private for `locode-app`.

_Resolved: Search — **ripgrep, host-resolved, bundled at packaging; no walker** (ADR-0011)._

## Parallelization

Once Phase 1 lands, some Phase 2 work can parallelize: Task 7 (host) and Task 8 (dialects) are
largely independent; Tasks 9/10/11 (tool impls) can proceed in parallel after Task 7+8, as each
tool is an independent slice sharing the settled `Tool` contract.
