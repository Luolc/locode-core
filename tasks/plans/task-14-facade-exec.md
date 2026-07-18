# Task 14 — `locode` facade + `locode-exec` minimal headless binary

> Scope: Task 14 in `tasks/todo.md:247`. Two deliverables: (a) the `locode` facade crate
> re-exporting the public driving surface for future `locode-app`; (b) `locode-exec`, the
> minimal headless binary with **strict stdout discipline** (ADR-0009), `--output-format
> {json,text,stream-json}` (ADR-0014), stderr logging, ADR-0009 exit codes, and an optional
> `bundle-rg` cargo feature (ADR-0011). Depends on Tasks 6 (engine/`Session`), 12 (Anthropic
> wire), 13 (grok prompt).
>
> Reference harnesses: **Codex-exec** for stdout discipline / output modes / exit codes /
> stderr logging (`~/dev/coding-cli-survey/submodules/codex/codex-rs/exec/src/…`); **Grok
> Build** for the `bundle-rg` build.rs + runtime self-extract
> (`~/dev/coding-cli-survey/submodules/grok-build/crates/codegen/xai-grok-tools/…`).

---

## 1. Purpose & scope (+ deferred)

**Purpose.** Close the v0 loop: a caller runs `cargo run -p locode-exec -- --prompt "…"
--harness grok --provider anthropic` and gets **exactly one machine-readable artifact on
stdout**, all diagnostics on stderr, and a meaningful exit code (Success Criteria #5,
`SPEC.md:133`; Checkpoint D, `tasks/plan.md:70`). The binary is a *thin reference consumer*
of the library (SPEC "Users" #3, `SPEC.md:24`) — the real UX belongs to `locode-app`. The
`locode` facade defines exactly what that future app (and `locode-exec`) may touch.

**In scope**
- `locode` facade: curated `pub use` of the driving API (`Session`/engine entry, harness +
  provider selection, `Report`/`Status`/`Event`), nothing more.
- `locode-exec`: clap arg parsing; session construction; three output modes; stderr `tracing`;
  exit-code mapping; `--provider mock` for keyless CI.
- Optional `bundle-rg` cargo feature: `build.rs` (download-or-copy static `rg`, `include_bytes!`)
  + runtime self-extract, wired into the `locode-host` `rg` resolver.

**Deferred (reserved seams)** — keep parity with Codex-exec's surface *shape* but not its
breadth:
- `resume` / session persistence, `review` subcommands (`exec/src/cli.rs:166-172`) — session
  durability is deferred (`SPEC.md:144`, Open Q4).
- `--output-schema` / `--json-schema` structured answers (`cli.rs:53`) — envelope-only in v0
  (`SPEC.md:143`, Open Q3); the `structured_output` field already exists in the envelope
  (`locode-protocol/src/lib.rs:157`) but stays `None`.
- `-o/--output-last-message` file sink (`cli.rs:73`), `--color`, image inputs, sandbox flags,
  `-c` config overrides, MCP — all post-v0.
- Windows `bundle-rg` (ripgrep ships `.zip`, no extractor) — falls back to PATH, exactly as
  grok does (`build.rs:84-92`).

---

## 2. Module layout

```
crates/locode/                       # the facade — small on purpose
├── Cargo.toml                        # already depends on all locode-* crates
└── src/lib.rs                        # curated pub use (currently a scaffold stub)

crates/locode-exec/
├── Cargo.toml                        # + clap, tracing, tracing-subscriber; [features] bundle-rg
├── build.rs                          # bundle-rg ONLY: download/copy + emit cfg(bundle_rg)
└── src/
    ├── main.rs                       # #![deny(clippy::print_stdout)]; parse → run → exit
    ├── cli.rs                        # clap Args struct + OutputFormat/Harness/Provider enums
    ├── run.rs                        # build session, drive it, dispatch output mode
    ├── output.rs                     # the 3 emitters (json / text / stream-json)
    ├── logging.rs                    # tracing_subscriber → stderr
    └── rg_bundle.rs                  # #[cfg(feature = "bundle-rg")] include_bytes! + self-extract
```

- **Why a dedicated `run.rs`/`output.rs`** rather than one `main.rs`: Codex-exec keeps the
  stdout-writing code in *named, narrowly-`#[allow(clippy::print_stdout)]`'d* event processors
  (`event_processor_with_jsonl_output.rs:103`, `event_processor_with_human_output.rs:391`),
  so the crate-wide `deny` stays intact and the two legitimate stdout sites are auditable. We
  copy that: `output.rs` holds the *only* `#[allow(clippy::print_stdout)]` in the crate.

---

## 3. Key types & signatures — concrete Rust sketches

### 3.1 clap args (`cli.rs`)

```rust
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

/// Minimal headless runner for the locode engine. One JSON report on stdout (ADR-0009).
#[derive(Parser, Debug)]
#[command(name = "locode-exec", version, about)]
pub struct Cli {
    /// The task prompt. If `-` or omitted, read the prompt from stdin.
    #[arg(long)]
    pub prompt: Option<String>,

    /// Workspace root / working directory (the path jail root). Defaults to CWD.
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// Harness pack selecting the toolset + system prompt.
    #[arg(long, value_enum, default_value_t = Harness::Grok)]
    pub harness: Harness,

    /// Provider wire.
    #[arg(long, value_enum, default_value_t = Provider::Anthropic)]
    pub provider: Provider,

    /// Hard ceiling on sample→dispatch→append turns (ADR-0005).
    #[arg(long, default_value_t = 30)]
    pub max_turns: u32,

    /// stdout contract: `json` = one Report; `stream-json` = Event JSONL; `text` = final msg.
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub output_format: OutputFormat,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum OutputFormat { Json, Text, StreamJson }

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Harness { Grok }                 // one variant in v0; enum reserves the seam

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Provider { Anthropic, Mock }     // `mock` = keyless CI (SPEC.md:115)
```

- **`--harness`/`--provider` as `ValueEnum`s, not free strings.** clap validates them and
  auto-generates `--help`; an unknown value is a clean parse error (exit 2), satisfying Task 8's
  "unknown `--harness` errors clearly" (`todo.md:162`). Codex uses free-form model strings but
  `ValueEnum` for closed sets like `--color` (`cli.rs:300-307`) — we mirror that for our closed
  sets. The single-variant `Harness::Grok` is deliberate: the flag + enum exist now so adding
  `codex`/`claude` (Task 15) is additive, per "seams, not forks" (`minimal-headless…:75`).
- **`--output-format` enum vs Codex's boolean `--json`.** Codex exposes `--json` (JSONL events)
  with the human transcript as the un-flagged default (`cli.rs:63-70`). We instead follow
  **Claude Code's** three-way `--output-format {text,json,stream-json}` (the shape ADR-0014 was
  written against — `claude -p … --output-format stream-json --verbose`, ADR-0014:14) because we
  have *three* distinct artifacts: a summary Report (`json`), the event trace (`stream-json`),
  and a bare final message (`text`). Cite this divergence: Codex conflates "final text" and
  "events"; our `json` is a *summary envelope*, distinct from the `stream-json` trace
  (ADR-0014:50 "Transcript-in-`json`-mode: deferred").

