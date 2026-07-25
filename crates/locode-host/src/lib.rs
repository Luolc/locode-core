//! locode-host — the one place the system touches the OS (ADR-0008).
//!
//! Every side effect funnels through the [`Host`]: a path jail, shell execution with a
//! timeout + output cap, jailed filesystem helpers, and the shared
//! post-processes. Tools never call `std::fs`/`Command` directly —
//! they go through a `Host`. This crate is a *sibling* of `locode-tools`, so it defines
//! its own plain error types; the harness pack (which depends on both) maps them to
//! `ToolError::Respond`.

mod fs;
mod home;
mod path;
mod rg;
mod root;
mod session_dirs;
mod settings;
mod shell;
mod trace;
mod walk;

pub use fs::{DirEntry, FileRead, FileStat, FsError};
pub use home::{default_locode_home, locode_home, resolve_home_from};
pub use path::PathError;
pub use rg::rg_program;
pub use root::find_root_from_markers;
pub use session_dirs::{decode_cwd_dirname, encode_cwd_dirname};
pub use settings::{
    Settings, SettingsLoad, SkillsExtraEntry, load_settings, load_settings_from,
    update_user_setting,
};
pub use shell::{
    ExecError, ExecOutput, ExecRequest, FrontBackCapture, FrontBackSpec, SPILL_RETAIN_MAX,
    ShellSpec,
};
pub use trace::{
    GitMeta, RolloutContents, SessionMeta, TRACE_SCHEMA_VERSION, TraceExtras, TraceWriter,
    find_latest_rollout, find_rollout_by_id, read_rollout,
};
pub use walk::{WalkEntry, WalkOptions};

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Whether the filesystem tools are jailed to the workspace root (ADR-0008 amendment).
///
/// The jail is the **default** posture, not a hard wall — every studied harness offers a
/// full-access mode, and since we are headless (no permission prompt) a caller must be
/// able to opt out (`--dangerously-skip-permissions` / `--yolo`). The shell tool is never
/// path-jailed regardless; only the structured FS tools consult this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PathPolicy {
    /// Resolve FS-tool paths under `workspace_root`; reject `..`/absolute/symlink escapes.
    #[default]
    Jailed,
    /// Resolve relative paths against `cwd`, allow absolute / out-of-root paths (the
    /// `--dangerously-skip-permissions` / `--yolo` behavior).
    Unrestricted,
}

/// Limits on shell execution. Defaults mirror Grok Build (the primary model).
#[derive(Debug, Clone)]
pub struct ExecLimits {
    /// Timeout used when a call passes none (grok/codex default = 10s).
    pub default_timeout: Duration,
    /// Hard ceiling; a caller-supplied timeout is clamped to this.
    pub max_timeout: Duration,
    /// Max retained output bytes per stream before truncation (grok = `30_000`).
    pub max_output_bytes: usize,
    /// Grace between `SIGTERM` and `SIGKILL` when killing a timed-out/cancelled command.
    pub kill_grace: Duration,
}

impl Default for ExecLimits {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(10),
            max_timeout: Duration::from_mins(10),
            max_output_bytes: 30_000,
            kill_grace: Duration::from_secs(2),
        }
    }
}

/// Construction-time configuration for a [`Host`].
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The path-jail root (canonicalized at [`Host::new`]).
    pub workspace_root: PathBuf,
    /// Whether FS tools are jailed to `workspace_root` (default [`PathPolicy::Jailed`]).
    pub path_policy: PathPolicy,
    /// Shell execution limits.
    pub exec: ExecLimits,
    /// The shell program to run commands through (default `bash`). A configurable field
    /// so a later pack / shell-detection can override it.
    pub shell_program: String,
    /// Run the shell as a login shell (`-lc`, loads the user's profile/RC — matches all
    /// four studied harnesses) rather than `-c`. Default `true`.
    pub login_shell: bool,
}

impl HostConfig {
    /// A config for `workspace_root` with all defaults (jailed, `bash -lc`, grok limits).
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            path_policy: PathPolicy::default(),
            exec: ExecLimits::default(),
            shell_program: "bash".to_string(),
            login_shell: true,
        }
    }
}

/// The injectable side-effect seam (ADR-0008). Construct once per session, share behind
/// an `Arc`; holds the jail root + policy and the exec/truncation limits.
#[derive(Debug, Clone)]
pub struct Host {
    pub(crate) workspace_root: PathBuf,
    pub(crate) path_policy: PathPolicy,
    pub(crate) limits: ExecLimits,
    pub(crate) shell_program: String,
    pub(crate) login_shell: bool,
    /// Lazily-built workspace gitignore matcher (Task 26 Slice 0).
    pub(crate) gitignore: walk::GitignoreCache,
    /// Once-probed login-shell PATH (Task 26 Slice 0b; grok's PATH probe).
    pub(crate) login_path: std::sync::Arc<tokio::sync::OnceCell<Option<String>>>,
}

impl Host {
    /// Build a host, canonicalizing `workspace_root` (the jail root must be a real,
    /// symlink-resolved absolute path).
    ///
    /// # Errors
    /// [`PathError::InvalidRoot`] if `workspace_root` does not exist or cannot be
    /// canonicalized.
    pub fn new(config: HostConfig) -> Result<Self, PathError> {
        let workspace_root = std::fs::canonicalize(&config.workspace_root).map_err(|e| {
            PathError::InvalidRoot(format!("{}: {e}", config.workspace_root.display()))
        })?;
        Ok(Self {
            workspace_root,
            path_policy: config.path_policy,
            limits: config.exec,
            shell_program: config.shell_program,
            login_shell: config.login_shell,
            gitignore: walk::GitignoreCache::new(),
            login_path: std::sync::Arc::new(tokio::sync::OnceCell::new()),
        })
    }

    /// The canonicalized jail root.
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// The active path policy.
    #[must_use]
    pub fn path_policy(&self) -> PathPolicy {
        self.path_policy
    }

    /// The shell execution limits.
    #[must_use]
    pub fn limits(&self) -> &ExecLimits {
        &self.limits
    }
}

#[cfg(test)]
pub(crate) fn test_host(root: &Path, policy: PathPolicy, login: bool) -> Host {
    let mut config = HostConfig::new(root);
    config.path_policy = policy;
    config.login_shell = login;
    Host::new(config).expect("test host")
}
