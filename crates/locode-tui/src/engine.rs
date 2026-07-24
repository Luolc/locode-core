//! The engine task: owns the `Session`, executes submits, and streams
//! engine output to the UI over typed channels — the minimal in-process form
//! of "the TUI is a pure client" (codex's protocol-seam lesson, study §3.2).

use std::sync::Arc;

use locode_core::{
    CacheHint, CancellationToken, EngineConfig, EventSink, FnSink, Host, HostConfig,
    InstructionsConfig, Pack, PackContext, PathPolicy, ProviderInit, ProviderRegistry, Report,
    SamplingArgs, Session,
};

use crate::cli::Cli;

/// Commands from the UI to the engine task.
#[derive(Debug)]
pub enum UiCommand {
    /// Run one turn with this (unwrapped) prompt text.
    Submit(String),
    /// Discard the current `Session` and build a fresh one (`/new`).
    NewSession,
}

/// Messages from the engine task to the UI.
#[derive(Debug)]
pub enum EngineMsg {
    /// Session assembled; the app is ready to accept prompts.
    Ready {
        /// Resolved model id (for the status display).
        model: String,
        /// Working directory, home-shortened (for the status display).
        cwd: String,
        /// Shell that `run_terminal_cmd` uses (for the status display), resolved
        /// with grok's `$SHELL` rule (see `detect_shell`).
        shell: String,
    },
    /// Session assembly failed pre-run (bad schema, missing key, …).
    BuildFailed(String),
    /// A run is about to start; carries the run's cancel handle (ADR-0018 —
    /// per-run token, cloned before the run, retired at run end so a late
    /// fire is a harmless no-op).
    RunStarted {
        /// Fire to interrupt this run (Esc/Ctrl+C).
        cancel: CancellationToken,
    },
    /// One engine event (message/error/approval …) from the run's sink
    /// (boxed — `Event` is large).
    Event(Box<locode_core::Event>),
    /// A tool call is awaiting the user's approval (ADR-0017). The loop takes
    /// the responder; the reducer renders the view.
    Approval(crate::approval::ApprovalAsk),
    /// The run reached its terminal state.
    RunFinished(Box<Report>),
    /// The session was reset (`/new`); the UI clears transcript-adjacent state.
    SessionReset,
}

/// Spawn the engine task. Returns the command sender and the message
/// receiver the event loop selects on.
///
/// # Panics ignored — the caller keeps both ends.
///
/// Channel audit (SPEC-TUI bounded-everything rule): the event channel is
/// unbounded but its volume is bounded by turn count — the core is
/// non-streaming (ADR-0005), so every `Event` is a whole message; revisit
/// when streaming deltas land.
#[must_use]
pub fn spawn(
    cli: Cli,
    registry: ProviderRegistry,
) -> (
    tokio::sync::mpsc::UnboundedSender<UiCommand>,
    tokio::sync::mpsc::UnboundedReceiver<EngineMsg>,
) {
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<UiCommand>();
    let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel::<EngineMsg>();

    // Own cli + registry so `/new` can rebuild the session on demand.
    tokio::spawn(async move {
        // Resolved once — `$SHELL` doesn't change over a session.
        let shell = detect_shell();
        // The pack owns user-prompt shaping; resolve it once for the run loop
        // (flag, else the settings default — ADR-0024 §1.4).
        let pack: &'static dyn Pack = match resolve_pack(&cli) {
            Ok(pack) => pack,
            Err(e) => {
                let _ = msg_tx.send(EngineMsg::BuildFailed(e));
                return;
            }
        };
        let mut session = match build_session(&cli, &registry, msg_tx.clone()) {
            Ok((session, model, cwd)) => {
                let _ = msg_tx.send(EngineMsg::Ready {
                    model,
                    cwd,
                    shell: shell.clone(),
                });
                session
            }
            Err(message) => {
                let _ = msg_tx.send(EngineMsg::BuildFailed(message));
                return;
            }
        };
        while let Some(command) = cmd_rx.recv().await {
            match command {
                UiCommand::Submit(text) => {
                    // Clone the handle BEFORE run() (ADR-0018 mandate — run
                    // takes &mut self, so nothing is callable mid-run).
                    let cancel = session.cancel_handle();
                    let _ = msg_tx.send(EngineMsg::RunStarted { cancel });
                    // Pack-faithful prompt shaping, as locode-exec does.
                    let report = session.run_text(pack.shape_user_prompt(&text)).await;
                    let _ = msg_tx.send(EngineMsg::RunFinished(Box::new(report)));
                }
                UiCommand::NewSession => match build_session(&cli, &registry, msg_tx.clone()) {
                    Ok((fresh, model, cwd)) => {
                        session = fresh;
                        let _ = msg_tx.send(EngineMsg::SessionReset);
                        let _ = msg_tx.send(EngineMsg::Ready {
                            model,
                            cwd,
                            shell: shell.clone(),
                        });
                    }
                    Err(message) => {
                        let _ = msg_tx.send(EngineMsg::BuildFailed(message));
                    }
                },
            }
        }
    });
    (cmd_tx, msg_rx)
}

/// Resolve the effective pack: the `--harness` flag, else the settings
/// `harness` default (ADR-0024 §1.4), else `grok`. Settings need a cwd; this
/// pre-session resolution mirrors `build_session`'s (same flags, same layers).
fn resolve_pack(cli: &Cli) -> Result<&'static dyn Pack, String> {
    let harness_name = if let Some(harness) = cli.harness {
        harness.as_str().to_string()
    } else {
        let cwd = match &cli.cwd {
            Some(dir) => dir.clone(),
            None => std::env::current_dir().map_err(|e| e.to_string())?,
        };
        locode_core::load_settings(&cwd, cli.settings.as_deref())
            .settings
            .harness
            .unwrap_or_else(|| "grok".to_string())
    };
    locode_core::resolve(&harness_name).map_err(|e| e.to_string())
}