### 3.2 main flow (`main.rs` / `run.rs`)

```rust
// main.rs
#![deny(clippy::print_stdout)]           // ADR-0009 / SPEC.md:125 — stdout is sacred
mod cli; mod run; mod output; mod logging;
#[cfg(feature = "bundle-rg")] mod rg_bundle;

fn main() -> std::process::ExitCode {
    let cli = cli::Cli::parse();          // clap errors → exit 2 automatically
    logging::init();                      // tracing → stderr (never stdout)
    // Run the async engine on a tokio runtime; map the outcome to an exit code.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let outcome = rt.block_on(run::run(cli));
    output::exit_code(outcome)            // ExitCode, NOT process::exit — lets stdout flush
}
```

```rust
// run.rs — build the session, drive it, emit in the chosen mode.
pub async fn run(cli: Cli) -> Outcome {
    // 1. Resolve prompt (arg or stdin).
    let prompt = read_prompt(&cli)?;                 // stdin fallback like Codex (cli.rs:81-85)
    let cwd = cli.cwd.unwrap_or(std::env::current_dir()?);

    // 2. bundle-rg: self-extract the embedded rg and hand its path to the host resolver
    //    BEFORE building the engine (so Grep/Glob resolve it). No-op without the feature.
    #[cfg(feature = "bundle-rg")]
    let bundled_rg: Option<PathBuf> = rg_bundle::extract().ok();
    #[cfg(not(feature = "bundle-rg"))]
    let bundled_rg: Option<PathBuf> = None;

    // 3. Assemble via the facade. The Host is built with the bundled rg path injected as the
    //    "host-provided bundled path" tier of the resolver (ADR-0011 order b).
    let host = locode::HostBuilder::new(&cwd).bundled_rg(bundled_rg).build()?;
    let pack = locode::pack_for(cli.harness)?;       // grok (Task 8) — tools + prompt (Task 13)
    let provider = locode::provider_for(cli.provider)?; // anthropic wire (12) or mock (5)

    // 4. stream-json needs to see EVERY event as it happens → give the Session an event sink.
    let mut sink = output::sink_for(cli.output_format);   // stream-json → writes JSONL live
    let report = locode::Session::new(pack, provider, host)
        .max_turns(cli.max_turns)
        .run(&prompt, &mut sink)                          // engine emits Init/Message/Result
        .await;

    Outcome { format: cli.output_format, report, sink }
}
```

