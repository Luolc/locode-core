# locode-core

The headless Rust core of **locode**, a custom coding agent: the
sample→dispatch→append loop, a typed tool registry, faithful per-harness tool
packs, and a provider/wire abstraction — shipped as a set of library crates
plus a minimal headless binary (`locode-exec`). No TUI lives here; that
belongs to a separate app built on these crates.

## Why

locode-core is a distilled re-implementation of the agent cores of four
studied coding harnesses (Claude Code, Codex, Grok Build, opencode). Each
harness pack reproduces its harness's real tools and system prompt —
names, schemas, caps, quirks — so different harness designs can be compared
honestly, A/B, on the same engine and the same provider wire. The tool packs
are also a first-class library surface: downstream consumers can drop the
tools into their own agent loop without using our engine.

## Quick start

```sh
export LOCODE_API_KEY=…      # required (except with --api-schema mock)
export LOCODE_BASE_URL=…     # optional: provider endpoint override
export LOCODE_MODEL=…        # optional: model id override

# One task, headless: one JSON report on stdout, diagnostics on stderr.
cargo run -p locode-exec -- "summarize this repo" --harness grok
```

Select the provider wire with `--api-schema` (`anthropic` | `openai-responses`
| `mock`, where `mock` runs keyless) and the stdout artifact with
`--output-format` (`json` | `stream-json` | `text`). See
`cargo run -p locode-exec -- --help` for the full surface.

`LOCODE_BASE_URL` defaults to the wire's native endpoint
(`https://api.anthropic.com` for `anthropic`, `https://api.openai.com` for
`openai-responses`); point it at any compatible gateway (e.g. OpenRouter) to
reach other providers over the same schema. `LOCODE_MODEL` defaults to
`claude-sonnet-5` on the Anthropic wire and `gpt-5-mini` on the OpenAI
Responses wire, and `LOCODE_API_KEY` must match whichever endpoint you target.

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
| `locode-exec` | Minimal headless binary — exactly one JSON report on stdout |