/// Assemble the session exactly as `locode-exec` does (canonical cwd shared
/// by jail/engine/pack; --yolo lifts the jail). Duplication flagged in the
/// slice plan; a facade helper is a future core proposal.
fn build_session(
    cli: &Cli,
    registry: &ProviderRegistry,
    events: tokio::sync::mpsc::UnboundedSender<EngineMsg>,
) -> Result<(Session, String, String), String> {
    let cwd = match &cli.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let cwd = std::fs::canonicalize(&cwd).map_err(|e| format!("--cwd {}: {e}", cwd.display()))?;
    let cwd_display = home_relative(&cwd);

    let mut host_config = HostConfig::new(&cwd);
    if cli.dangerously_skip_permissions {
        host_config.path_policy = PathPolicy::Unrestricted;
    }
    let host = Arc::new(Host::new(host_config).map_err(|e| e.to_string())?);

    // Settings (ADR-0024): durable defaults under the flags. Interactive mode
    // has no stderr surface, so layer warnings are dropped here; the `-p`
    // headless path prints them (locode-exec).
    let settings = locode_core::load_settings(&cwd, cli.settings.as_deref()).settings;
    let harness_name = match cli.harness {
        Some(harness) => harness.as_str().to_string(),
        None => settings
            .harness
            .clone()
            .unwrap_or_else(|| "grok".to_string()),
    };
    let pack = locode_core::resolve(&harness_name).map_err(|e| e.to_string())?;
    let registry_tools = pack.build_registry(&host);

    // Provider first so the pack env block can name the model (D9).
    let session_id = new_session_id();
    let api_schema = cli
        .api_schema
        .clone()
        .or_else(|| settings.api_schema.clone())
        .unwrap_or_else(|| "anthropic".to_string());
    let built = registry
        .build(
            &api_schema,
            &ProviderInit {
                session_id: session_id.clone(),
                model: settings.model.clone(),
            },
        )
        .map_err(|e| e.to_string())?;
    let (provider, model) = (built.provider, built.model);

    let pack_ctx = PackContext {
        cwd: cwd.clone(),
        os: std::env::consts::OS.to_string(),
        shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
        headless: false,
        is_git_repo: cwd.ancestors().any(|dir| dir.join(".git").exists()),
        model: Some(model.clone()),
        os_version: detect_os_version(),
        timezone: detect_timezone(),
        strip_identity: cli.strip_identity,
    };
    let preamble = pack.preamble(&pack_ctx);

    let config = EngineConfig {
        session_id,
        harness: pack.name().to_string(),
        api_schema: provider.api_schema().to_string(),
        model: model.clone(),
        cwd: cwd.clone(),
        workspace_root: cwd,
        max_turns: None,
        sampling_args: SamplingArgs::default(),
        cache_hint: CacheHint::Standard,
        // The interactive TUI always streams (ADR-0021) — live token render.
        streaming: true,
        // Project-instruction loading (`AGENTS.md`, ADR-0023) — on by default,
        // `--no-project-instructions` opts out.
        instructions: InstructionsConfig {
            enabled: !cli.no_project_instructions,
            ..InstructionsConfig::default()
        },
        ..EngineConfig::default()
    };

    // The approver surfaces asks on the same channel; --yolo makes it
    // auto-allow without ever surfacing UI (ADR-0017 client-side stickiness).
    let approver = Arc::new(crate::approval::TuiApprover::new(
        cli.dangerously_skip_permissions,
        events.clone(),
    ));

    let sink: Box<dyn EventSink> = Box::new(FnSink(move |event| {
        let _ = events.send(EngineMsg::Event(Box::new(event)));
    }));
    let session =
        Session::new(provider, registry_tools, preamble, config, sink).with_approver(approver);
    Ok((session, model, cwd_display))
}

/// Shorten a path for display by replacing a leading `$HOME` with `~`.
fn home_relative(path: &std::path::Path) -> String {
    let Ok(home) = std::env::var("HOME") else {
        return path.display().to_string();
    };
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The shell `run_terminal_cmd` runs commands through, for the status display.
/// Mirrors the host's resolution exactly (grok's `ShellSpec { detect_program:
/// true }` in `locode-host::shell::shell_program_for`): the `$SHELL` basename
/// when it is `bash` or `zsh`, else the host default `bash`. This is a display
/// label — commands still run non-interactively (`<shell> -c`), so it reflects
/// the shell binary, not that `~/.zshrc` is sourced (it isn't; see the shell docs).
fn detect_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .and_then(|s| {
            std::path::Path::new(&s)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        })
        .filter(|base| base == "bash" || base == "zsh")
        .unwrap_or_else(|| "bash".to_string())
}

/// `uname -s -r` for the Claude pack's env `OS Version:` line (D9); `None` off
/// Unix or on probe failure.
fn detect_os_version() -> Option<String> {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("uname")
            .args(["-s", "-r"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Best-effort IANA timezone name for the codex pack's `<environment_context>`,
/// dependency-free: `$TZ` if set, else the `/etc/localtime` symlink target after
/// `zoneinfo/`. `None` omits the `<timezone>` line.
fn detect_timezone() -> Option<String> {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim();
        if !tz.is_empty() {
            return Some(tz.to_string());
        }
    }
    #[cfg(unix)]
    {
        let target = std::fs::read_link("/etc/localtime").ok()?;
        let s = target.to_string_lossy();
        s.split_once("zoneinfo/")
            .map(|(_, name)| name.to_string())
            .filter(|name| !name.is_empty())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// A unique-enough session id (mirrors locode-exec; no uuid dep).
fn new_session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("sess-{now}-{}", std::process::id())
}
