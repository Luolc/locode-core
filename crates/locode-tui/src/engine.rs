//! The engine task: owns the `Session`, executes submits, and streams
//! engine output to the UI over typed channels — the minimal in-process form
//! of "the TUI is a pure client" (codex's protocol-seam lesson, study §3.2).

use std::sync::Arc;

use locode_core::{
    CacheHint, CancellationToken, EngineConfig, EventSink, FnSink, Host, HostConfig,
    InstructionsConfig, Pack, PackContext, PathPolicy, ProviderInit, ProviderRegistry, Report,
    SamplingArgs, Session, SkillsConfig,
};

use crate::cli::Cli;

/// Commands from the UI to the engine task.
#[derive(Debug)]
pub enum UiCommand {
    /// Run one turn with this (unwrapped) prompt text.
    Submit(String),
    /// Discard the current `Session` and build a fresh one (`/new`).
    NewSession,
    /// Swap the running session's model, and persist it as the next session's
    /// default (`/model <id>`).
    SetModel(String),
}

/// The context occupancy recovered for a resumed session: **exact** when the
/// rollout carries `usage` records (the last run's input + cache-read + output),
/// else a byte-derived estimate (`serialized bytes / 4`, codex's heuristic) shown
/// as `~N` until the first real usage report replaces it.
#[derive(Debug, Clone, Copy)]
pub struct RecoveredContext {
    /// The recovered/estimated token count.
    pub tokens: u64,
    /// Whether it is an estimate (`~` in the footer) rather than reported usage.
    pub estimated: bool,
}

/// Messages from the engine task to the UI.
#[derive(Debug)]
pub enum EngineMsg {
    /// Session assembled; the app is ready to accept prompts.
    Ready {
        /// User-invocable skills, to register as slash commands (ADR-0026 §4).
        skills: Vec<locode_skills::Skill>,
        /// Resolved model id (for the status display).
        model: String,
        /// Working directory, home-shortened (for the status display).
        cwd: String,
        /// Shell that `run_terminal_cmd` uses (for the status display), resolved
        /// with grok's `$SHELL` rule (see `detect_shell`).
        shell: String,
        /// For a **resumed** session: the recovered context occupancy.
        /// `None` = fresh session (footer resets to 0).
        context: Option<RecoveredContext>,
    },
    /// Session assembly failed pre-run (bad schema, missing key, …).
    BuildFailed(String),
    /// `/model` finished: the model now in use (unchanged when the switch failed) and
    /// the line to show. Carrying the resolved model — not the requested one — is what
    /// keeps the status bar honest when a factory resolves something else or refuses.
    ModelChanged {
        /// The model actually in use after the attempt.
        model: String,
        /// User-facing outcome.
        message: String,
    },
    /// A recovered user prompt from a resumed session's transcript — rendered
    /// like a live submit echo (the generic event path deliberately drops
    /// plain user text because live submits echo it themselves).
    ReplayedPrompt(String),
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
        // Build first, THEN resolve the pack from the built identity — a
        // resumed session's recorded harness wins over flags/settings
        // (ADR-0024 §2.5), so shaping must follow the build.
        let mut built = match build_session(&cli, &registry, msg_tx.clone(), true) {
            Ok(built) => built,
            Err(message) => {
                let _ = msg_tx.send(EngineMsg::BuildFailed(message));
                return;
            }
        };
        let mut pack: &'static dyn Pack = match locode_core::resolve(&built.harness) {
            Ok(pack) => pack,
            Err(e) => {
                let _ = msg_tx.send(EngineMsg::BuildFailed(e.to_string()));
                return;
            }
        };
        let _ = msg_tx.send(EngineMsg::Ready {
            model: built.model.clone(),
            cwd: built.cwd_display.clone(),
            shell: shell.clone(),
            context: built.context,
            skills: built.skills.clone(),
        });
        // A resumed session replays its recovered transcript into the UI.
        // Assistant messages + tool results ride the normal event path (tool
        // cells pair up exactly as live); plain user prompts need the
        // submit-echo path instead, and machinery text (the pack preamble,
        // injected `<system-reminder>`s, pack prompt wrappers) is unwrapped or
        // skipped — it was never rendered live either.
        for message in built.replay.drain(..) {
            replay_message(&msg_tx, message);
        }
        while let Some(command) = cmd_rx.recv().await {
            match command {
                UiCommand::Submit(text) => {
                    // Clone the handle BEFORE run() (ADR-0018 mandate — run
                    // takes &mut self, so nothing is callable mid-run).
                    let cancel = built.session.cancel_handle();
                    let _ = msg_tx.send(EngineMsg::RunStarted { cancel });
                    // Pack-faithful prompt shaping, as locode-exec does.
                    let report = built.session.run_text(pack.shape_user_prompt(&text)).await;
                    let _ = msg_tx.send(EngineMsg::RunFinished(Box::new(report)));
                }
                UiCommand::SetModel(model) => {
                    let _ = msg_tx.send(switch_model(&mut built, &registry, &model));
                }
                // `/new` always starts FRESH — the resume intent does not stick.
                UiCommand::NewSession => {
                    match build_session(&cli, &registry, msg_tx.clone(), false) {
                        Ok(fresh) => {
                            built = fresh;
                            pack = match locode_core::resolve(&built.harness) {
                                Ok(pack) => pack,
                                Err(e) => {
                                    let _ = msg_tx.send(EngineMsg::BuildFailed(e.to_string()));
                                    return;
                                }
                            };
                            let _ = msg_tx.send(EngineMsg::SessionReset);
                            let _ = msg_tx.send(EngineMsg::Ready {
                                model: built.model.clone(),
                                cwd: built.cwd_display.clone(),
                                shell: shell.clone(),
                                context: None,
                                skills: built.skills.clone(),
                            });
                        }
                        Err(message) => {
                            let _ = msg_tx.send(EngineMsg::BuildFailed(message));
                        }
                    }
                }
            }
        }
    });
    (cmd_tx, msg_rx)
}

