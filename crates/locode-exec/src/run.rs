//! Session assembly + drive (plan §3.2): resolve inputs, build host/pack/
//! provider through the facade, run the engine, emit per `--output-format`.

use std::io::Read;
use std::process::ExitCode;
use std::sync::Arc;

use locode_core::{
    CacheHint, EngineConfig, EventSink, FnSink, Host, HostConfig, InstructionsConfig, PackContext,
    PathPolicy, ProviderInit, ProviderRegistry, SamplingArgs, Session,
};

use crate::cli::{Cli, OutputFormat};
use crate::output;

/// A pre-run failure (config/setup — no report exists yet): stderr + exit 1.
pub struct PreRunError(pub String);

impl<E: std::fmt::Display> From<E> for PreRunError {
    fn from(e: E) -> Self {
        PreRunError(e.to_string())
    }
}

/// Build and drive one session; returns the process exit code.
///
/// Every terminal state of a *started* run yields a report (the engine's
/// `run()` is infallible) — only pre-run setup can fail here.
///
/// # Errors
/// [`PreRunError`] on config/setup failures before a run exists (bad `--cwd`,
/// unknown/misconfigured provider, empty prompt): stderr + exit 1, nothing on
/// stdout.
pub async fn run(cli: Cli, providers: &ProviderRegistry) -> Result<ExitCode, PreRunError> {
    // ---- 0. SIGTERM handler (ADR-0018, Task 21): installed before any
    //         pre-run work so a pre-run SIGTERM exits 1 cleanly; armed with
    //         the session's cancel handle once one exists. ----
    #[cfg(unix)]
    let cancel_slot = crate::signal::install_sigterm();

    // ---- 1. Prompt: positional, or stdin when absent / `-`. ----
    let prompt = resolve_prompt(cli.prompt.as_deref())?;

    // ---- 2. Workspace root: canonicalize FIRST, then hand the SAME canonical
    //         path to the host (jail root), the engine (cwd), and the pack
    //         (prompt context) — they must agree (STATUS concern #7). ----
    let cwd = match &cli.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir()?,
    };
    let cwd = std::fs::canonicalize(&cwd)
        .map_err(|e| PreRunError(format!("--cwd {}: {e}", cwd.display())))?;

    let mut host_config = HostConfig::new(&cwd);
    if cli.dangerously_skip_permissions {
        host_config.path_policy = PathPolicy::Unrestricted;
    }
    let host = Arc::new(Host::new(host_config)?);

    // ---- 2b. Settings (ADR-0024): the durable defaults *under* the flags —
    //          an explicit flag/env always wins. Layer degradations surface as
    //          stderr warnings, never hard errors. ----
    let settings_load = locode_core::load_settings(&cwd, cli.settings.as_deref());
    for warning in &settings_load.warnings {
        output::warning_line(warning);
    }
    let settings = settings_load.settings;

    // ---- 2c. Resume target (`-c`/`-r`, ADR-0024 §2.5): recover the transcript
    //          and the run identity (pack/wire/model/id) from the rollout header;
    //          an explicit conflicting flag errors rather than silently swapping
    //          a session's pack or wire mid-transcript. ----
    let identity = resolve_identity(&cli, &cwd, &settings)?;

    // ---- 3. Provider: registry-resolved (ADR-0015); unknown names and factory
    //         failures (missing env, …) fail BEFORE driving the loop. Built first
    //         so the pack env block can name the model (D9). ----
    let pack = locode_core::resolve(&identity.harness)?;
    let registry = pack.build_registry(&host);
    let session_id = identity.session_id.clone();
    let built = providers
        .build(
            &identity.api_schema,
            &ProviderInit {
                session_id: session_id.clone(),
                model: identity.model_override.clone(),
            },
        )
        .map_err(|e| PreRunError(e.to_string()))?;
    let (provider, model) = (built.provider, built.model);

    // ---- 3b. Wire requirement: a pack whose tools only round-trip on a specific
    //          wire (codex's freeform `apply_patch` → openai-responses, D5) rejects
    //          a mismatched `--api-schema` here, before the loop. ----
    enforce_wire_requirement(pack, provider.api_schema())?;

    // ---- 4. Pack: preamble (system prompt + env + first-turn reminder). ----
    let pack_ctx = PackContext {
        cwd: cwd.clone(),
        os: std::env::consts::OS.to_string(),
        shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
        headless: true,
        is_git_repo: detect_git_repo(&cwd),
        model: Some(model.clone()),
        os_version: os_version(),
        timezone: timezone(),
        strip_identity: cli.strip_identity,
    };
    // A resumed session's preamble IS the recovered history (the pack preamble
    // is already inside it — it was traced); Init then carries the full
    // transcript, keeping the stream self-sufficient with zero engine changes.
    let preamble = match &identity.resumed {
        Some(resumed) => resumed.history.clone(),
        None => pack.preamble(&pack_ctx),
    };

    // Pack-specific user-prompt shaping (grok wraps in <user_query>; claude sends
    // it verbatim). The pack owns the shape — the exec layer stays harness-agnostic.
    let user_prompt = pack.shape_user_prompt(&prompt);

    // ---- 5. Engine config + event sink per output mode. ----
    let config = EngineConfig {
        session_id,
        harness: pack.name().to_string(),
        api_schema: provider.api_schema().to_string(),
        model,
        cwd: cwd.clone(),
        workspace_root: cwd,
        max_turns: cli.max_turns,
        sampling_args: SamplingArgs::default(),
        cache_hint: CacheHint::Standard,
        // Opt-in headless streaming (`--stream`) — required for unbounded output
        // (Anthropic rejects non-streaming past ~10 min). Off by default keeps
        // `-p` byte-for-byte as it was (ADR-0021).
        streaming: cli.stream,
        // Project-instruction loading (`AGENTS.md`, ADR-0023) — on by default,
        // `--no-project-instructions` opts out. `root_stop_pattern` threads
        // through from settings (ADR-0024 §1.4; matching activates in S2).
        instructions: InstructionsConfig {
            enabled: !cli.no_project_instructions,
            root_stop_pattern: settings.root_stop_pattern.clone(),
            ..InstructionsConfig::default()
        },
        ..EngineConfig::default()
    };
    // ---- 5b. Session trace (ADR-0024 §2): every run appends a rollout under
    //          `<locode home>/sessions/<encoded-cwd>/`. See `build_trace_writer`.
    let mut trace = build_trace_writer(&cli, &identity, &config.cwd);

    let sink = make_sink(cli.output_format, trace.take());

    // ---- 6. Drive to a terminal state (infallible) and emit the artifact. ----
    let mut session = Session::new(provider, registry, preamble, config, sink);
    #[cfg(unix)]
    crate::signal::arm(&cancel_slot, session.cancel_handle());
    let report = session.run_text(user_prompt).await;

    match cli.output_format {
        OutputFormat::Json => output::write_json_line(&report),
        OutputFormat::Text => output::write_text(report.final_message.as_deref().unwrap_or("")),
        OutputFormat::StreamJson => {} // the result event already streamed
    }
    Ok(output::exit_code(report.status))
}

