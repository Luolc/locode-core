//! Session assembly + drive (plan §3.2): resolve inputs, build host/pack/
//! provider through the facade, run the engine, emit per `--output-format`.

use std::io::Read;
use std::process::ExitCode;
use std::sync::Arc;

use locode_core::{
    CacheHint, EngineConfig, EventSink, FnSink, Host, HostConfig, NullSink, PackContext,
    PathPolicy, ProviderInit, ProviderRegistry, SamplingArgs, Session, grok,
};

use crate::cli::{Cli, Harness, OutputFormat};
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

    // ---- 3. Pack: tools + preamble (system prompt + <user_info> prefix). ----
    let pack = locode_core::resolve(cli.harness.as_str())?;
    let registry = pack.build_registry(&host);
    let pack_ctx = PackContext {
        cwd: cwd.clone(),
        os: std::env::consts::OS.to_string(),
        shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
        headless: true,
        strip_identity: cli.strip_identity,
    };
    let preamble = pack.preamble(&pack_ctx);

    // The grok system prompt directs the model to the <user_query> tag — wrap
    // the prompt the way the harness expects (pack-specific shaping).
    let user_prompt = match cli.harness {
        Harness::Grok => grok::prompt::user_query(&prompt),
    };

    // ---- 4. Provider: registry-resolved (ADR-0015); unknown names and
    //         factory failures (missing env, …) fail BEFORE driving the loop. ----
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
        ..EngineConfig::default()
    };
    let sink: Box<dyn EventSink> = match cli.output_format {
        // stream-json writes each event live; the terminal `result` event
        // carries the same Report as json mode.
        OutputFormat::StreamJson => Box::new(FnSink(|event| output::write_json_line(&event))),
        // json/text only want the final report — events are dropped.
        OutputFormat::Json | OutputFormat::Text => Box::new(NullSink),
    };

    // ---- 6. Drive to a terminal state (infallible) and emit the artifact. ----
    let mut session = Session::new(provider, registry, preamble, config, sink);
    let report = session.run_text(user_prompt).await;

    match cli.output_format {
        OutputFormat::Json => output::write_json_line(&report),
        OutputFormat::Text => output::write_text(report.final_message.as_deref().unwrap_or("")),
        OutputFormat::StreamJson => {} // the result event already streamed
    }
    Ok(output::exit_code(report.status))
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

/// A unique-enough session id for a headless run (no uuid dep in v0).
fn new_session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("sess-{now}-{}", std::process::id())
}