- **`ExitCode` return, not `std::process::exit`.** Codex calls `std::process::exit(1)`
  liberally (`lib.rs:1060`, and ~15 config-error sites), which skips destructor/flush. Because
  our stdout artifact is small and printed synchronously we prefer returning `ExitCode` from
  `main` so buffered stdout is guaranteed flushed before exit — a correctness nicety for the
  "exactly one JSON document" contract. clap parse failures still exit 2 via clap's own path.

### 3.3 output-format dispatch (`output.rs`) — the only stdout in the crate

```rust
/// The ONLY place the crate writes stdout. Everything else is `deny`'d.
mod stdout_sink {
    use locode_protocol::Event;

    #[allow(clippy::print_stdout)]                    // audited: the report/trace artifact
    pub fn writeln_json(value: &impl serde::Serialize) {
        // One line, one object (JSONL for stream-json; one object total for json).
        match serde_json::to_string(value) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("error: failed to serialize output: {e}"), // stderr, non-fatal
        }
    }
}
```

Mirrors Codex's jsonl `emit` exactly: `println!("{}", serde_json::to_string(&event)…)` inside a
narrow `#[allow(clippy::print_stdout)]` (`event_processor_with_jsonl_output.rs:103-115`),
serialize-error → an error object rather than a panic.

Three modes:

