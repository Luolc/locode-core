# ADR-0008: One dispatch door + workspace path jail as the v0 security posture

## Status
Accepted

## Date
2026-07-17

## Context
Codex's central lesson: do not scatter allow/deny checks inside tools. Every side-effecting call should funnel through a single `dispatch` function so policy/sandbox can be added in one place later. locode-core is headless and cannot prompt a human, so v0 policy must be decidable up front. The v0 threat model is single-user, trusted workspace.

## Decision
Route **every** tool call through one `dispatch(name, args, ctx)` door — even under always-allow. v0 policy is **always-allow inside a `workspace_root` path jail**: resolve every path under `workspace_root` and reject `..` escapes; give the shell tool a hard timeout and a max-output-byte cap with a truncation marker. Non-interactive hygiene (path resolution, caps) lives in `locode-host`, not in a permission UI. Tool-result truncation is a shared post-process applied before the model re-enters, not per-tool ad hoc.

## Alternatives Considered
### Per-tool permission checks (Claude Code style)
- Rejected: policy sprawl across six tools; adding a sandbox later means editing all of them.

### OS sandbox (Seatbelt/Landlock/seccomp) in v0
- Deferred: overkill for single-user/personal use. It slots behind the same dispatch door when multi-tenant/CI use arrives — a change to one function, not six tools.

### Interactive approval prompts
- Out of scope by ADR-0001 (headless; no human to prompt).

## Amendment (2026-07-18): the jail is a configurable policy with a skip escape hatch
The path jail is the **default**, not a hard wall — every studied harness offers a
full-access / bypass mode, and since we are headless (the jail substitutes for their
permission prompt) a caller must be able to opt out. Model it as a host **path policy**,
minimizing Codex's `SandboxPolicy`:

- `PathPolicy::Jailed { root }` — **default.** The first-class FS tools (`read_file`,
  `search_replace`, `grep`, `list_dir`) resolve every path under `root`; `..`/absolute
  escapes are a soft `Respond` error. (≈ Codex `WorkspaceWrite`.)
- `PathPolicy::Unrestricted` — resolve relative paths against `cwd`, allow absolute /
  out-of-root paths, no rejection. (≈ Codex `SandboxPolicy::DangerFullAccess` /
  Claude Code `--dangerously-skip-permissions` / `bypassPermissions`.)

Extensible later to `ReadOnly` and a real OS sandbox — the deferred policy layer; this
two-value toggle is its safe seed and does not pull that work forward. Set on the host by
the caller; `locode-exec` exposes it as **`--dangerously-skip-permissions`** (matching
Claude Code) with a **`--yolo`** alias; **default = `Jailed`** (opt *into* danger).

Scope notes: (1) the jail only ever constrained the **structured FS tools** — the shell
tool was never path-jailed — so `Unrestricted` mainly widens the FS tools to match the
shell's existing reach. (2) The shell's **timeout + output caps stay on** even under
`--yolo`: those are robustness limits, not access control.

## Consequences
- When a sandbox or workspace policy arrives, one function changes.
- The path jail + shell caps give a small, safe minimal agent without a permission UI.
- Arbitrary-shell risk is bounded by timeout/byte caps and the jail; first-class FS tools (not a shell-only surface) keep the privilege surface smaller.
- The jail is default-on but skippable (`--dangerously-skip-permissions`/`--yolo`), so a trusting caller gets the harnesses' full-access behavior without a code change.

## Amendment (2026-07-18): shared truncation applies at the dispatch door

`truncate_for_model` (the shared middle-truncation post-process) moved from
`locode-host` — where nothing consumed it — into `locode-tools`, applied
centrally inside `Registry::dispatch` when the `tool_result` is built (both
success and error payloads). Rationale: the byte budget is a property of the
**model-facing boundary**, not of OS access, and the dispatch door is the one
place every result passes through — no tool can flood the model regardless of
its own caps, and the engine needs no `locode-host` dependency (resolving the
Task-12 handoff's open concern #1, option "at the door" over "in the engine"
or "facade wraps the registry"). `HostConfig.model_output_budget` (never read)
was removed.
