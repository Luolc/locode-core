//! stderr logging (ADR-0009): all diagnostics on stderr, never stdout.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Install the tracing subscriber: fmt layer → **stderr**, filtered by
/// `RUST_LOG` (default `warn`). Codex-exec's exact setup
/// (`fmt::layer().with_writer(std::io::stderr).with_filter(env_filter)`).
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let layer = fmt::layer().with_writer(std::io::stderr);
    // try_init: a second init (tests) is fine to ignore.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();
}