| `--output-format` | stdout | how |
|---|---|---|
| `json` (default) | one `Report` JSON object | sink buffers events silently; at end, `writeln_json(&report)`. This is the terminal `Event::Result.report` alone (ADR-0014:33). |
| `stream-json` | JSONL: `init` → `message`… → `result` | the sink writes each `Event` with `writeln_json` **as the engine emits it** (live trace). |
| `text` | the final assistant message only | at end, print `report.final_message.unwrap_or("")` to stdout (one `#[allow]`'d `println!`). Matches Codex's default human mode reducing to the last agent message. |

### 3.4 exit-code mapping (`output.rs`)

```rust
/// ADR-0009: exit 0 on any STRUCTURED terminal state; non-zero on fatal.
pub fn exit_code(o: Outcome) -> std::process::ExitCode {
    // emit stdout first (json/text) so the artifact exists regardless of status
    o.emit_final();
    match o.report.status {
        Status::Completed | Status::MaxTurns => std::process::ExitCode::SUCCESS, // 0
        Status::ModelError | Status::Error   => std::process::ExitCode::from(1),
    }
}
```

- **`completed` and `max_turns` are BOTH exit 0.** They are *structured* terminal states — the
  run produced a valid report, it just hit the ceiling. Only `model_error` (provider failed
  after retry) and `error` (a `Fatal` tool/host error) are non-zero. Verbatim from ADR-0009:14
  ("Exit `0` on any structured terminal state (`completed`/`max_turns`); non-zero on fatal") and
  the design streams table (`minimal-headless-rust-agent.md:294`). This is subtler than Codex's
  binary `error_seen → exit(1)` (`lib.rs:1059`); our `Status` enum (`locode-protocol/src/lib.rs:173`)
  already encodes the four states, so the mapping is total and testable.
- **clap/usage errors → exit 2** (clap default) — unknown `--harness`, bad `--max-turns`.
- **Config/setup failure before a run exists** (e.g. `--provider anthropic` with no API key):
  return exit 1 with an `error:`-prefixed stderr line, matching Codex's pre-run
  `eprintln! + exit(1)` pattern (each guarded `#[allow(clippy::print_stderr)]`, `lib.rs:304-307`).

### 3.5 stderr logging (`logging.rs`)

```rust
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// All human logs/traces go to STDERR. A stray stdout write fails the build (deny lint).
pub fn init() {
    let filter = EnvFilter::try_from_default_env()          // RUST_LOG / LOCODE_LOG
        .unwrap_or_else(|_| EnvFilter::new("warn"));
    let layer = fmt::layer()
        .with_writer(std::io::stderr)                        // <-- the load-bearing line
        .with_filter(filter);
    let _ = tracing_subscriber::registry().with(layer).try_init();
}
```

Direct port of Codex's setup: `fmt::layer().with_writer(std::io::stderr).with_filter(
exec_stderr_env_filter())` (`lib.rs:288-291`), with the default-filter fallback
(`lib.rs:232-237`). `with_writer(std::io::stderr)` is what keeps traces off the sacred stdout.

### 3.6 `build.rs` for `bundle-rg` (sketch, ported from grok's `xai-grok-tools/build.rs`)

```rust
// crates/locode-exec/build.rs — active only when the `bundle-rg` feature compiles it in.
// (Cargo always runs build.rs; we gate the WORK on the feature env + a path override,
//  exactly as grok gates on PROFILE=release OR GROK_TOOLS_BUNDLE_RG_PATH — build.rs:76-82.)
const RG_VER: &str = "14.1.1";                 // pin (grok pins 15.0.0, build.rs:10)

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=LOCODE_BUNDLE_RG_PATH");
    println!("cargo:rustc-check-cfg=cfg(bundle_rg)");        // lint-clean cfg (build.rs:70)
    // Only do work when the feature is on (CARGO_FEATURE_BUNDLE_RG set by cargo).
    if std::env::var_os("CARGO_FEATURE_BUNDLE_RG").is_none() { return Ok(()); }

    let out = PathBuf::from(std::env::var("OUT_DIR")?).join("bundle-rg");
    std::fs::create_dir_all(&out)?;

    // (a) OFFLINE / CI override: copy a local static rg. (grok build.rs:97-108)
    if let Some(src) = std::env::var_os("LOCODE_BUNDLE_RG_PATH") {
        let dest = out.join(format!("rg-{RG_VER}-override.bin"));
        std::fs::copy(&src, &dest)?;
        emit_cfg("override"); return Ok(());
    }
    // Windows ships .zip and we have no zip extractor → skip, fall back to PATH. (build.rs:84-92)
    if std::env::var("CARGO_CFG_TARGET_OS")? == "windows" { return Ok(()); }

    // (b) Download the pinned STATIC tarball for the target triple and extract `rg`.
    let triple = asset_triple()?;              // musl for linux (build.rs:116-121)
    let bytes = reqwest::blocking::get(rg_url(RG_VER, &triple))?.error_for_status()?.bytes()?;
    let rg = extract_rg_from_tar_gz(&bytes)?;  // flate2::GzDecoder + tar (build.rs:157-176)
    std::fs::write(out.join(format!("rg-{RG_VER}-{triple}.bin")), rg)?;
    emit_cfg(&triple); Ok(())
}

fn emit_cfg(target: &str) {
    println!("cargo:rustc-cfg=bundle_rg");                   // gates include_bytes! (build.rs:82)
    println!("cargo:rustc-env=LOCODE_RG_VER={RG_VER}");
    println!("cargo:rustc-env=LOCODE_RG_TARGET={target}");
}
```

Runtime side (`rg_bundle.rs`), ported from grok's `rg_path()`/`resolve_bundled_rg`
(`grep/ripgrep.rs:5-81`):

```rust
#[cfg(bundle_rg)]
const RG_BYTES: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"), "/bundle-rg/rg-", env!("LOCODE_RG_VER"), "-", env!("LOCODE_RG_TARGET"), ".bin"));

