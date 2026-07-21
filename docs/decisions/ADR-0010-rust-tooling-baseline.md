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

**Merge gate.** ~~Platform-enforced required status checks (and true `gh pr merge --auto`) need GitHub Pro or a public repo; … the gate is **procedural**~~ — **superseded 2026-07-21 (repo is public):** the gate is now **platform-enforced**. `main` has classic branch protection: required status check `fmt · clippy · test · doc` (strict up-to-date), `enforce_admins` (direct pushes rejected for everyone, including the agent/owner), linear history, no force pushes or deletions; repo-level auto-merge is enabled. The agent flow is: branch → `gh pr create` → `gh pr merge --auto --squash --delete-branch` immediately — GitHub merges on green with no watcher process. Direct-to-`main` no longer exists, even for trivial fixes (auto-merge makes PR overhead negligible).

## Amendment (2026-07-21): release binary artifacts

On every version tag (`vX.Y.Z`, pushed after the crates.io publish), the
`release` workflow (`.github/workflows/release.yml`) creates a GitHub Release
and attaches **fully static musl Linux binaries** of `locode-exec` —
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` tarballs with
sha256 checksums (taiki-e action family; same third-party tier as the existing
`Swatinem/rust-cache`). Static linking is possible because the workspace is
OpenSSL-free by design (`rustls`, SPEC/ADR-0007). The release procedure is:
version-bump PR → auto-merge on green → `cargo publish` in dependency order →
`git tag vX.Y.Z && git push origin vX.Y.Z`. macOS/Windows artifacts are an
easy later addition to the same matrix if wanted.

## Amendment (2026-07-21): curl installer + macOS release targets

The release matrix additionally builds `x86_64-apple-darwin` and
`aarch64-apple-darwin` (native on the macOS runner — the cross-toolchain step
is Linux-only; `upload-rust-binary-action` installs the Rust target itself).
A hand-rolled `install.sh` at the repo root gives macOS/Linux users a single
install-and-update command:

```sh
curl -fsSL https://raw.githubusercontent.com/luolc/locode-core/main/install.sh | bash
```

The script is modeled on the grok-build and opencode installers (survey
submodules: `xai-grok-pager/scripts/install.sh`, `opencode/install`) but uses
GitHub Releases directly (`releases/latest/download/…`, opencode-style — no
version-pointer infrastructure) and adds sha256 verification, which neither
reference does: platform detect → download → checksum verify → `--version`
smoke test → atomic swap into `~/.locode/bin` → marker-delimited PATH block
(idempotent re-runs; re-running the one-liner *is* the update mechanism,
grok-style). Windows is out of scope (user decision, 2026-07-21). cargo-dist
was considered and declined for now: the hand-rolled workflow already exists
and the script is ~170 lines; revisit if Homebrew/MSI/self-update surfaces
are wanted. macOS installs resolve `latest` to the first release tagged after
this amendment (earlier releases carry Linux assets only).
