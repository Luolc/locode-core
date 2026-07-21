# ADR-0001: locode-core is a headless core library, no TUI

## Status
Accepted

## Date
2026-07-17

## Context
locode is a custom coding agent. Production coding agents (Claude Code, Codex, Grok Build, OpenCode) all share one spine: a *sample → run tools → append results → re-sample* loop. In every one of them, **headless mode (`-p`/`exec`/`run`) is not a separate agent loop** — it is the production loop with interactive presentation removed and an output emitter swapped in. We want to build that spine once, cleanly, as a reusable library, and layer UX on top later.

## Decision
This repo (`locode-core`) delivers the **headless engine only**: the agent loop, typed tool registry, dialect layer, provider abstraction, host seam, and a single structured-output contract. It contains **no TUI and no interactive permission prompts**. A separate future repo (`locode-app`) will build the TUI and richer features on top of these crates. A *minimal* headless binary (`locode-exec`) ships here purely to exercise the library end-to-end.

## Alternatives Considered
### Build the TUI and core together
- Pros: one repo, immediate demoable product.
- Rejected: couples presentation into the loop (Claude Code couples Ink render methods into the tool contract — an anti-pattern we explicitly avoid). Slows the core and muddies its boundaries.

### Pure library with no binary at all
- Pros: strictest "core library" reading.
- Rejected: a minimal `locode-exec` gives a real end-to-end target and a reference consumer at near-zero cost; the full-featured binary still lives out-of-repo.

## Consequences
- The engine must be **drivable programmatically** (a `Session`/`Engine` API), not only via a binary — `locode-app` is a first-class consumer.
- Permissions must be **decidable without a human** (auto-allow within the workspace jail); interactive prompting is out of scope by construction.
- Interactive permission prompts, TUI, MCP, subagents, plan mode, streaming UI are **deliberately deferred** as extension slots, not built here.

## Amendment (2026-07-21): TUI components move into this repo — the headless boundary becomes a crate boundary

The separate-repo plan (`locode-app`) is dropped: the TUI components will be
built **in this repository**, as separate crate(s) layered on the core. What
this ADR actually protects is unchanged and stays binding — the **core crates**
(protocol/tools/packs/provider/host/engine/facade/exec) remain headless: no TUI
dependencies, no interactive prompts, no presentation coupled into the loop or
the tool contract (the rejected Claude-Code-style coupling stays rejected).
Interaction reaches the engine only through the public seams built for it:
the approval seam (ADR-0017), the cancel handle (ADR-0018), the event sink
(ADR-0014), and session continuity (ADR-0016).

Rationale: the "Build the TUI and core together" alternative above was rejected
for *coupling*, not co-location. With the seams now in place the coupling risk
is structural, not organizational — a crate boundary provides the isolation the
repo boundary stood in for, and one repo removes cross-repo version churn for a
solo project. The README introduces the TUI surface when it actually ships.
