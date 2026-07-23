# ADR-0009: Single JSON report on stdout; diagnostics on stderr

## Status
Accepted

## Date
2026-07-17

## Context
Headless agents are driven by other programs. Codex enforces "stdout is sacred" structurally with `#![deny(clippy::print_stdout)]` in its `exec` crate: exactly one machine-readable artifact on stdout, everything else on stderr. A predictable output contract is what makes cross-run analysis and programmatic consumption mechanical.

## Decision
`locode-exec` emits **exactly one JSON document** — the final report — on **stdout**, and routes all human logs/traces to **stderr**. Exit `0` on any structured terminal state (`completed`/`max_turns`); non-zero on fatal (auth/config error, `Fatal` tool error, model error after retry). The report envelope stamps `harness` and `api_schema` (so A/B runs are self-describing) and freezes `schema_version: 1` early. (The wire-identity field is named `api_schema`, not `provider`: it names the request/response *protocol shape* — the provider's `api_schema()` — not a gateway/endpoint, which is configuration.) Enforce stdout discipline structurally: `#![deny(clippy::print_stdout)]` in `locode-exec`; library crates never print. Keep two "JSON" concerns distinct: the always-emitted **report envelope**, and an optional **schema-constrained task answer** (`--json-schema`, deferred) that would go in `structured_output` inside the envelope.

Illustrative envelope: `{ schema_version, status, harness, api_schema, final_message, structured_output, turns, tool_calls[], usage, session_id, error }`; `status ∈ {completed, max_turns, model_error, error, cancelled}` (`cancelled` added by ADR-0018).

*(Amended 2026-07-22: the standalone `locode-exec` **binary** was retired — this stdout/exit contract is now the `locode -p` path calling `locode_exec::run_headless` (a library); the discipline is unchanged, ADR-0019.)*

## Alternatives Considered
### Human-readable transcript on stdout
- Rejected: not machine-consumable; breaks programmatic driving (the `locode-app` use case).

### Interleave events on stdout
- Rejected: pollutes the single-document contract. A future `--events-jsonl` stream must go to stderr (or a separate fd/file), never stdout.

## Consequences
- Any caller can parse one JSON blob and know the run's full outcome.
- A stray `println!` in `locode-exec` fails the build.
- `harness`/`api_schema` in the envelope make A/B comparison mechanical; `schema_version` protects consumers against format drift.

## Amendment (2026-07-19): `stop_reason` + honest usage counters

Two envelope deltas, landed together before any external consumer exists
(swe-lab has not ported yet; `schema_version` stays 1 as the envelope's shape
is still pre-adoption):

- **`stop_reason: Option<String>`** — the final completion's neutral stop
  reason (`"end_turn"`, `"max_tokens"`, …), so an eval pipeline distinguishes
  "model finished" from "model got truncated" without re-reading the trace.
- **Usage counters become `Option<u64>`** (`cache_read_tokens`,
  `cache_creation_tokens`, new `reasoning_tokens`): `Some(0)` ≠ `None` —
  "reported as zero" vs "this wire does not report the counter". Zero-as-N/A
  was rejected (user decision); summation treats `None` as identity, so a run
  total is `null` only when no turn ever reported the counter.
