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

## Consequences
- When a sandbox or workspace policy arrives, one function changes.
- The path jail + shell caps give a small, safe minimal agent without a permission UI.
- Arbitrary-shell risk is bounded by timeout/byte caps and the jail; first-class FS tools (not a shell-only surface) keep the privilege surface smaller.
