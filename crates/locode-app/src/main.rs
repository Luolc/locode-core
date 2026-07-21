//! `locode` — the interactive terminal app (SPEC-TUI).
//!
//! Flag-free composition only (SPEC-TUI crate shape): all substance lives in
//! `locode-tui` behind [`locode_tui::main_with`]; this binary exists so a
//! downstream crate can build the same app with a custom [`ProviderRegistry`]
//! (the ADR-0015 pattern, mirrored from `locode-exec`).

use locode_core::ProviderRegistry;

fn main() -> std::process::ExitCode {
    locode_tui::main_with(ProviderRegistry::builtin())
}