/// Assemble the session exactly as `locode-exec` does (canonical cwd shared
/// by jail/engine/pack; --yolo lifts the jail). Duplication flagged in the
/// slice plan; a facade helper is a future core proposal.
/// The recovered context for a resumed rollout: exact from the last `usage`
/// record when present (input + cache-read + output — what the next turn starts
/// from), else a byte-derived estimate (`serialized bytes / 4` — codex's
/// `APPROX_BYTES_PER_TOKEN`).
fn recovered_context(contents: &locode_core::RolloutContents) -> RecoveredContext {
    if let Some(usage) = &contents.last_usage {
        return RecoveredContext {
            tokens: usage.context_tokens(),
            estimated: false,
        };
    }
    let bytes: usize = contents
        .history
        .iter()
        .map(|m| serde_json::to_string(m).map_or(0, |s| s.len()))
        .sum();
    RecoveredContext {
        tokens: (bytes / 4) as u64,
        estimated: true,
    }
}

/// Send one recovered message to the UI the way it would have rendered live.
fn replay_message(
    msg_tx: &tokio::sync::mpsc::UnboundedSender<EngineMsg>,
    message: locode_core::Message,
) {
    use locode_core::{ContentBlock, Event, Message, Role};
    match message.role {
        // The pack preamble is never rendered.
        Role::System | Role::Developer => {}
        Role::Assistant => {
            let _ = msg_tx.send(EngineMsg::Event(Box::new(Event::Message { message })));
        }
        Role::User => {
            // Split: tool results pair with the already-replayed tool calls via
            // the normal event path; plain text is a prompt echo — minus the
            // engine-injected reminders and the pack's prompt wrapper.
            let mut tool_results = Vec::new();
            for block in message.content {
                match block {
                    ContentBlock::ToolResult { .. } => tool_results.push(block),
                    ContentBlock::Text { text } => {
                        if text.trim_start().starts_with("<system-reminder>") {
                            continue; // injected machinery, never rendered live
                        }
                        let _ = msg_tx.send(EngineMsg::ReplayedPrompt(unwrap_user_query(&text)));
                    }
                    _ => {}
                }
            }
            if !tool_results.is_empty() {
                let _ = msg_tx.send(EngineMsg::Event(Box::new(Event::Message {
                    message: Message {
                        role: Role::User,
                        content: tool_results,
                    },
                })));
            }
        }
    }
}

/// Strip grok's `<user_query>` shaping wrapper for display (the live UI echoes
/// the raw prompt *before* `shape_user_prompt`; the trace holds the shaped
/// form). Claude/codex shape verbatim, so this is a no-op for them.
fn unwrap_user_query(text: &str) -> String {
    let trimmed = text.trim();
    trimmed
        .strip_prefix("<user_query>")
        .and_then(|rest| rest.strip_suffix("</user_query>"))
        .map_or_else(|| text.to_string(), |inner| inner.trim().to_string())
}