/// A recovered session (`-c`/`-r`): its rollout path + replayable history.
struct ResumedSession {
    path: std::path::PathBuf,
    history: Vec<locode_core::Message>,
}

/// The run identity: pack/wire/model/session-id, from flags, settings, and (for
/// `-c`/`-r`) the rollout header — which wins over settings but loses to an
/// explicit flag only by *erroring* (no silent pack/wire swap mid-transcript).
struct RunIdentity {
    harness: String,
    api_schema: String,
    model_override: Option<String>,
    session_id: String,
    resumed: Option<ResumedSession>,
}

fn resolve_identity(
    cli: &Cli,
    cwd: &std::path::Path,
    settings: &locode_core::Settings,
) -> Result<RunIdentity, PreRunError> {
    // Locate + read the rollout when resuming.
    let recovered = if cli.continue_session || cli.resume.is_some() {
        let home = locode_core::locode_home().map_err(PreRunError)?;
        let root = home.join("sessions");
        let path = if let Some(id) = &cli.resume {
            locode_core::find_rollout_by_id(&root, cwd, id)
                .ok_or_else(|| PreRunError(format!("--resume: no session `{id}` found")))?
        } else {
            locode_core::find_latest_rollout(&root, cwd).ok_or_else(|| {
                PreRunError(format!(
                    "--continue: no session found for {}",
                    cwd.display()
                ))
            })?
        };
        let contents = locode_core::read_rollout(&path).map_err(PreRunError)?;
        Some((path, contents))
    } else {
        None
    };

    if let Some((path, contents)) = recovered {
        let meta = contents.meta;
        // Explicit flags may confirm the recorded identity, never change it.
        if let Some(flag) = cli.harness
            && flag.as_str() != meta.harness
        {
            return Err(PreRunError(format!(
                "--harness {} conflicts with the resumed session's harness `{}`",
                flag.as_str(),
                meta.harness
            )));
        }
        if let Some(flag) = &cli.api_schema
            && flag != &meta.api_schema
        {
            return Err(PreRunError(format!(
                "--api-schema {flag} conflicts with the resumed session's wire `{}` \
                 (a session never crosses wires)",
                meta.api_schema
            )));
        }
        return Ok(RunIdentity {
            harness: meta.harness.clone(),
            api_schema: meta.api_schema.clone(),
            // The model is deliberately NOT recovered from the header (user
            // decision 2026-07-24): resume resolves it exactly like a fresh
            // run — flag > settings > the wire's default. Pack/wire stay
            // header-bound because they affect transcript validity; the model
            // doesn't, and yesterday's model should not leak into today.
            model_override: cli.model.clone().or_else(|| settings.model.clone()),
            session_id: meta.session_id.clone(),
            resumed: Some(ResumedSession {
                path,
                history: contents.history,
            }),
        });
    }

    Ok(RunIdentity {
        harness: match cli.harness {
            Some(harness) => harness.as_str().to_string(),
            None => settings
                .harness
                .clone()
                .unwrap_or_else(|| "claude".to_string()),
        },
        api_schema: cli
            .api_schema
            .clone()
            .or_else(|| settings.api_schema.clone())
            .unwrap_or_else(|| "anthropic".to_string()),
        model_override: cli.model.clone().or_else(|| settings.model.clone()),
        session_id: new_session_id(),
        resumed: None,
    })
}