/// Self-extract once to the OS cache dir, chmod +x, return the path. Cached in a OnceLock.
pub fn extract() -> std::io::Result<PathBuf> {
    static RG: OnceLock<PathBuf> = OnceLock::new();
    // ... write RG_BYTES to $XDG_CACHE_HOME/locode/vendor/rg-<ver>-<triple> if absent,
    //     set_mode(0o755) on unix, atomic-rename for concurrency (ADR-0011:51). ...
}
```

- **Cache dir differs from grok.** Grok extracts to `~/.grok/vendor/` (`ripgrep.rs:19`); ADR-0011:51
  specifies `$XDG_CACHE_HOME`/`~/.cache/locode/vendor` (platform equivalent). Use the `dirs`/
  `directories` crate or hand-roll from `$XDG_CACHE_HOME`.
- **Atomic-rename on extract.** Write to a temp file then `rename` into place, so two concurrent
  `locode-exec` invocations don't race a half-written binary (ADR-0011:51 "atomic-rename for
  concurrency"). Grok's version just checks `!exists` then writes (`ripgrep.rs:25-34`) — a mild
  race we improve on.
- **The extracted path is INJECTED into `locode-host`'s resolver, not stomped onto
  `LOCODE_RG_PATH`.** ADR-0011:41-44 defines the resolver order as (a) `LOCODE_RG_PATH` env
  [user/test override], (b) host-provided bundled path, (c) bare `rg` on PATH. `locode-exec`
  supplies tier (b) via `HostBuilder::bundled_rg(path)` so it never clobbers a user's explicit
  (a). This keeps *how rg got on disk* out of the core (ADR-0011:21 layering) — the host only
  knows "here is a bundled path to try".

---

## 4. Behavior / algorithms + edge cases

- **Prompt from stdin.** If `--prompt` is absent or `-`, read stdin to string (Codex:
  `cli.rs:81-85`). Empty prompt after stdin → exit 2 with a usage error.
- **`--provider mock` needs no key.** For CI, `Provider::Mock` builds a `MockProvider`
  (Task 5, `locode-provider`) whose scripted turns end in a final text message. Acceptance
  criterion: `--provider mock` "runs in CI without a key" (`todo.md:256`, `SPEC.md:115`). The
  mock's script for the CI test is a single no-tool text turn → `Status::Completed`, one Report.
- **`--provider anthropic` with no key** → fail *before* driving the loop: check
  `LOCODE_API_KEY`/`ANTHROPIC_API_KEY` (ADR-0007:13), emit `error:` on stderr, exit 1. Do not
  emit a partial report.
- **stream-json ordering.** `init` must be first and carry `preamble` (System+Developer) +
  `tools` specs so the trace self-reconstructs (ADR-0014:28); `result` is last and equals the
  `json`-mode Report. The sink writes live; the engine (Task 6) is the emitter — `locode-exec`
  only serializes what the sink receives. Verify with `reconstruct_conversation`
  (`locode-protocol/src/lib.rs:271`).
- **json mode still consumes events.** The engine emits events regardless of mode; in `json`
  mode the sink discards intermediate events and keeps only the terminal `Report`. So the same
  engine drive powers all three modes — no second loop (Boundaries "Never … second, throwaway
  loop", `SPEC.md:125`).
- **A serialize failure on the final Report** is near-impossible (all fields are plain), but if
  it happens: emit an `{"type":"error",…}` object to stdout and exit 1 (Codex's fallback,
  `jsonl_output.rs:106-113`). This is the *one* case stdout carries a non-Report object.
- **`bundle-rg` with empty PATH.** The acceptance test builds `--features bundle-rg --release`
  and runs with `PATH=""`; Grep/Glob must still resolve `rg` from the self-extracted copy
  (`todo.md:257`). The resolver tier (b) covers this; tier (c) `rg` on PATH is unavailable.
- **`bundle-rg` build offline.** Without network, set `LOCODE_BUNDLE_RG_PATH=/path/to/rg` and
  the build.rs copies it instead of downloading (grok's offline hatch, `build.rs:76`,
  message at `build.rs:135`). CI uses this to stay hermetic.

---

## 5. Design decisions (each: harness `file:line`, why, why-not, differences)

1. **`#![deny(clippy::print_stdout)]` at the `locode-exec` crate root; one audited `#[allow]`.**
   - Codex: `#![deny(clippy::print_stdout)]` (`exec/src/lib.rs:5`) with narrow `#[allow]` only on
     the two real stdout emitters (`jsonl_output.rs:103`, `human_output.rs:391`).
   - Why: makes "stdout is sacred" a *structural* guarantee — a stray `println!` fails the build
     (ADR-0009:13, SPEC "Never … `println!` from non-report paths", `SPEC.md:125`). The scaffold
     already has the lint (`locode-exec/src/main.rs:5`).
   - Why not: rely on code review — Codex explicitly chose the compiler over discipline; so do we.
   - Difference: library crates never print at all (they emit via the event sink / return values),
     so only `locode-exec` needs the lint. We centralize the single `#[allow]` in `output.rs`.

