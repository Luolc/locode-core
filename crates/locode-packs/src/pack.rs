//! The harness-pack abstraction (ADR-0012): a named toolset + a base prompt.

use std::path::PathBuf;

use locode_protocol::Message;
use locode_tools::Registry;

/// Dynamic, per-run context a pack's preamble is rendered against.
///
/// Deliberately small (like `ToolCtx`, ADR-0003 rejects god-object contexts): the fields
/// a harness's real prompt needs (Task 13) — cwd/OS/shell/date + the headless identity
/// branch. Grows only if a ported prompt needs more.
#[derive(Debug, Clone)]
pub struct PackContext {
    /// Absolute working directory shown to the model.
    pub cwd: PathBuf,
    /// Target OS label (e.g. `macos`).
    pub os: String,
    /// Login shell (e.g. `/bin/zsh`).
    pub shell: String,
    /// Current date, preformatted (the preamble stays a pure function of context).
    pub date: String,
    /// Headless run → autonomous identity branch (vs interactive). See Task 13.
    pub headless: bool,
}

/// A faithful reproduction of one harness: its real toolset + its base prompt, selected
/// whole via `--harness` (ADR-0012). One pack is active per run.
///
/// The pack — not a per-tool field — is the unit of harness identity (contrast Grok
/// Build, which tags every tool with a namespace because it co-locates all harnesses'
/// tools in one registry; we build a fresh registry per pack, so no tag is needed).
pub trait Pack: Send + Sync {
    /// The `--harness` selector and the report-envelope `harness` value.
    fn name(&self) -> &'static str;

    /// Register this pack's tools into `reg`, each under its harness's **real wire name**
    /// (`Tool` has no name of its own — the name is assigned here). A duplicate name is a
    /// wiring bug and panics inside `Registry::register`.
    fn register(&self, registry: &mut Registry);

    /// The pack's **base preamble**: the ordered, role-tagged `System`/`Developer`
    /// messages that seed the conversation (ADR-0013). Each pack maps its harness onto
    /// our roles faithfully — a single `System` message, or `System` + `Developer`, etc.
    /// The wire (Task 12) places each role in the right slot. Task 8 ships a scaffold;
    /// the real content lands in Task 13.
    fn preamble(&self, ctx: &PackContext) -> Vec<Message>;

    /// Convenience: a fresh [`Registry`] holding exactly this pack's tools.
    ///
    /// # Panics
    /// If the pack's [`Pack::register`] assigns the same wire name twice.
    fn build_registry(&self) -> Registry {
        let mut registry = Registry::new();
        self.register(&mut registry);
        registry
    }
}