/// One built session plus its resolved identity and (for resume) the recovered
/// transcript to replay into the UI.
struct BuiltSession {
    session: Session,
    model: String,
    cwd_display: String,
    /// The effective harness (a resumed session's recorded pack wins).
    harness: String,
    /// Recovered messages to render (empty for a fresh session).
    replay: Vec<locode_core::Message>,
    /// Recovered context occupancy (`None` for a fresh session).
    context: Option<RecoveredContext>,
    /// Skills discovered for this session, so the UI can offer the user-invocable
    /// ones as slash commands (ADR-0026 §4). Discovered here rather than in the UI so
    /// both halves see the *same* resolved settings.
    skills: Vec<locode_skills::Skill>,
    /// The wire this session is on, kept so `/model` can rebuild its provider without
    /// re-resolving the whole session.
    api_schema: String,
    /// The session id, which some factories fold into the provider (the
    /// `openai-responses` cache key).
    session_id: String,
}

/// `/model <id>`: swap the provider on the live session, then remember the choice.
///
/// Order matters. The **session** changes first and the settings write is best-effort
/// after it: a failed write must not leave the user on a model the status bar says they
/// left. The reported model is the one the factory *resolved*, not the one requested, so
/// a redirected or refused switch cannot leave the footer lying.
fn switch_model(built: &mut BuiltSession, registry: &ProviderRegistry, model: &str) -> EngineMsg {
    let init = ProviderInit {
        session_id: built.session_id.clone(),
        model: Some(model.to_string()),
    };
    let rebuilt = match registry.build(&built.api_schema, &init) {
        Ok(rebuilt) => rebuilt,
        Err(e) => {
            return EngineMsg::ModelChanged {
                model: built.model.clone(),
                message: format!("cannot switch to {model}: {e}"),
            };
        }
    };
    let resolved = rebuilt.model.clone();
    let notice = built.session.set_model(rebuilt.provider, &resolved);
    built.session.announce(notice);
    built.model.clone_from(&resolved);

    // Persisted to the user-global file — what the NEXT session starts with; the running
    // one already switched. Both reference harnesses persist globally and neither has a
    // project-scoped model (Claude Code `userSettings.model`; grok `[models].default`).
    let saved = locode_core::locode_home()
        .map_err(|e| e.clone())
        .and_then(|home| {
            locode_core::update_user_setting(
                &home,
                "model",
                serde_json::Value::String(resolved.clone()),
            )
        });
    let message = match saved {
        Ok(_) => format!("model: {resolved} (also saved as the default)"),
        Err(e) => format!("model: {resolved} — switched, but saving the default failed: {e}"),
    };
    EngineMsg::ModelChanged {
        model: resolved,
        message,
    }
}