2. **Three-way `--output-format {json,text,stream-json}` (Claude shape), not Codex's `--json`.**
   - Codex: boolean `--json` (JSONL events) vs default human text (`cli.rs:63-70`).
   - Claude/ADR-0014: `--output-format stream-json` is the model ADR-0014 was written against
     (`ADR-0014:14`); `json` = the `result` Report alone, `stream-json` = full event stream,
     `text` = final message (`ADR-0014:62-64`, `todo.md:252`).
   - Why: we have three genuinely distinct artifacts; a boolean can't express "summary vs trace".
   - Why not Codex's boolean: it conflates final-text and events and has no summary-envelope mode.
   - Difference: our `json` is a *summary* (not the trace); the trace is `stream-json`'s job
     (ADR-0014:50 defers transcript-in-json).

3. **Exit 0 for both `completed` and `max_turns`; 1 for `model_error`/`error`; 2 for usage.**
   - Codex: single `error_seen` boolean → `exit(1)` (`lib.rs:1059-1061`).
   - Why: ADR-0009:14 + design streams table (`minimal-headless-rust-agent.md:294`) — `max_turns`
     is a *structured* outcome, not a failure. Our `Status` enum makes the 4-way mapping total.
   - Why not Codex's binary flag: too coarse — a caller can't distinguish "hit the ceiling" (retry
     with more turns) from "provider died" (retry later) from exit code alone.
   - Difference: richer, `Status`-driven mapping; `ExitCode` return (flush-safe) over `process::exit`.

4. **`tracing_subscriber` fmt layer → `std::io::stderr`, `EnvFilter` from `RUST_LOG`.**
   - Codex: `fmt::layer().with_ansi(...).with_writer(std::io::stderr).with_filter(env_filter)`
     (`lib.rs:288-291`); default-filter fallback (`lib.rs:232-237`).
   - Why: all diagnostics on stderr (ADR-0009), structured/filterable logs for a headless tool.
   - Why not `env_logger`/`log`: `tracing` is the ecosystem default for async and what Codex/Grok
     use; spans help when the loop grows.
   - Difference: we skip Codex's color/ANSI plumbing (`--color`, `lib.rs:280-287`) in v0 — a plain
     stderr layer; ANSI is a later nicety.

5. **`bundle-rg` as a cargo feature + build.rs download/copy + `include_bytes!` + runtime
   self-extract; resolver stays in `locode-host`.**
   - Grok: `build.rs` gated on release-or-override, downloads the static musl/darwin tarball,
     extracts `rg`, emits `cfg(bundle_rg)` (`xai-grok-tools/build.rs:66-185`); runtime
     `include_bytes!` + `resolve_bundled_rg` self-extract with `chmod 0o755`, `OnceLock`-cached,
     PATH fallback (`grep/ripgrep.rs:5-81`).
   - Why: guarantees `rg` availability in a shipped single-file binary (ADR-0011:1,3); keeps *how
     rg lands on disk* a packaging concern outside the core (ADR-0011:21). Feature-gated so the
     default dev build is fast (no download) — mirrors grok gating on `PROFILE=release`.
   - Why not: **sidecar next to the exe** (Claude Code, `vendor/ripgrep/<arch>/rg`, ADR-0011:64) —
     better for a notarized macOS *app*, but embed-first suits a single-file CLI; the host resolver
     abstracts both so it stays a packaging choice (ADR-0011:66). **Hand-rolled walker** — rejected
     by ADR-0011:1 (divergent gitignore/semantics).
   - Difference from grok: our override env is `LOCODE_BUNDLE_RG_PATH`; extract dir is XDG cache
     not `~/.grok`; we atomic-rename (grok checks-then-writes); and the resolver lives in
     `locode-host` (ADR-0011 tier b injection) rather than inside the grep tool — so the tool stays
     host-agnostic (ADR-0008).

