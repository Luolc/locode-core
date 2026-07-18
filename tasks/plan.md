# Implementation Plan: locode-core v0

Source of truth: [`../SPEC.md`](../SPEC.md) and [`../docs/decisions/`](../docs/decisions/).
This plan covers the **v0** milestone: a headless engine that drives one agent session
to completion against Claude under the `grok` harness pack and emits one JSON report.

## Overview

Build the agent spine in dependency order, but **prove the loop with a mock provider before
spending a single API token** — the loop (transcript pairing, soft/fatal handling, max-turns,
abort repair) is where the subtle bugs live, not the tools. Then port the **`grok` harness
pack's** real tools onto the host seam, then the live Anthropic wire + the minimal binary.
Additional harness packs (and the first A/B) are the next milestone — v0 ships one faithful pack.

## Architecture decisions (see ADRs)

- Multi-crate `locode-*` workspace (ADR-0002); tools are host-agnostic, side effects only via `locode-host`.
- Typed `Tool` contract, schemas derived from arg types, dual `output`/`prompt_text` result (ADR-0003).
- `ToolError { Respond, Fatal }`; every `tool_use` paired with exactly one `tool_result` (ADR-0004).
- Sample→dispatch→append loop; non-streaming, serial-first; explicit max-turns (ADR-0005).
- Harness packs — faithful per-harness toolsets (not re-skins); v0 = the `grok` pack; `ToolKind` is only a cross-pack comparison tag (ADR-0012, supersedes ADR-0006).
- `Provider` trait over API-agnostic `ConversationRequest`; Anthropic Messages wire first (ADR-0007).
- One dispatch door + workspace path jail (ADR-0008); single JSON report on stdout (ADR-0009).

## Dependency graph

```
locode-protocol  (pure types)
    ├── locode-host        (fs/shell/path-jail/truncation/rg)
    ├── locode-tools       (Tool trait, registry, dispatch — framework, no concrete tools)
    │       └── locode-packs   (grok pack: real per-harness tools, over tools + host)
    ├── locode-provider   (Provider trait + MockProvider, then Anthropic wire)
    └── locode-engine     (loop + Session)  ← composes packs + tools + provider + host
            └── locode (facade) ── locode-exec (minimal binary)
```

Build bottom-up; slice vertically so each checkpoint leaves a working, tested system.

## Task list

### Phase 0: Scaffolding (foundation must land green first)
- [x] Task 1: Cargo workspace + crate skeletons + pinned toolchain + fmt/clippy configs
- [x] Task 2: CI (single GitHub Actions job) + justfile + strict-from-empty lints

**Checkpoint A:** empty workspace compiles; `just check` is green in CI.

### Phase 1: Core spine, proven with a mock provider (zero API spend)
- [x] Task 3: `locode-protocol` — conversation model (4-role, ADR-0013), tool call/result, report envelope + golden test
- [x] Task 3b: streaming event protocol types (`Event` + `reconstruct_conversation`) — the `stream-json` foundation (ADR-0014)
- [x] Task 4: `locode-tools` — `Tool` trait, `ToolKind`, `ToolError`, `ToolCtx`, `ToolOutput`, `DynTool` erasure, registry (typed + `register_dyn` MCP seam) + `dispatch` door
- [x] Task 5: `locode-provider` — `Provider` trait (`api_schema`+`complete`), `ConversationRequest`, `SamplingArgs`, `Completion` (normalized `Vec<ContentBlock>`, thinking preserved), `ProviderError` (exhaustive+`retryable`), `MockProvider` (scripted) + `ToolCallAssembler` partial-JSON helper
- [x] Task 6: `locode-engine` — the loop + `Session` API; terminal states, transcript repair/dedup (`repair_pairing` in `locode-provider`), max-turns, abort synthesis; **emits the `stream-json` `Event`s** (ADR-0014); unit-tested end-to-end with mock provider + trivial tools

**Checkpoint B:** the full loop runs to every terminal state under `MockProvider` with zero network — the core is proven. ✅ reached.

