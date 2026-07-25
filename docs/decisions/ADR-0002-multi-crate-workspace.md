# ADR-0002: Multi-crate `locode-*` Cargo workspace

## Status
Accepted

## Date
2026-07-17

## Context
The cleanest studied Rust trees (Codex, Grok Build) separate a **portable tools layer** from the **session/loop layer**, and further separate *tool definition* from *dialect selection* and *wire protocol*. Crate boundaries make those separations enforceable by the compiler rather than by convention. The design-doc placeholder names were `agent-*`; the real project uses `locode-*`.

## Decision
Use a Cargo workspace of small `locode-*` crates. `locode-core` names the **workspace/repo**
and — since the 2026-07-18 rename below — the facade crate as well.

> **Note (renamed facade, 2026-07-18):** the facade crate `locode` was renamed
> **`locode-core`** at the first crates.io release: the bare name `locode` was already
> owned on the registry by an unrelated UN/LOCODE (country/city codes) crate. The crate
> boundary and re-export surface are unchanged; lib imports are now `locode_core::…`.
> The planned `locode` *harness pack* (a pack name, not a crate) is unaffected.

> **Note (superseded naming):** `locode-dialects` was renamed **`locode-packs`** by
> [ADR-0012](ADR-0012-harness-packs.md) — harness packs (faithful per-harness toolsets)
> supersede the dialect-packs/`EditEncoding` model of the superseded ADR-0006. The crate
> boundary this ADR establishes is unchanged; only the crate's name and contents did.

| Crate | Role |
|---|---|
| `locode-protocol` | conversation model (4-role, ADR-0013), tool call/result, report envelope (pure types, no I/O) |
| `locode-tools` | canonical `Tool` trait + registry + dispatch (host-agnostic framework) |
| `locode-packs` (was `locode-dialects`) | harness packs — faithful per-harness toolsets over `locode-tools` (ADR-0012) |
| `locode-provider` | `Provider` trait + API-agnostic `ConversationRequest` + Anthropic wire impl |
| `locode-host` | fs/shell/path-jail/truncation/rg-resolution (injectable side-effect seam) |
| `locode-instructions` | project instructions (`AGENTS.md`): discovery, assembly, and the injected `<system-reminder>` message (added 2026-07-24) |
| `locode-skills` | agent skills (`SKILL.md`): discovery and the model-facing listing (added 2026-07-24) |
| `locode-commands` | slash commands: the `SlashCommand` trait, `CommandResult`, and the registry (added 2026-07-25, ADR-0026) |
| `locode-engine` | sample→dispatch→append loop + `Session` driving API |
| `locode-core` (was `locode`) | thin facade re-exporting the public surface |
| `locode-exec` | headless runner **library** (`run_headless` + `main_with`); binary target removed 2026-07-23 — kept standalone (not collapsed into `locode-tui`) so headless-only consumers avoid the TUI dep (ADR-0019 amendment) |
| `locode-tui` | TUI components + interactive app + `-p` headless dispatch (ADR-0019; added 2026-07-21) |
| `locode-app` | flag-free product binary — the shipped `locode` (ADR-0019; added 2026-07-21) |

## Alternatives Considered
### Single crate with modules
- Pros: fewer `Cargo.toml` files; simplest start.
- Rejected: boundaries become conventions only; the portable-tools vs orchestration split (the strongest structural lesson from the survey) erodes as the code grows.

### Fewer, coarser crates (3–4)
- Pros: middle ground.
- Rejected: softens exactly the boundaries we most want hard (host seam, provider wire, tools vs loop). We can always merge later; splitting later is harder.

## Consequences
- Dependency direction is explicit and acyclic: `protocol` is the shared base; `engine` composes `tools`/`packs`/`provider`/`host`; `locode-core` re-exports; `locode-exec` depends only on `locode-core`.
- Tools **never** touch `std::fs`/`Command` directly — only through `locode-host` — making them trivially testable and sandbox-ready.
- More Cargo manifests and a `[workspace.lints]` table to maintain; accepted cost.

## Amendment (2026-07-24): one crate per injected-context feature — `locode-instructions` and `locode-skills`

Project-instruction (`AGENTS.md`) loading was split across two crates — the loader in
`locode-host`, the renderer in `locode-engine` — and skills (ADR-0025) were about to
add a second copy of the same shape. Both now get **their own crate** *(user
decision)*:

- **`locode-instructions`** (this change): discovery, assembly, and the `User`
  `<system-reminder>` message. Depends on `locode-host` (for the shared cwd→root
  marker walk) and `locode-protocol` (for `Message`); nothing depends on it but
  `locode-engine` and the facade.
- **`locode-skills`**: the same shape for skills (landed with Task 32 S2).

**Why two crates rather than one.** A single `locode-context` crate was drafted and
rejected as too broad: "context" names no feature, and the two have almost nothing in
common beyond riding the same envelope — different roots, different file formats,
different refresh rules, different budgets. Codex draws the line the same way: skills
are a crate (`codex-core-skills`, plus `ext/skills`) while `AGENTS.md` lives in
`core/src/agents_md_manager.rs`, and the *envelope* abstraction is its own small crate
(`codex-context-fragments`). Grok splits by layer instead — skills under
`xai-grok-tools/src/implementations/skills/`, `AGENTS.md` under
`xai-grok-agent/src/prompt/` — which is the same refusal to merge them.

**Why not in `locode-host`.** The loader never used `Host`; it reads with `std::fs`
directly (ADR-0023's implementation note explains why: discovery legitimately spans
ancestors above the tool jail). It sat there for want of a better home, not by design.
The one genuine host primitive it needs — `find_root_from_markers`, the cwd→root walk
that the settings loader also uses — **stays in `locode-host`** (now
`locode_host::find_root_from_markers`), which keeps the dependency one-way and avoids
a cycle.

**We do not adopt codex's third crate.** The shared envelope machinery (render a
`User` `<system-reminder>`, diff it, decide when to re-inject) stays in
`locode-engine`, which already owns injection. It becomes its own crate only if a
third consumer appears.

**Consequence for releases:** the published set grows from 8 crates to 10. Publish
order gains both after `host`:
`protocol → tools → host → instructions → skills → provider → packs → engine → core → exec`.