6. **Facade = curated `pub use` of the driving API only (SPEC Open Q5).**
   - Design: `locode` "re-exports the driving API (`Session`, harness/provider selection, report
     types)" (`todo.md:251`); `locode-exec → locode` only (`SPEC.md:83`).
   - Why: the facade is the *stable* surface `locode-app` will build on; keeping extension internals
     (the `Tool`/`Provider` traits, `locode-host` guts, wire types) out of the default surface lets
     them churn without breaking the app. Re-export `Session`/engine entry, `Report`/`Status`/
     `Event`/`Usage`/`ToolCallRecord` from `locode-protocol`, the `Harness`/`Provider` selectors,
     and a top-level error type.
   - Why not re-export everything (`pub use locode_tools::*` etc.): leaks unstable internals and
     makes the facade meaningless. If `locode-app` needs to *author* tools/providers later, expose
     them behind an explicit `locode::extend` module (post-v0), not the flat surface.
   - Difference: this is our call (no single harness dictates it); Open Q5 (`SPEC.md:145`) — start
     narrow, widen on demand.

---

## 6. Tests (Task 14 acceptance)

1. **`mock_provider_emits_one_parseable_json_report` (CI, keyless).** Integration test under
   `crates/locode-exec/tests/`: run the binary (via `assert_cmd` or an in-process `run()` entry)
   with `--provider mock --output-format json --prompt "hi"`; assert **stdout is exactly one
   line** that `serde_json::from_str::<Report>()` parses, with `status == "completed"`,
   `harness=="grok"`, `provider=="mock"`. This is Success Criterion #5 (`SPEC.md:133`) and the
   Checkpoint-D shape. (Codex analog: `main_tests.rs`.)
2. **`json_stdout_is_single_document`.** Assert stdout has no second line / no log lines
   interleaved — the single-document contract (ADR-0009:22 "Interleave events on stdout: Rejected").
3. **`stream_json_is_valid_jsonl_and_reconstructs`.** `--output-format stream-json --provider
   mock`; assert each stdout line parses as `Event`, first is `init` (with non-empty `preamble` +
   `tools`), last is `result`, and `reconstruct_conversation(&events)` yields the full history
   incl. System/Developer (ADR-0014, using `locode-protocol/src/lib.rs:271`).
4. **`text_mode_prints_final_message_only`.** `--output-format text`; stdout == the report's
   `final_message` and nothing else.
5. **`logs_go_to_stderr_not_stdout`.** With `RUST_LOG=debug`, assert stdout stays a clean single
   JSON doc and the log lines appear on stderr. (Enforces ADR-0009 streams split.)
6. **`exit_codes_map_status`.** Table test: mock scripts producing `completed`→0, `max_turns`→0,
   `model_error`→1, `error`→1; a clap usage error →2. (Drives the §3.4 mapping.)
7. **`unknown_harness_is_clean_error`.** `--harness bogus` → non-zero (clap exit 2), stderr names
   the valid values; nothing on stdout. (`todo.md:162`.)
8. **`bundle_rg_resolves_with_empty_PATH` (feature-gated, release).** `cargo build -p locode-exec
   --features bundle-rg --release`; run a grep-driving task with `PATH=""`; assert `rg` resolves
   from the self-extracted copy and the search returns results (`todo.md:257`). CI provides
   `LOCODE_BUNDLE_RG_PATH` to stay offline/hermetic.

---

## 7. Deps to add (with justification + precedent)

