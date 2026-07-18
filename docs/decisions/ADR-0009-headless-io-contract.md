# ADR-0009: Single JSON report on stdout; diagnostics on stderr

## Status
Accepted

## Date
2026-07-17

## Context
Headless agents are driven by other programs. Codex enforces "stdout is sacred" structurally with `#![deny(clippy::print_stdout)]` in its `exec` crate: exactly one machine-readable artifact on stdout, everything else on stderr. A predictable output contract is what makes cross-run analysis and programmatic consumption mechanical.

## Decision
`locode-exec` emits **exactly one JSON document** — the final report — on **stdout**, and routes all human logs/traces to **stderr**. Exit `0` on any structured terminal state (`completed`/`max_turns`); non-zero on fatal (auth/config error, `Fatal` tool error, model error after retry). The report envelope stamps `harness` and `provider` (so A/B runs are self-describing) and freezes `schema_version: 1` early. Enforce stdout discipline structurally: `#![deny(clippy::print_stdout)]` in `locode-exec`; library crates never print. Keep two "JSON" concerns distinct: the always-emitted **report envelope**, and an optional **schema-constrained task answer** (`--json-schema`, deferred) that would go in `structured_output` inside the envelope.

Illustrative envelope: `{ schema_version, status, harness, provider, final_message, structured_output, turns, tool_calls[], usage, session_id, error }`; `status ∈ {completed, max_turns, model_error, error}`.

## Alternatives Considered
### Human-readable transcript on stdout
- Rejected: not machine-consumable; breaks programmatic driving (the `locode-app` use case).

### Interleave events on stdout
- Rejected: pollutes the single-document contract. A future `--events-jsonl` stream must go to stderr (or a separate fd/file), never stdout.

## Consequences
- Any caller can parse one JSON blob and know the run's full outcome.
- A stray `println!` in `locode-exec` fails the build.
- `harness`/`provider` in the envelope make A/B comparison mechanical; `schema_version` protects consumers against format drift.
