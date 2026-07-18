# Developer commands (https://just.systems). Mirrors the mandatory quality
# triangle plus a docs gate — see ADR-0010. Scope with `-p <crate>` while iterating.

# Run the full local gate.
default: check

# Format all crates.
fmt:
    cargo fmt --all

# Check formatting (CI).
fmt-check:
    cargo fmt --all -- --check

# Lint with warnings-as-errors (CI).
clippy:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Auto-fix formatting + lints.
fix:
    cargo fmt --all
    cargo clippy --workspace --all-targets --all-features --fix --allow-dirty -- -D warnings

# Run all tests, including doctests.
test:
    cargo test --workspace

# Build docs, failing on broken intra-doc links (CI).
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# The full gate: format, lint, test, docs.
check: fmt-check clippy test doc
