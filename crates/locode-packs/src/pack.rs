//! The harness-pack abstraction (ADR-0012): a named toolset + a base prompt.

use std::path::PathBuf;
use std::sync::Arc;

use locode_host::Host;
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
    /// Whether the working directory is inside a git repository. Rendered in the
    /// Claude pack's `# Environment` block (`Is a git repository: <bool>`, D9).
    /// Cheaply probed by the exec/tui layer (a `.git` walk) — no host handle in
    /// `preamble()`.
    pub is_git_repo: bool,
    /// The model id/name for the Claude pack's env "You are powered by the model …"
    /// line (D9). `None` skips the line (the pack does not guess).
    pub model: Option<String>,
    /// The OS version string for the Claude pack's env `OS Version:` line
    /// (`uname -s -r`, computed by the exec/tui layer). `None` skips the line.
    pub os_version: Option<String>,
    /// The IANA timezone name for the codex pack's `<environment_context>`
    /// `<timezone>` line (e.g. `America/Los_Angeles`), resolved best-effort by the
    /// exec/tui layer. `None` omits the line (codex's field is optional).
    pub timezone: Option<String>,
    /// Strip identity-revealing sentences from the rendered prompt (e.g. grok's
    /// "You are Grok released by xAI." and the `<user_guide>` block naming the
    /// Grok Build TUI). **Default `false` = faithful reproduction** (user
    /// decision, 2026-07-18); `true` is for A/B runs where the harness's
    /// self-identity would contaminate the comparison. Post-processing on the
    /// rendered output only — the verbatim template copies are never edited.
    pub strip_identity: bool,
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

    /// Register this pack's tools into `registry`, each under its harness's **real wire
    /// name** (`Tool` has no name of its own — the name is assigned here). Tools are
    /// constructed here holding an `Arc<Host>` (the only OS seam; `ToolCtx` is too small
    /// to carry it). A duplicate name is a wiring bug and panics inside
    /// `Registry::register`.
    fn register(&self, host: &Arc<Host>, registry: &mut Registry);

    /// The pack's **base preamble**: the ordered, role-tagged `System`/`Developer`
    /// messages that seed the conversation (ADR-0013). Each pack maps its harness onto
    /// our roles faithfully — a single `System` message, or `System` + `Developer`, etc.
    /// The wire (Task 12) places each role in the right slot. Task 8 ships a scaffold;
    /// the real content lands in Task 13.
    fn preamble(&self, ctx: &PackContext) -> Vec<Message>;

    /// The provider wire schemas this pack's tools require, or `None` when the pack
    /// is wire-agnostic (grok, claude). The codex pack pins `["openai-responses"]`
    /// because its `apply_patch` is a freeform (custom-grammar) tool that only
    /// round-trips on the OpenAI Responses wire (ADR-0012, D5). Checked pre-run
    /// against the resolved provider: a real schema not in the set fails before the
    /// loop with an actionable message. The `mock` wire (keyless CI) is a universal
    /// escape hatch and is always allowed, independent of this list.
    fn required_api_schemas(&self) -> Option<&'static [&'static str]> {
        None
    }

    /// Shape a raw user prompt the way this harness expects it. Some harnesses wrap
    /// the task in a tag their system prompt refers to (grok's `<user_query>`);
    /// Claude Code sends it verbatim. The default is verbatim; a pack overrides only
    /// if its prompt is written against a wrapper. Keeps harness-specific shaping in
    /// the pack rather than spread across the exec/tui layers.
    fn shape_user_prompt(&self, text: &str) -> String {
        text.to_owned()
    }

    /// Convenience: a fresh [`Registry`] holding exactly this pack's tools, over `host`.
    ///
    /// # Panics
    /// If the pack's [`Pack::register`] assigns the same wire name twice.
    fn build_registry(&self, host: &Arc<Host>) -> Registry {
        let mut registry = Registry::new();
        self.register(host, &mut registry);
        registry
    }
}
