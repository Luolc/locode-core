# ADR-0011: Search tools use ripgrep — host-resolved, bundled at packaging

## Status
Accepted

## Date
2026-07-17

## Context
`Grep` and `Glob` need fast, gitignore-aware, regex-capable search with consistent
behavior across machines. All the reference CLIs standardize on **ripgrep (`rg`)**.
Two forces shape the decision:

- **Determinism.** Depending on whatever `rg` happens to be on a user's PATH (or
  none at all) makes search behavior vary by version/availability. We want `rg`'s
  *exact* semantics, pinned — not a hand-rolled walker with divergent gitignore /
  output behavior. (SPEC Open Question #2; the prior default was "rg if on PATH,
  else walk".)
- **Layering.** `locode-core` is a **library**; the shipped binary and its
  packaging live separately (`locode-exec` here, `locode-app` later). *How* `rg`
  gets onto disk is a packaging concern and must not leak into the core.

Studied bundling mechanisms (verified against the survey submodules):
- **Grok Build** — `build.rs` (release-gated) downloads the prebuilt **static** `rg`
  for the target triple from ripgrep's GitHub releases (or copies a local binary via
  `GROK_TOOLS_BUNDLE_RG_PATH`), `include_bytes!` embeds it, and the runtime
  self-extracts once to `~/.grok/vendor/`, `chmod +x`, caches in a `OnceLock`;
  Windows falls back to PATH. Fallback chain: override env → embedded → PATH.
- **Claude Code** — vendors per-platform `rg` **sidecar** files under
  `vendor/ripgrep/<arch>-<platform>/rg`; prefers a system `rg` but invokes it by the
  bare name `'rg'` (never an absolute cwd-relative path) to avoid PATH hijacking;
  `USE_BUILTIN_RIPGREP` forces the vendored copy.

## Decision
1. **Search engine is ripgrep, unconditionally — no hand-rolled walker.** `Grep`
   shells out to a resolved `rg`; `Glob` uses `rg --files` + glob filtering. If `rg`
   cannot be resolved, the tools return a **soft `Respond` error** (not a silent
   divergent fallback).
2. **The `rg` path is injectable through `locode-host`** — a cached `rg` resolver
   with this order: (a) explicit override env `LOCODE_RG_PATH` (tests/packaging), (b)
   a host-provided bundled/self-extracted path, (c) bare `rg` on PATH, invoked *by
   name* (PATH-hijack hygiene, per Claude Code). The core library never assumes how
   `rg` got there — keeping it testable and packaging-agnostic (aligns with the host
   seam, ADR-0008).
3. **Bundling is a packaging-layer concern, following Grok's pattern.** Behind a
   `bundle-rg` cargo feature (release-gated), a `build.rs` downloads the pinned
   static `rg` for the target triple (or copies a local binary via an override env
   for offline/hermetic CI), `include_bytes!` embeds it, and the runtime
   self-extracts once to a cache dir (`$XDG_CACHE_HOME`/`~/.cache/locode/vendor` or
   platform equivalent), `chmod +x`, atomic-rename for concurrency. Wired into
   `locode-exec` now; reused by `locode-app`. Windows falls back to PATH (ripgrep
   ships `.zip`); zip extraction is a later add.

## Alternatives Considered
### Hand-rolled walker (`ignore`/`walkdir` + regex), as fallback or primary
- Pros: no external binary; pure Rust; Windows for free.
- Rejected as the default: divergent semantics from `rg` (gitignore, output shape)
  produce inconsistent results, and the user explicitly wants `rg`. Kept in mind as
  the "link the `grep-*`/`ignore` crates" path (ripgrep's own engine as a library) if
  we ever want zero external binary — that would be a **future ADR superseding this**.

### System `rg` only (the prior "rg if on PATH, else walk" default)
- Rejected: non-deterministic (version drift / absence); not robust for a shipped binary.

### Sidecar binary next to the executable (Claude Code)
- Viable, and **better for a notarized macOS GUI app** — a signed sidecar avoids the
  Gatekeeper issues an extracted-then-exec'd binary can hit. We choose embed-first for
  the CLI's single-file convenience; `locode-app` may prefer the sidecar for its
  notarized bundle. The host resolver abstracts both, so this stays a packaging choice,
  not a code fork.

## Consequences
- **Resolves SPEC Open Question #2** and supersedes the prior "rg if on PATH, else
  walk" search default (SPEC/plan).
- **Task 11 simplifies:** `Grep`/`Glob` call the host resolver; there is no walker to
  build or test. Missing `rg` yields a clear soft error.
- The "`rg` absent" risk is mitigated by bundling; dev/CI resolve via PATH or the
  `bundle-rg` feature.
- **Licensing:** ripgrep is MIT/Unlicense — freely redistributable; carry its license
  in a `NOTICE`/`licenses/` entry when shipping bundled binaries.
- **macOS notarization** caveat is noted for `locode-app`; the host resolver keeps
  embed-vs-sidecar a packaging decision.
- Binary size: each release artifact embeds only its own target's `rg` (~5 MB).