#[allow(clippy::too_many_lines)] // linear assembly, mirrored from locode-exec
fn build_session(
    cli: &Cli,
    registry: &ProviderRegistry,
    events: tokio::sync::mpsc::UnboundedSender<EngineMsg>,
    allow_resume: bool,
) -> Result<BuiltSession, String> {
    let cwd = match &cli.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let cwd = std::fs::canonicalize(&cwd).map_err(|e| format!("--cwd {}: {e}", cwd.display()))?;
    let cwd_display = home_relative(&cwd);

    let add_dirs = locode_exec::canonicalize_add_dirs(&cli.add_dir).map_err(|e| e.0)?;
    let mut host_config = HostConfig::new(&cwd);
    host_config.extra_roots.clone_from(&add_dirs);
    // Unrestricted is the default (ADR-0008 amendment 2026-07-24) — see the headless
    // path for the rationale. `--restricted` opts back in.
    if !cli.restricted {
        host_config.path_policy = PathPolicy::Unrestricted;
    }
    let host = Arc::new(Host::new(host_config).map_err(|e| e.to_string())?);

    // Settings (ADR-0024): durable defaults under the flags. Interactive mode
    // has no stderr surface, so layer warnings are dropped here; the `-p`
    // headless path prints them (locode-exec).
    let settings_load = locode_core::load_settings(&cwd, cli.settings.as_deref());
    let extends_dirs = settings_load.extends_dirs;
    let settings = settings_load.settings;

    // Resume target (`-c`/`-r`, ADR-0024 §2.5): the rollout header wins the
    // identity; an explicit conflicting flag errors (no silent pack/wire swap).
    let resumed = if allow_resume && (cli.continue_session || cli.resume.is_some()) {
        let home = locode_core::locode_home()?;
        let root = home.join("sessions");
        let path = if let Some(id) = &cli.resume {
            locode_core::find_rollout_by_id(&root, &cwd, id)
                .ok_or_else(|| format!("--resume: no session `{id}` found"))?
        } else {
            locode_core::find_latest_rollout(&root, &cwd)
                .ok_or_else(|| format!("--continue: no session found for {}", cwd.display()))?
        };
        let contents = locode_core::read_rollout(&path)?;
        if let Some(flag) = cli.harness
            && flag.as_str() != contents.meta.harness
        {
            return Err(format!(
                "--harness {} conflicts with the resumed session's harness `{}`",
                flag.as_str(),
                contents.meta.harness
            ));
        }
        if let Some(flag) = &cli.api_schema
            && flag != &contents.meta.api_schema
        {
            return Err(format!(
                "--api-schema {flag} conflicts with the resumed session's wire `{}`",
                contents.meta.api_schema
            ));
        }
        Some((path, contents))
    } else {
        None
    };

    let harness_name = match (&resumed, cli.harness) {
        (Some((_, contents)), _) => contents.meta.harness.clone(),
        (None, Some(harness)) => harness.as_str().to_string(),
        (None, None) => settings
            .harness
            .clone()
            .unwrap_or_else(|| "claude".to_string()),
    };
    let pack = locode_core::resolve(&harness_name).map_err(|e| e.to_string())?;
    let registry_tools = pack.build_registry(&host);

    // Provider first so the pack env block can name the model (D9).
    let session_id = match &resumed {
        Some((_, contents)) => contents.meta.session_id.clone(),
        None => new_session_id(),
    };
    let api_schema = match &resumed {
        Some((_, contents)) => contents.meta.api_schema.clone(),
        None => cli
            .api_schema
            .clone()
            .or_else(|| settings.api_schema.clone())
            .unwrap_or_else(|| "anthropic".to_string()),
    };
    // The model is deliberately NOT recovered from the header (user decision
    // 2026-07-24): flag > settings > the wire's default, resumed or not.
    let model_override = cli.model.clone().or_else(|| settings.model.clone());
    let built = registry
        .build(
            &api_schema,
            &ProviderInit {
                session_id: session_id.clone(),
                model: model_override,
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
    // A resumed session's preamble IS the recovered history (the pack preamble
    // is inside it — it was traced); Init then carries the full transcript.
    let preamble = match &resumed {
        Some((_, contents)) => contents.history.clone(),
        None => pack.preamble(&pack_ctx),
    };

    let config_session_id = session_id.clone();
    let config = EngineConfig {
        session_id,
        harness: pack.name().to_string(),
        api_schema: provider.api_schema().to_string(),
        model: model.clone(),
        cwd: cwd.clone(),
        workspace_root: cwd.clone(),
        max_turns: None,
        sampling_args: SamplingArgs::default(),
        cache_hint: CacheHint::Standard,
        // The interactive TUI always streams (ADR-0021) — live token render.
        streaming: true,
        // Project-instruction loading (`AGENTS.md`, ADR-0023) — on by default,
        // `--no-project-instructions` opts out.
        instructions: InstructionsConfig {
            enabled: !cli.no_project_instructions,
            root_stop_pattern: settings.root_stop_pattern.clone(),
            extends_dirs: extends_dirs.clone(),
            extra_roots: add_dirs.clone(),
            ..InstructionsConfig::default()
        },
        // Skills (ADR-0025): the same resolved settings feed discovery, which is what
        // keeps "settings before discovery" an invariant rather than a convention.
        skills: SkillsConfig {
            extends_dirs,
            extra_roots: add_dirs,
            extra: settings.skills_extra.clone(),
            ..SkillsConfig::enabled()
        },
        ..EngineConfig::default()
    };

    // The command menu offers the same skills the model is told about, from the same
    // resolved settings — discovered once here rather than re-walked by the UI.
    // Warnings stay with the engine, which reports them on every run.
    let skills = if config.skills.enabled {
        locode_skills::discover(&config.cwd, &config.skills).skills
    } else {
        Vec::new()
    };

    // The approver surfaces asks on the same channel; --yolo makes it
    // auto-allow without ever surfacing UI (ADR-0017 client-side stickiness).
    let approver = Arc::new(crate::approval::TuiApprover::new(
        !cli.restricted,
        events.clone(),
    ));

    // Session trace (ADR-0024 §2): decoration on the sink, same as the headless
    // path. Interactive mode has no stderr surface; a trace failure silently
    // disables the writer (the headless path warns).
    let mut trace = locode_core::locode_home()
        .ok()
        .filter(|_| !cli.no_session_persistence)
        .and_then(|home| {
            let root = home.join("sessions");
            match &resumed {
                Some((path, _)) => locode_core::TraceWriter::resume(path.clone(), root).ok(),
                None => Some(locode_core::TraceWriter::new(
                    root,
                    locode_core::TraceExtras {
                        cli_version: env!("CARGO_PKG_VERSION").to_string(),
                        git: git_meta(&cwd),
                        ..Default::default()
                    },
                )),
            }
        });
    let sink: Box<dyn EventSink> = Box::new(FnSink(move |event| {
        if let Some(trace) = trace.as_mut() {
            trace.on_event(&event);
        }
        let _ = events.send(EngineMsg::Event(Box::new(event)));
    }));
    let session =
        Session::new(provider, registry_tools, preamble, config, sink).with_approver(approver);
    let context = resumed
        .as_ref()
        .map(|(_, contents)| recovered_context(contents));
    let replay = resumed
        .map(|(_, contents)| contents.history)
        .unwrap_or_default();
    Ok(BuiltSession {
        session,
        model,
        cwd_display,
        harness: harness_name,
        replay,
        context,
        skills,
        api_schema,
        session_id: config_session_id,
    })
}

/// Best-effort git provenance for the trace header (ADR-0024 §2.3) — mirrors
/// `locode-exec`'s helper (the same deliberate duplication as `detect_shell`/
/// `detect_os_version` between the two front-ends).
fn git_meta(cwd: &std::path::Path) -> Option<locode_core::GitMeta> {
    if !cwd.ancestors().any(|dir| dir.join(".git").exists()) {
        return None;
    }
    let run = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    };
    Some(locode_core::GitMeta {
        root: run(&["rev-parse", "--show-toplevel"]).map(std::path::PathBuf::from),
        branch: run(&["rev-parse", "--abbrev-ref", "HEAD"]),
        head: run(&["rev-parse", "HEAD"]),
        remote: run(&["remote", "get-url", "origin"]),
    })
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

#[cfg(test)]
mod replay_tests {
    use super::*;
    use locode_core::{ContentBlock, Message, Role};

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<EngineMsg>) -> Vec<EngineMsg> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    #[test]
    fn replay_routes_prompts_reminders_and_tools() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // System preamble: dropped.
        replay_message(&tx, text_msg(Role::System, "base prompt"));
        // Injected reminder: dropped.
        replay_message(
            &tx,
            text_msg(Role::User, "<system-reminder>\nrules\n</system-reminder>"),
        );
        // A grok-shaped prompt: unwrapped and echoed.
        replay_message(
            &tx,
            text_msg(Role::User, "<user_query>\nfix the bug\n</user_query>"),
        );
        // An assistant turn: forwarded as a normal event.
        replay_message(&tx, text_msg(Role::Assistant, "done"));
        // A tool result: forwarded as a normal event (pairs with pending calls).
        replay_message(
            &tx,
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: vec![],
                    is_error: false,
                }],
            },
        );

        let msgs = drain(&mut rx);
        assert_eq!(msgs.len(), 3, "system + reminder dropped");
        assert!(
            matches!(&msgs[0], EngineMsg::ReplayedPrompt(p) if p == "fix the bug"),
            "prompt unwrapped"
        );
        assert!(matches!(&msgs[1], EngineMsg::Event(e)
            if matches!(&**e, locode_core::Event::Message { message } if message.role == Role::Assistant)));
        assert!(matches!(&msgs[2], EngineMsg::Event(e)
            if matches!(&**e, locode_core::Event::Message { message } if message.role == Role::User)));
    }

    #[test]
    fn unwrap_user_query_is_a_noop_for_verbatim_packs() {
        assert_eq!(
            unwrap_user_query("plain claude prompt"),
            "plain claude prompt"
        );
        assert_eq!(unwrap_user_query("<user_query>\nhi\n</user_query>"), "hi");
    }
}
