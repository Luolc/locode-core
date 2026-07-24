//! Session assembly + drive (plan §3.2): resolve inputs, build host/pack/
//! provider through the facade, run the engine, emit per `--output-format`.

use std::io::Read;
use std::process::ExitCode;
use std::sync::Arc;

use locode_core::{
    CacheHint, EngineConfig, EventSink, FnSink, Host, HostConfig, InstructionsConfig, NullSink,
    PackContext, PathPolicy, ProviderInit, ProviderRegistry, SamplingArgs, Session,
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
    let cwd = match cli.cwd {
        Some(dir) => dir,
        None => std::env::current_dir()?,
    };
    let cwd = std::fs::canonicalize(&cwd)
        .map_err(|e| PreRunError(format!("--cwd {}: {e}", cwd.display())))?;

    let mut host_config = HostConfig::new(&cwd);
    if cli.dangerously_skip_permissions {
        host_config.path_policy = PathPolicy::Unrestricted;
    }
    let host = Arc::new(Host::new(host_config)?);

    // ---- 3. Provider: registry-resolved (ADR-0015); unknown names and factory
    //         failures (missing env, …) fail BEFORE driving the loop. Built first
    //         so the pack env block can name the model (D9). ----
    let pack = locode_core::resolve(cli.harness.as_str())?;
    let registry = pack.build_registry(&host);
    let session_id = new_session_id();
    let built = providers
        .build(
            &cli.api_schema,
            &ProviderInit {
                session_id: session_id.clone(),
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
        strip_identity: cli.strip_identity,
    };
    let preamble = pack.preamble(&pack_ctx);

    // Pack-specific user-prompt shaping (grok wraps in <user_query>; claude sends
    // it verbatim). The pack owns the shape — the exec layer stays harness-agnostic.
    let user_prompt = pack.shape_user_prompt(&prompt);

    // ---- 5. Engine config + event sink per output mode. ----
    let config = EngineConfig {
        session_id,
        harness: cli.harness.as_str().to_string(),
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
        // `--no-project-instructions` opts out.
        instructions: InstructionsConfig {
            enabled: !cli.no_project_instructions,
            ..InstructionsConfig::default()
        },
        ..EngineConfig::default()
    };
    let sink: Box<dyn EventSink> = match cli.output_format {
        // stream-json writes each event live; the terminal `result` event
        // carries the same Report as json mode.
        OutputFormat::StreamJson => Box::new(FnSink(|event| {
            if in_whole_message_trace(&event) {
                output::write_json_line(&event);
            }
        })),
        // json/text only want the final report — events are dropped.
        OutputFormat::Json | OutputFormat::Text => Box::new(NullSink),
    };

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
