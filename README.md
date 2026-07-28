# LoCode

A Rust-based custom coding agent: the sample → dispatch → append loop, a typed
tool registry, faithful per-harness tool packs, and a provider/wire abstraction
— shipped as a set of library crates plus the `locode` binary (run
`locode -p "say hi"` for a headless rollout, or `locode` without `-p` for an
experimental interactive TUI).

## Why

LoCode is a distilled re-implementation of the agent cores of four studied
coding harnesses (Claude Code, Codex, Grok Build, opencode). Each harness pack
reproduces its harness's real tools and system prompt — names, schemas, caps,
quirks — so different harness designs can be compared honestly, A/B, on the same
engine and the same provider wire. The tool packs are also a first-class library
surface: downstream consumers can drop the tools into their own agent loop
without using our engine.

The reproduction is deliberately scoped. What each pack faithfully mirrors is
its harness's **system prompt** and its **tool behavior** — and the tool surface
is the six core tools every coding agent shares (shell, read, write, edit, list,
search). Everything above that line — memory files (`AGENTS.md` and friends),
skills, and the fancier engine behaviors — is implemented once and shared across
all packs rather than reproduced per harness, so the fancier per-harness
features are not modeled. That core is enough to run most coding-benchmark
evaluations and everyday tasks, but it is not exhaustive: LoCode is built for
evaluation and research, not as an end-user product.

## Install

Prebuilt `locode` binaries for macOS and Linux (one command to install,
re-run it to update):

```sh
curl -fsSL https://raw.githubusercontent.com/luolc/locode-core/main/install.sh | bash
```

The script installs to `~/.locode/bin` (override with `LOCODE_BIN_DIR`),
verifies the sha256 checksum, and puts the binary on your PATH. Pass a version
for a specific release: `… | bash -s 0.1.17`. Then run `locode` for the
interactive agent or `locode -p "task"` for a headless one-shot. The search
tools shell out to [ripgrep](https://github.com/BurntSushi/ripgrep) — have `rg`
on PATH or point `LOCODE_RG_PATH` at a binary.

Or build from source: `cargo install --git https://github.com/Luolc/locode-core locode-app`.

## Quick start

```sh
export LOCODE_API_KEY=…      # required (except with --api-schema mock)
export LOCODE_BASE_URL=…     # optional: provider endpoint override

# One task, headless: one JSON report on stdout, diagnostics on stderr.
locode -p "summarize this repo"
# …or from a source checkout, without installing:
cargo run -p locode-app -- -p "summarize this repo"
```

Select the harness pack with `--harness` (`claude` | `codex` | `grok` — default
`claude`), the provider wire with `--api-schema` (`anthropic` | `openai-responses`
| `mock`, where `mock` runs keyless), the model with `--model`, and the stdout
artifact with `--output-format` (`json` | `stream-json` | `text`). See
`locode --help` for the full surface.

`LOCODE_BASE_URL` defaults to the wire's native endpoint
(`https://api.anthropic.com` for `anthropic`, `https://api.openai.com` for
`openai-responses`); point it at any compatible gateway (e.g. OpenRouter) to
reach other providers over the same schema. `LOCODE_API_KEY` must match
whichever endpoint you target. (The model is chosen with `--model` or in
settings — there is no model env var.)

## Configuration & sessions (`~/.locode`)

Durable settings live in `~/.locode/settings.json` (scaffolded with defaults on
first run; override the home with `LOCODE_HOME`). They layer **user →
`extends` → project `.locode/settings.json` → `.locode/settings.local.json` →
`--settings` flag**, and an explicit flag always wins. Keys include `harness`,
`api_schema`, `model`, `instructions.root_stop_pattern`, and `skills.extra`.

`extends` points at **another locode folder**, not at a settings file — a shared
team config you want to inherit. All three of its parts merge: its
`settings.json` (at the layer shown above), its `skills/`, and its `AGENTS.md`
(ranked below your own global one, so yours still wins).

## Skills

A skill is a folder holding a `SKILL.md`: YAML frontmatter with `name` and
`description`, then instructions in markdown. Put them in
`<repo>/.agents/skills/` to share with the project (the same folder other agents
read), or `~/.locode/skills/` for your own.

Every run advertises the skills it found — name, description, and file path —
and the agent opens the file when a task matches. `when-to-use` adds a trigger
hint; `disable-model-invocation: true` hides a skill from the agent. Writing or
editing a skill takes effect on the next turn, with no restart.

You can also run a skill yourself: type `/` in the composer and pick it, or type
`/<name>` and anything you want to pass along. `user-invocable: false` keeps a
skill out of that menu.

## Commands

Typing `/` at the start of the composer opens a menu of what you can run.
Keep typing to narrow it, ↑/↓ to move, Tab to complete, Enter to run, Esc to
close. `/help` lists everything, `/model` shows or switches the model, `/new` starts
a fresh session, and `/quit` (or `/exit`) leaves. `/model <id>` changes the running
session and saves the id as the default for the next one; the menu lists a few common
ids, and any other id can be typed. Every skill you can invoke
appears there too.

Every run appends a JSONL trace under `~/.locode/sessions/`; `--continue`
resumes the newest session for the current directory and `--resume <id>` resumes
by id (both in headless and interactive modes). `--no-session-persistence`
skips writing the trace for a run.

## Interactive app (`locode`)

LoCode is primarily a headless engine for evaluation and data pipelines. It
also ships an **experimental** interactive terminal UI, built in this repo as
separate crates (`locode-tui` = components/library, `locode-app` = the `locode`
binary) layered on the core (ADR-0019). It drives one session interactively:
type a prompt, watch tool calls and the reply stream in, approve or deny tool
calls, cancel a turn with Esc, and continue the conversation. You can also type
a follow-up while a turn is still running — it is delivered as soon as the
running tool call finishes, rather than waiting for the whole turn. Treat it as
a convenience for exploring the engine, not a supported product.

```sh
# Keyless demo (scripted mock wire — no API key):
locode --api-schema mock

# Against a real wire:
LOCODE_API_KEY=… locode
```

Tool calls run **without approval prompts**, and file access is not limited to the
working directory. `--restricted` turns both on, but it is incomplete: there is no
way to record an answer yet, so the same call asks again every time. That is why it
is not the default — the finished permission rules are still being built.

## Development

To build and test everything:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

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