/// The session-trace writer (ADR-0024 §2): decoration over the event sink, so
/// tracing needs zero engine changes. A resumed run reopens the same rollout for
/// appending; a fresh run creates a new one. `None` — no rollout written — when
/// there is no `~/.locode` home, `--no-session-persistence` is set, or a resume
/// reopen fails (warned, never fatal).
fn build_trace_writer(
    cli: &Cli,
    identity: &RunIdentity,
    cwd: &std::path::Path,
) -> Option<locode_core::TraceWriter> {
    let root = locode_core::locode_home()
        .ok()
        .filter(|_| !cli.no_session_persistence)?
        .join("sessions");
    match &identity.resumed {
        // Reopen the same rollout for appending (id and file continue).
        Some(resumed) => locode_core::TraceWriter::resume(resumed.path.clone(), root)
            .map_err(|e| output::warning_line(&format!("trace: {e}; tracing disabled")))
            .ok(),
        None => Some(locode_core::TraceWriter::new(
            root,
            locode_core::TraceExtras {
                cli_version: env!("CARGO_PKG_VERSION").to_string(),
                git: git_meta(cwd),
                ..Default::default()
            },
        )),
    }
}

/// The event sink for `output_format`, decorated with the session-trace writer
/// (ADR-0024 §2): every event feeds the trace first; a trace failure warns once
/// and disables tracing, never the run. `stream-json` additionally writes the
/// whole-message JSONL to stdout; `json`/`text` emit nothing per-event.
fn make_sink(
    output_format: OutputFormat,
    mut trace: Option<locode_core::TraceWriter>,
) -> Box<dyn EventSink> {
    let stream = matches!(output_format, OutputFormat::StreamJson);
    Box::new(FnSink(move |event| {
        if let Some(writer) = trace.as_mut() {
            writer.on_event(&event);
            if let Some(e) = writer.take_error() {
                output::warning_line(&format!("trace: {e}; tracing disabled"));
            }
        }
        if stream && in_whole_message_trace(&event) {
            output::write_json_line(&event);
        }
    }))
}

