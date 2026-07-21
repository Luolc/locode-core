//! The engine task: owns the `Session`, executes submits, and streams
//! engine output to the UI over typed channels — the minimal in-process form
//! of "the TUI is a pure client" (codex's protocol-seam lesson, study §3.2).

use std::sync::Arc;

use locode_core::{
    CacheHint, CancellationToken, EngineConfig, EventSink, FnSink, Host, HostConfig, PackContext,
    PathPolicy, ProviderInit, ProviderRegistry, Report, SamplingArgs, Session, grok,
};

use crate::cli::Cli;

/// Commands from the UI to the engine task.
#[derive(Debug)]
pub enum UiCommand {
    /// Run one turn with this (unwrapped) prompt text.
    Submit(String),
}

/// Messages from the engine task to the UI.
#[derive(Debug)]
pub enum EngineMsg {
    /// Session assembled; the app is ready to accept prompts.
    Ready {
        /// Resolved model id (for the status/footer display).
        model: String,
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
    /// The run reached its terminal state.
    RunFinished(Box<Report>),
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
    cli: &Cli,
    registry: &ProviderRegistry,
) -> (
    tokio::sync::mpsc::UnboundedSender<UiCommand>,
    tokio::sync::mpsc::UnboundedReceiver<EngineMsg>,
) {
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<UiCommand>();
    let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel::<EngineMsg>();

    let build = build_session(cli, registry, msg_tx.clone());
    tokio::spawn(async move {
        let mut session = match build {
            Ok((session, model)) => {
                let _ = msg_tx.send(EngineMsg::Ready { model });
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
                    let report = session.run_text(grok::prompt::user_query(&text)).await;
                    let _ = msg_tx.send(EngineMsg::RunFinished(Box::new(report)));
                }
            }
        }
    });
    (cmd_tx, msg_rx)
}

/// Assemble the session exactly as `locode-exec` does (canonical cwd shared
/// by jail/engine/pack; --yolo lifts the jail). Duplication flagged in the
/// slice plan; a facade helper is a future core proposal.
fn build_session(
    cli: &Cli,
    registry: &ProviderRegistry,
    events: tokio::sync::mpsc::UnboundedSender<EngineMsg>,
) -> Result<(Session, String), String> {
    let cwd = match &cli.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let cwd = std::fs::canonicalize(&cwd).map_err(|e| format!("--cwd {}: {e}", cwd.display()))?;

    let mut host_config = HostConfig::new(&cwd);
    if cli.dangerously_skip_permissions {
        host_config.path_policy = PathPolicy::Unrestricted;
    }
    let host = Arc::new(Host::new(host_config).map_err(|e| e.to_string())?);

    let pack = locode_core::resolve(&cli.harness).map_err(|e| e.to_string())?;
    let registry_tools = pack.build_registry(&host);
    let pack_ctx = PackContext {
        cwd: cwd.clone(),
        os: std::env::consts::OS.to_string(),
        shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
        headless: false,
        strip_identity: cli.strip_identity,
    };
    let preamble = pack.preamble(&pack_ctx);

    let session_id = new_session_id();
    let built = registry
        .build(
            &cli.api_schema,
            &ProviderInit {
                session_id: session_id.clone(),
            },
        )
        .map_err(|e| e.to_string())?;
    let (provider, model) = (built.provider, built.model);

    let config = EngineConfig {
        session_id,
        harness: cli.harness.clone(),
        api_schema: provider.api_schema().to_string(),
        model: model.clone(),
        cwd: cwd.clone(),
        workspace_root: cwd,
        max_turns: None,
        sampling_args: SamplingArgs::default(),
        cache_hint: CacheHint::Standard,
        ..EngineConfig::default()
    };

    let sink: Box<dyn EventSink> = Box::new(FnSink(move |event| {
        let _ = events.send(EngineMsg::Event(Box::new(event)));
    }));
    let session = Session::new(provider, registry_tools, preamble, config, sink);
    Ok((session, model))
}

/// A unique-enough session id (mirrors locode-exec; no uuid dep).
fn new_session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("sess-{now}-{}", std::process::id())
}
