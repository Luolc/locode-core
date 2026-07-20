//! The stock locode-exec binary: the library CLI with the built-in providers.
//! Downstream binaries with custom providers look identical plus `.register`
//! calls (ADR-0015) — see the crate docs.

use std::process::ExitCode;

fn main() -> ExitCode {
    locode_exec::main_with(locode_exec::ProviderRegistry::builtin())
}
