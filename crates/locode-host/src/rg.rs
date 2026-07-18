//! Ripgrep binary resolution (ADR-0011).

use std::path::PathBuf;

/// The ripgrep program to invoke: the `LOCODE_RG_PATH` override, else bare `rg` on PATH
/// (invoked by name — PATH-hijack hygiene, per Claude Code).
///
/// Bundled self-extraction is a packaging concern layered in later (the `bundle-rg`
/// feature, Task 14); if `rg` cannot be started, the caller surfaces a soft error rather
/// than falling back to a divergent hand-rolled search.
#[must_use]
pub fn rg_program() -> PathBuf {
    std::env::var_os("LOCODE_RG_PATH").map_or_else(|| PathBuf::from("rg"), PathBuf::from)
}
