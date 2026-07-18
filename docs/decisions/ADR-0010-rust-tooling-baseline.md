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

## Amendment (2026-07-17): strict-from-empty lints

The original "start mild, tighten later" advice assumed the usual trap of tightening lints *after* code exists — which Grok Build's own `Cargo.toml` documents as painful (a lint added late breaks `main` when older branches merge). We are in the opposite position: the workspace was enabled while all 8 crates are **empty**, so front-loading strict lints is free. The maintainer prefers a high standard, so we turn the strictness up now rather than retrofit it.

**Enabled at scaffold time (Task 2):**
- `[workspace.lints.rust]`: `unsafe_code = "forbid"` (this project needs no `unsafe`), `unused_must_use = "deny"`, `missing_docs = "warn"` (every public item is documented), `rust_2018_idioms = "warn"`.
- `[workspace.lints.clippy]`: `pedantic = "warn"` (the opinionated high-value group; individual lints get `#[allow(…)]` with a reason when genuinely wrong), plus `unwrap_used`/`expect_used`/`dbg_macro = "deny"`.
- `clippy.toml`: `allow-unwrap-in-tests` / `allow-expect-in-tests = true` — the "ban in library code, allow in tests" pattern (matches Codex).
- CI (`.github/workflows/ci.yml`): a single Ubuntu job — `fmt --check`, `clippy --all-targets --all-features -D warnings`, `cargo test`, and `cargo doc --no-deps` with `RUSTDOCFLAGS=-D warnings` (broken intra-doc links fail). `Swatinem/rust-cache`; `concurrency: cancel-in-progress`; toolchain pinned to match `rust-toolchain.toml`.

**Deliberately deferred** (revisit as the repo grows): the clippy `cargo` group (needs crate metadata/licence fields first), `cargo-deny` (Phase B — little to check against an empty dep tree), `cargo-nextest` (+ a `--doc` step, since nextest skips doctests), a macOS matrix (when we build/bundle `rg`), coverage, `cargo-semver-checks`, and typo/spell checks. **Not** adopted at all: Bazel, custom dylint, multi-hour path-filtered job graphs (Codex-scale machinery).

**Merge gate.** Platform-enforced required status checks (and true `gh pr merge --auto`) need GitHub Pro or a public repo; classic branch protection is unavailable on a free **private** repo. Until the repo goes Pro or public, the gate is **procedural**: the agent opens a PR, watches CI, and squash-merges **only on green** — still fully agent-driven (no manual clicks), just not enforced by the platform. When the plan changes, switch to a required-checks ruleset + `--auto`.
