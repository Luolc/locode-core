# locode-core

The headless Rust core of **locode**, a custom coding agent: the
sample→dispatch→append loop, a typed tool registry, faithful per-harness tool
packs, and a provider/wire abstraction — shipped as a set of library crates
plus the `locode` binary (run it for the interactive TUI, or `locode -p "task"`
for a headless one-shot).

## Why

locode-core is a distilled re-implementation of the agent cores of four
studied coding harnesses (Claude Code, Codex, Grok Build, opencode). Each
harness pack reproduces its harness's real tools and system prompt —
names, schemas, caps, quirks — so different harness designs can be compared
honestly, A/B, on the same engine and the same provider wire. The tool packs
are also a first-class library surface: downstream consumers can drop the
tools into their own agent loop without using our engine.

## Install

Prebuilt `locode` binaries for macOS and Linux (one command to install,
re-run it to update):

```sh
curl -fsSL https://raw.githubusercontent.com/luolc/locode-core/main/install.sh | bash
```

The script installs to `~/.locode/bin` (override with `LOCODE_BIN_DIR`),
verifies the sha256 checksum, and puts the binary on your PATH. Pass a version
for a specific release: `… | bash -s 0.1.5`. Then run `locode` for the
interactive agent or `locode -p "task"` for a headless one-shot. The search
tools shell out to [ripgrep](https://github.com/BurntSushi/ripgrep) — have `rg`
on PATH or point `LOCODE_RG_PATH` at a binary.

Or build from source: `cargo install --git https://github.com/Luolc/locode-core locode-app`.

## Quick start

```sh
export LOCODE_API_KEY=…      # required (except with --api-schema mock)
export LOCODE_BASE_URL=…     # optional: provider endpoint override
export LOCODE_MODEL=…        # optional: model id override

# One task, headless: one JSON report on stdout, diagnostics on stderr.
locode -p "summarize this repo" --harness grok
# …or from a source checkout, without installing:
cargo run -p locode-app -- -p "summarize this repo" --harness grok
```

Select the provider wire with `--api-schema` (`anthropic` | `openai-responses`
| `mock`, where `mock` runs keyless) and the stdout artifact with
`--output-format` (`json` | `stream-json` | `text`). See `locode --help` for the
full surface.

`LOCODE_BASE_URL` defaults to the wire's native endpoint
(`https://api.anthropic.com` for `anthropic`, `https://api.openai.com` for
`openai-responses`); point it at any compatible gateway (e.g. OpenRouter) to
reach other providers over the same schema. `LOCODE_MODEL` defaults to
`claude-sonnet-5` on the Anthropic wire and `gpt-5-mini` on the OpenAI
Responses wire, and `LOCODE_API_KEY` must match whichever endpoint you target.

## Interactive app (`locode`)

An interactive terminal UI is built in this repo as separate crates
(`locode-tui` = components/library, `locode-app` = the `locode` binary) layered
on the core (ADR-0019). It drives one session interactively: type a prompt,
watch tool calls and the reply stream in, approve or deny tool calls, cancel a
turn with Esc, continue the conversation.

```sh
# Keyless demo (scripted mock wire — no API key):
cargo run -p locode-app -- --api-schema mock

# Against a real wire, with tool approvals prompted:
LOCODE_API_KEY=… cargo run -p locode-app

# …or skip approvals (auto-allow every tool, lift the path jail):
LOCODE_API_KEY=… cargo run -p locode-app -- --yolo
```

Keys: Enter sends (Alt+Enter for a newline); Esc cancels a running turn, or
pops a queued prompt / clears a draft when idle; Ctrl+C cancels then quits;
Up/Down browse prompt history; `/new` starts a fresh session, `/quit` exits.
`--harness`, `--api-schema`, and `--cwd` match the headless `-p` flags. The app
crates are `publish = false`; the binary ships from the GitHub Release and
`install.sh` installs it as `locode`.

To build and test everything:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Crates

| Crate | What it is |
| --- | --- |
| `locode-protocol` | Conversation model, tool call/result types, report envelope — pure types, no I/O |
| `locode-tools` | `Tool` trait, registry, and the one dispatch door — host-agnostic framework |
| `locode-packs` | Harness packs: faithful per-harness toolsets + system prompts |
| `locode-provider` | `Provider` trait, API-agnostic request type, and the wire implementations |
| `locode-host` | Filesystem/shell/path-jail seam — every side effect goes through here |
| `locode-engine` | The sample→dispatch→append loop and the `Session` driving API |
| `locode-core` | Facade: re-exports the driving API and the full tool surface |
| `locode-exec` | Headless engine library (`run_headless`, one JSON report) — reused by `locode -p`; the standalone binary is retired |

## Custom providers

The provider surface is open: implement the `Provider` trait for any backend,
register it under a name, and your own binary crate gets the entire CLI —
flags, report envelope, exit codes — with your provider selectable at run time:

```rust
use locode_exec::{ProviderRegistry, main_with};

fn main() -> std::process::ExitCode {
    let registry = ProviderRegistry::builtin()
        .register("my-wire", |init| my_wire::build(init));
    main_with(registry)
}
```

`--api-schema my-wire` then selects it; unknown names fail before the run
starts, listing what's registered.

## [Disclaimer](DISCLAIMER.md)

This is a personal project and is not affiliated with any company. The content
does not reflect any specific company's projects, products or internal work.
