# ADR-0015: Custom providers — `ProviderRegistry` + library-entry `locode-exec`

## Status
Accepted

## Date
2026-07-20

## Context
Downstream consumers need to run the CLI (and the future `locode-app` TUI) against
backends we don't ship — custom wires with request/response handling unlike any
built-in schema, implemented in their own codebases. The `Provider` trait
(ADR-0007) already abstracts the wire, but there is no way to get a *custom*
implementation into the binary: `--api-schema` is a closed clap `ValueEnum` and
provider construction is hard-coded in `locode-exec`'s `run.rs`.

How the studied harnesses handle out-of-tree providers:

- **Codex** — *config-defined* providers: `model_providers` entries in
  `config.toml` (name, `base_url`, `env_key`, `wire_api = responses|chat`;
  `codex-rs/core/src/model_provider_info.rs`). Powerful for gateways that speak an
  existing schema, but cannot express a novel wire — the schema set is closed.
- **Claude Code** — env-based gateway switching over a fixed schema set
  (`ANTHROPIC_BASE_URL`, Bedrock/Vertex modes) plus `apiKeyHelper` (a user command
  minting auth per request). Same limitation: endpoints/auth vary, wires don't.
- **Grok Build** — the unification model (one abstraction, N registered
  implementations) that this repo already follows for packs; its provider set is
  compiled in, not extensible.

We already cover the codex/claude case (existing schema, custom endpoint) via
`LOCODE_BASE_URL`/`LOCODE_API_KEY`. The gap is genuinely custom wires.

## Decision
1. **`ProviderRegistry` in the `locode-core` facade** — an ordered name → factory
   map. `ProviderRegistry::builtin()` carries the built-ins (`anthropic`,
   `openai-responses`, `mock`); `register(name, factory)` adds or replaces an
   entry. A factory receives a `ProviderInit` (session id, for cache-key routing)
   and returns a `BuiltProvider` (`Arc<dyn Provider>` + resolved model name). The
   registry lives in the facade — not in `locode-exec` — so the future
   `locode-app` gets the same injection seam.
2. **`locode-exec` becomes a library with a trivial binary.** The CLI's substance
   (clap surface, session assembly, output discipline, exit codes) moves to the
   crate's lib target behind `main_with(registry) -> ExitCode`; the shipped
   binary is `main_with(ProviderRegistry::builtin())`. A downstream binary crate
   is ~5 lines: builtin registry + `.register(...)` — it tracks new CLI features
   by version bump, with no copied code to drift.
3. **`--api-schema` validates against the registry, not a closed enum.** Unknown
   names fail pre-run (exit 1) with the available names listed; registered names
   are selectable without touching this repo.
4. **Trait stability posture:** downstream `Provider` impls make the trait a real
   public contract. Signature changes remain "ask first" (AGENTS.md boundary);
   new capabilities should arrive as provided-default methods where possible.

## Alternatives Considered
### Template binary crate (copy `main.rs`, add your provider)
- Pros: zero new API surface.
- Rejected: template drift — every new flag/feature must be manually mirrored;
  divergence is silent. The library entry point keeps one copy of the logic.

### Config-defined providers (codex's `model_providers`)
- Pros: no code for the gateway case; runtime-selectable.
- Rejected *as the primary mechanism*: cannot express a custom wire (the reason
  this ADR exists). Remains a natural later layer **on top of** the registry for
  existing-schema gateways.

### Dynamic loading (dlopen plugins) or a sidecar provider process
- Rejected: heavy machinery none of the studied harnesses needed; ABI/protocol
  surface to maintain; the downstream-binary pattern achieves the same with plain
  Cargo.

## Consequences
- Downstream binaries: `ProviderRegistry::builtin().register("my-wire", …)` +
  `locode_exec::main_with(registry)`; `--api-schema my-wire` selects it.
- The built-in provider construction (env resolution, mock scripting) moves from
  `locode-exec/src/run.rs` into the facade's registry module — `run.rs` no longer
  hard-codes any provider.
- `--api-schema`'s clap `ValueEnum` help listing is replaced by registry-driven
  validation; help text names the built-ins and points at the registry.
- README gains a "Custom providers" section (general downstream framing).