| Dep | Crate / section | Justification | Precedent |
|---|---|---|---|
| `clap` v4 (derive) | `locode-exec` runtime | SPEC's chosen CLI parser ("CLI (locode-exec only): `clap`", `SPEC.md:38`); derive gives the args struct + `ValueEnum` + `--help` + exit-2 usage errors. | Codex-exec uses clap derive (`exec/src/cli.rs:1-14`). |
| `tracing` + `tracing-subscriber` (feat `env-filter`, `fmt`) | `locode-exec` runtime | stderr structured logging + `RUST_LOG` filtering (ADR-0009). | Codex: `tracing_subscriber::fmt::layer().with_writer(stderr)` (`lib.rs:288`, `156-157`). |
| `tokio` (feat `rt-multi-thread`, `macros`) | `locode-exec` runtime | the engine/provider are async (SPEC tech stack, `SPEC.md:18`); the binary owns the runtime. | Already a workspace dep (`Cargo.toml:23`); Codex/grok both tokio. |
| `reqwest` (`blocking`, `rustls`), `flate2`, `tar` | `locode-exec` **`[build-dependencies]`**, gated behind `bundle-rg` | download + un-gzip + un-tar the static `rg` in build.rs. Build-only → not in the runtime dep graph. | Grok's build.rs uses exactly `reqwest::blocking` + `flate2::GzDecoder` + `tar::Archive` (`build.rs:137,155-157`). |
| `directories` or `dirs` v5 | `locode-exec` runtime, gated behind `bundle-rg` | resolve `$XDG_CACHE_HOME`/platform cache dir for the self-extract target (ADR-0011:51). | Common; grok hand-rolls `grok_home()` — `directories` is the portable equivalent. |
| `assert_cmd` + `predicates` (dev) | `locode-exec` `[dev-dependencies]` | drive the built binary and assert stdout/stderr/exit in the integration tests. | Standard for CLI testing; Codex tests the binary end-to-end. |

- **AGENTS.md "Ask first: adding a dependency"** — `clap` and `tokio` are pre-blessed by SPEC;
  `tracing*`, `reqwest`/`flate2`/`tar` (build-only, feature-gated), `directories`, and the dev
  `assert_cmd` are new — enumerate them explicitly in the PR for sign-off. The build/rg deps only
  compile under `--features bundle-rg`, so the default build's dep graph stays lean.

---

## 8. Open questions

1. **Facade breadth (SPEC Open Q5, `SPEC.md:145`).** Confirm the narrow surface in §5.6: does
   `locode-app` need to *author* custom tools/providers in the near term (→ expose `Tool`/
   `Provider` behind `locode::extend`), or is driving-only enough for v0? Recommend narrow now.
2. **`ExitCode` return vs `std::process::exit`.** §3.2 prefers `ExitCode` (flush-safe). Confirm no
   code path needs an immediate hard exit before stdout is written (Codex uses `process::exit`
   pervasively). Recommend `ExitCode`.
3. **stdin prompt semantics.** Match Codex exactly — if stdin is piped *and* `--prompt` given,
   append stdin as a `<stdin>` block (`cli.rs:82-85`)? Or v0-simple: `--prompt` XOR stdin.
   Recommend the simple XOR for v0.
4. **Where does the Session take the event sink?** Assumes Task 6 exposes
   `Session::run(&prompt, &mut sink)` with a sink trait the binary implements. Confirm the exact
   engine API (sink trait shape, sync vs async emit) so `output.rs` matches.
5. **rg version pin + platform matrix.** Pin `rg` (sketch uses 14.1.1; grok uses 15.0.0) and
   confirm the target-triple table (musl for linux, darwin for macOS). Multi-platform bundle
   matrix + macOS notarization are explicitly deferred (`tasks/todo.md:305`, ADR-0011:74).
6. **Cache dir + concurrency.** Confirm `$XDG_CACHE_HOME/locode/vendor` (ADR-0011:51) and the
   atomic-rename extract; decide `directories` vs hand-rolled.

---

## Addendum (2026-07-18): prompt is a positional argument, not `--prompt`

User decision, superseding every `--prompt` mention above. Rationale: the field
convention is a *mode* flag plus a positional prompt — Claude Code's `-p/--print`
enters headless mode and reads the prompt from the first non-flag argument (or
stdin); `codex exec "…"` is a subcommand plus positional. `locode-exec` is
**always** headless, so it needs no mode flag at all:

```sh
locode-exec "summarize this repo" [--harness grok] [--api-schema anthropic] …
```

- Prompt = the single positional argument. Absent or `-` → read stdin to string
  (Codex's convention). The §8 stdin open question resolves to **v0-simple:
  positional XOR stdin** (no `<stdin>`-append hybrid).
- All other flags stay as specified, and should hew to the studied harnesses'
  conventions (e.g. `--output-format json|text|stream-json` mirrors Claude
  Code's `--output-format`, `--dangerously-skip-permissions` is Claude Code's
  own flag name).