/// Reject a `--api-schema` a pack's tools can't round-trip on (codex's freeform
/// `apply_patch` requires the OpenAI Responses wire, D5). `mock` (keyless CI) is
/// the universal escape hatch and is always allowed, independent of the pack list.
fn enforce_wire_requirement(pack: &dyn locode_core::Pack, schema: &str) -> Result<(), PreRunError> {
    if schema != "mock"
        && let Some(required) = pack.required_api_schemas()
        && !required.contains(&schema)
    {
        return Err(PreRunError(format!(
            "harness `{}` requires one of these wires: {}; got `--api-schema {}`",
            pack.name(),
            required.join(", "),
            schema,
        )));
    }
    Ok(())
}

/// Positional prompt, or stdin when absent / `-` (Codex's convention;
/// positional XOR stdin per the plan addendum). Empty → usage error.
fn resolve_prompt(arg: Option<&str>) -> Result<String, PreRunError> {
    let prompt = match arg {
        Some("-") | None => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
        Some(text) => text.to_string(),
    };
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(PreRunError(
            "no prompt: pass it as the positional argument or on stdin".to_string(),
        ));
    }
    Ok(prompt)
}

/// Best-effort git provenance for the trace header (ADR-0024 §2.3) — one
/// `git rev-parse` + one `remote get-url`, all fields optional; `None` when not
/// in a repo at all.
fn git_meta(cwd: &std::path::Path) -> Option<locode_core::GitMeta> {
    if !detect_git_repo(cwd) {
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

/// Whether `cwd` is inside a git repository — walk up looking for a `.git` entry
/// (a cheap probe for the Claude pack's env `Is a git repository:` line, D9; no
/// host handle needed in `preamble()`).
fn detect_git_repo(cwd: &std::path::Path) -> bool {
    cwd.ancestors().any(|dir| dir.join(".git").exists())
}

/// `uname -s -r` for the Claude pack's env `OS Version:` line; `None` off Unix or
/// if the probe fails.
fn os_version() -> Option<String> {
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

/// Best-effort IANA timezone name for the codex pack's `<environment_context>`
/// (`America/Los_Angeles`), dependency-free: `$TZ` if set, else the `/etc/localtime`
/// symlink target after `zoneinfo/`. `None` when neither resolves (the pack then
/// omits the `<timezone>` line — codex's field is optional).
fn timezone() -> Option<String> {
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

/// A unique-enough session id for a headless run (no uuid dep in v0).
fn new_session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("sess-{now}-{}", std::process::id())
}

/// Whether an event belongs in the whole-message `stream-json` trace (ADR-0021
/// Q1): everything except live token deltas, which are a TUI-only concern so the
/// trace stays replayable/whole-message even under `--stream`.
fn in_whole_message_trace(event: &locode_core::Event) -> bool {
    !matches!(event, locode_core::Event::MessageDelta { .. })
}

#[cfg(test)]
mod tests {
    use super::{enforce_wire_requirement, in_whole_message_trace};
    use locode_core::{Event, Message, Role};

    #[test]
    fn codex_rejects_a_non_responses_wire() {
        let codex = locode_core::resolve("codex").unwrap();
        // A real, mismatched wire is rejected pre-run with an actionable message.
        let err = enforce_wire_requirement(codex, "anthropic").expect_err("mismatch");
        assert!(err.0.contains("codex"), "{}", err.0);
        assert!(err.0.contains("openai-responses"), "{}", err.0);
        assert!(err.0.contains("anthropic"), "{}", err.0);
        // The required wire and the keyless-CI escape hatch both pass.
        assert!(enforce_wire_requirement(codex, "openai-responses").is_ok());
        assert!(enforce_wire_requirement(codex, "mock").is_ok());
    }

    #[test]
    fn wire_agnostic_packs_accept_any_wire() {
        let grok = locode_core::resolve("grok").unwrap();
        assert!(enforce_wire_requirement(grok, "anthropic").is_ok());
        assert!(enforce_wire_requirement(grok, "openai-responses").is_ok());
    }

    #[test]
    fn stream_json_trace_drops_message_deltas_keeps_whole_messages() {
        // Token deltas are dropped from the trace...
        assert!(!in_whole_message_trace(&Event::MessageDelta {
            text: "tok".into()
        }));
        // ...but whole messages and every other event stay.
        assert!(in_whole_message_trace(&Event::Message {
            message: Message {
                role: Role::Assistant,
                content: vec![],
            },
        }));
        assert!(in_whole_message_trace(&Event::Error {
            message: "e".into()
        }));
    }
}
