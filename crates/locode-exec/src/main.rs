//! locode-exec — minimal headless binary that exercises the locode library.
//!
//! Stdout discipline (ADR-0009): stdout carries exactly one JSON report and nothing else,
//! enforced structurally by the crate-level lint below. The real entrypoint lands in Task 14.
#![deny(clippy::print_stdout)]

fn main() {
    // Scaffold only — wired up in Task 14 (facade + CLI).
}