### Phase 2: The `grok` harness pack + host seam
- [ ] Task 7: `locode-host` — path jail, shell exec (timeout + byte cap + truncation marker), fs helpers, shared truncation post-process
- [ ] Task 8: `locode-packs` — pack framework (a `Pack` = named tool set + system prompt + registration) + `--harness` selection; grok pack wiring with `ToolKind` tags for A/B
- [ ] Task 9: grok pack — `run_terminal_command` + `read_file`, ported from `xai-grok-tools` over the host
- [ ] Task 10: grok pack — `write` + `search_replace`, ported (grok's real exact-string edit + freshness invariants)
- [ ] Task 11: grok pack — `grep` + dir/glob, ripgrep-backed (host-resolved — ADR-0011)

**Checkpoint C:** the grok pack's tools work end-to-end under the mock provider; edit invariants and path jail unit-tested.

### Phase 3: Live Anthropic wire + minimal CLI end-to-end
- [ ] Task 12: `locode-provider` Anthropic Messages wire impl (request build, parse, tool-call id preservation, usage, cache_control breakpoints, omit-temp-when-thinking, two-tier retry, 401 refresh, 429 surface, pre-send repair)
- [ ] Task 13: grok pack system prompt (minijinja, ported from grok's real prompt, headless-branched identity)
- [ ] Task 14: `locode` facade + `locode-exec` minimal headless binary (`--output-format {json,text,stream-json}` per ADR-0014, `#![deny(clippy::print_stdout)]`, stderr logging; optional `bundle-rg` feature per ADR-0011)

**Checkpoint D:** `cargo run -p locode-exec -- --prompt "summarize this repo"` completes against Claude and prints exactly one JSON report. **v0 success criteria met.**

### Next milestone (post-v0): more harness packs → first A/B
- [ ] Additional packs: `codex`, `claude`, `opencode` (faithful ports) + the `locode` best-of pack (grok-build-style naming). The `codex` pack brings `apply_patch` (JSON-string framing on Anthropic).
- [ ] First A/B run — same task under `--harness grok` vs another pack; diff trajectories/token counts/edit-success (aligned by `ToolKind` tags).

**Milestone goal:** two packs run the same task with **genuinely different tool behavior**; the A/B comparison is honest and mechanical.

### Deferred (seams reserved, not v0 — see SPEC §Open Questions and ADR-0007/0012)
`apply_patch` JSON-string framing (with the `codex` pack) · 2nd provider wire (OpenAI Chat Completions) + freeform-grammar `apply_patch` (Responses wire) ·
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
| Edit invariants subtly wrong → silent file corruption | High | Dedicated unit tests per invariant; reject on any doubt; port grok's real exact-string `search_replace` behavior faithfully (Task 10) |
| `rg` absent on target machines | Low | Bundled `rg` guarantees availability in shipped binaries (ADR-0011); dev/CI use PATH or the `bundle-rg` feature; missing `rg` → clear soft error, never a silent divergent result |
| Loop/registry coupling makes the loop hard to test in isolation | Med | Trivial in-test tool + `MockProvider`; keep `dispatch` behind a small trait boundary (Task 4/6) |
| Scope creep from deferred seams | Med | Seams are enum variants / trait impls with reserved slots; v0 builds only the first variant |

## Open questions (from SPEC; resolve during their phase)

1. Edit strictness — exact-only vs one tolerant replacer (Task 10). Default: exact-only.
2. When `apply_patch` lands — with the `codex` pack (JSON-string framing on Anthropic; freeform-grammar deferred to a Responses wire).
3. Schema-constrained answers (`--json-schema`) — envelope-only for v0; native-first + tool-fallback later.
4. Session durability — when ephemeral runs need JSONL persistence.
5. Facade surface — how much `locode` re-exports vs keeps crate-private for `locode-app`.

_Resolved: Search — **ripgrep, host-resolved, bundled at packaging; no walker** (ADR-0011)._

## Parallelization

Once Phase 1 lands, some Phase 2 work can parallelize: Task 7 (host) and Task 8 (pack framework) are
largely independent; Tasks 9/10/11 (grok pack tools) can proceed in parallel after Task 7+8, as each
tool is an independent slice sharing the settled `Tool` contract.
