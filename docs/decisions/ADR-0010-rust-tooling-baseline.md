# ADR-0010: Rust tooling and CI baseline

## Status
Accepted

## Date
2026-07-17

## Context
Codex shows a mature, heavyweight Rust engineering system (fmt/clippy-as-errors, cargo-deny, nextest, `just`, a required CI gate, Bazel, dylint, multi-OS matrices). Grok shows strong local toolchain discipline and scoped-crate habits. A greenfield agent should steal the *ideas* without the machinery. The maintainer comes from a Python + pre-commit + Black/Ruff background, so the mental model is: rustfmt ≈ Black, clippy ≈ Ruff, `cargo test` ≈ pytest.

## Decision
Adopt the **mandatory triangle** — `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` — as the day-one quality bar, plus:
- `rust-toolchain.toml` pinning current stable + `rustfmt`, `clippy` (bump one minor at a time, deliberately).
- `rustfmt.toml` near-default; `clippy.toml` starts minimal; `[workspace.lints]` starts mild (`unused_must_use = "deny"`) and tightens later.
- A single GitHub Actions job (checkout → toolchain from file → `Swatinem/rust-cache` → fmt/clippy/test).
- A `justfile` (`fmt`, `fmt-check`, `clippy`, `fix`, `test`, `check`) and scoped `-p <crate>` iteration.
- Optional `.pre-commit-config.yaml` matching the Python habit: **fmt+clippy on commit, test on pre-push**.
- Commit `Cargo.lock` (this repo ships a binary).

## Alternatives Considered
### Codex-scale CI on day one (Bazel, dylint, multi-OS matrix, `unwrap_used=deny` workspace-wide)
- Rejected: heavy ops cost; `unwrap_used=deny` fights tests early. Phase these in (Phase 2/3) as the repo hardens.

### CI-only, no local hooks
- Rejected for this maintainer: pre-commit maps cleanly from the existing Black/Ruff/pytest workflow and catches issues offline.

## Consequences
- Every incremental slice lands green against a consistent, pinned toolchain.
- Phased hardening path: **Phase 0** triangle+CI → **Phase 1** justfile/pre-commit/rust-cache/nextest → **Phase 2** cargo-deny/audit, tighter lints → **Phase 3** multi-platform release (only if shipping installers).
- `cargo-nextest` and `cargo-deny`/`cargo-audit` are adopted **when** the suite gets slow/flaky or deps grow — not before.
