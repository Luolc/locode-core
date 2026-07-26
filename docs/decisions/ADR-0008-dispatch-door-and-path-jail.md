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

## Amendment (2026-07-21): the approval *seam* is in scope — the prompt still is not

The "Interactive approval prompts — out of scope" alternative above is
narrowed by ADR-0017: the engine now consults an injected `Approver` **in
front of** the dispatch door (`dispatch_batch`, before `ToolCtx`
construction), with a headless `AllowAll` default. What remains out of scope
here is unchanged: no prompt UI, no terminal interaction, and no policy inside
individual tools or inside `Registry::dispatch` itself — the tools crate stays
interaction-free (ADR-0017 rejected exactly that as Option P2). See ADR-0017
for the trait, vocabulary, and event/record semantics.

## Amendment (2026-07-24): `~` is expanded before the jail runs

`Host::resolve_in_jail` now expands a leading `~` / `~/…` against `$HOME` as its
first step, before the absolute-vs-relative branch.

The old behavior was a bug, not a policy: `Path::is_absolute()` is false for
`~/…`, so the path took the relative branch and became `<cwd>/~/…`. That path
lives *inside* the workspace root, so both jail checks passed and the tool failed
later with a plain "not found" — for a directory literally named `~` that the
caller never asked for. Both reference harnesses expand it (Claude Code's
`expandPath`, applied to Read/Write/Edit/Glob input — `src/utils/path.ts:57-64`,
`FileReadTool.ts:392`; grok's `shellexpand::tilde`), and our own settings loader
already did (`locode-host/src/settings.rs:376`), so the tool path was the one
place in the codebase where `~` meant nothing.

Three properties keep this inside the ADR's existing posture:

- **Expansion is not a permission.** The expanded path faces the same two checks.
  Under `PathPolicy::Jailed` a home path outside the workspace is still
  `PathError::Escape` — the change is only that the rejection names the path the
  caller meant instead of a fictional one.
- **`$HOME`, never `$LOCODE_HOME`.** `~` is the OS home in every shell and every
  surveyed harness; `$LOCODE_HOME` (ADR-0024) relocates only our dotfolder.
  Conflating them would make `~/x` denote different files depending on an
  unrelated override.
- **Only `~` and `~/`.** `~user` is untouched — resolving another user's home
  needs a passwd lookup we have no reason to perform, and both harnesses stop at
  the same place. With no `$HOME`, the path is left unchanged rather than guessed.

Ported-pack tool *descriptions* are unchanged: they are verbatim reproductions
(ADR-0012), and this is host-level path resolution shared by every pack.

**Still out of scope:** reading `~/.locode` (or any path outside the workspace)
from a jailed session. That is a genuine widening of the jail — an extra
permitted root — and needs its own decision, not a side effect of a bug fix.

## Amendment (2026-07-24): the locode home is readable from inside the jail — never writable

The previous amendment left one thing explicitly open: "reading `~/.locode` (or any
path outside the workspace) from a jailed session … needs its own decision."
[ADR-0025](ADR-0025-agent-skills.md) forces and makes that decision.

Skills are advertised to the model as a `<system-reminder>` listing carrying the
**absolute path** of each `SKILL.md`, and there is no skill tool — the model reads
the file itself (ADR-0025 §4). User skills live in `~/.locode/skills/`, outside the
workspace root, so without an exception the jail would reject the very paths the
listing just advertised.

**Decision** *(user)*: the **locode home** (`$LOCODE_HOME`, else `~/.locode`) and
every **skill root contributed from outside it** (`extends` dotfolders,
`skills.extra` entries) are **readable** from inside the jail. They remain **not
writable**: create, write, edit and delete are rejected exactly as before. The
relaxation is on the read path only.

A narrower variant — admitting only the individual skill directories discovery
returned, leaving `~/.locode/` itself closed — was drafted, raised together with
its cost, and **overruled**: the simpler rule is easier to reason about and to
explain, and read-only is judged sufficient.

**The cost, recorded plainly:** `~/.locode/sessions/` holds full JSONL transcripts
of previous runs across every project. A jailed run can now read them. Nothing
advertises those paths, so the model will not stumble into them, but a prompt that
asks will succeed. Read-only bounds the damage: no run can rewrite or delete
another run's history. This is also moot under
`--dangerously-skip-permissions`, which already lifts the jail entirely; the
exception exists so that skills work in a *jailed* session too.

The rest of the posture is unchanged: one dispatch door, one jail, and no per-tool
policy. This is a data-scope exception with an asymmetric read/write rule, not a
new mechanism.

## Amendment (2026-07-24): unrestricted is the default; the jail is opt-in until permissions land

The 2026-07-18 amendment made the jail "a configurable policy with a skip escape
hatch", jailed by default. **The default now inverts** *(user decision)*: a run is
`PathPolicy::Unrestricted` with an auto-allowing approver unless the user passes
`--restricted` (alias `--no-yolo`).

**Why.** The two halves of the restricted posture are not equally finished. The
jail (this ADR) and the approval *seam* (ADR-0017) both ship, but the **permission
rules behind the seam do not**: `permissions` `{allow, deny, ask, default_mode}` is
still a reserved settings key (ADR-0024 §1.4). With nowhere to record an answer,
the restricted path asks about the same command on every call and cannot be told
"yes, always". That is not a safety feature; it is a prompt loop that trains the
user to approve reflexively — which makes the gate *weaker* once it does exist.
Defaulting to unrestricted states the real posture instead of implying an
enforcement we have not built.

**Both modes announce themselves**, because a silent default of "no jail, no
prompts" is exactly the kind of thing a user must not discover by accident:

- default → *"running without approval prompts, and with file access outside the
  working directory; pass `--restricted` to limit both."*
- `--restricted` → a notice that the mode is incomplete and answers cannot be saved.

The strings live once in `locode-exec` and are used verbatim by both surfaces
(stderr headless; a `Notice` block in the TUI, which has no stderr).

**`--dangerously-skip-permissions` / `--yolo` is retained as an accepted no-op** —
it names the default now. Removing it would break existing invocations (the README
used it) for no gain; it is hidden from `--help` and errors if combined with
`--restricted`, since asking for both is contradictory.

**Nothing about the mechanism changes.** One dispatch door, one jail, no per-tool
policy; `PathPolicy` keeps both variants and every jail test still runs both ways.
This is a default, and it is expected to flip back when the permission rules land —
at which point `--restricted` stops being a preview and this amendment should be
revisited.

## Amendment (2026-07-25): the jail may hold more than one root (`--add-dir`)

`HostConfig.extra_roots` (default empty) adds jail roots beyond
`workspace_root`. A jailed path is accepted when it resolves under the
workspace root **or** any extra root. This lifts the deferral recorded in
ADR-0023, whose `--add-dir` design was accepted but blocked on exactly this
security-posture change.

**Additive, never relaxing.** Extra roots do not weaken any check: `..`,
absolute, and symlink escapes are rejected the same way, and a path outside
*every* root is still an `Escape`. The only difference is the size of the
allowed set, and it grows only when a caller passes `--add-dir`. Roots are
canonicalized at `Host::new`, so a non-existent directory is a startup error
naming the path the user typed — never a silently narrower jail.

**Two forms in the lexical pre-check.** Step (1) is a cheap lexical prefix test
and step (2) is the authoritative canonical one. Checking step (1) against the
canonical roots alone breaks a root reached through a symlink: on macOS
`/var/…` canonicalizes to `/private/var/…`, so an absolute path the canonical
check would accept is rejected before it gets there — and a monorepo mounted
behind a symlinked path is the case that matters. Each root is therefore kept
in both its as-given (lexically normalized) and canonical form; step (1)
accepts either, step (2) still requires canonical. Claude Code checks the same
two forms (`permissions/filesystem.ts:688`). Widening a pre-filter ahead of an
unchanged authoritative check cannot admit an escape.

**One flag, three effects.** `--add-dir` widens the jail *and* contributes the
directory's `AGENTS.md` (ADR-0023's `extra_roots`) *and* its `.agents/skills`
(ADR-0025). Claude and Codex likewise bind jail-widening to the same flag
(`claude-code: main.tsx:1000`; `codex: shared_options.rs:61`). Claude Code gates
the CLAUDE.md half behind `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`,
default off (`claudemd.ts:938-975`); we do not, because instruction and skill
discovery is loop-adjacent engine machinery rather than pack fidelity
(ADR-0023 §fidelity boundary), and the motivating case — pointing at one
subtree of a monorepo too large to open at its root — is worthless without them.

## Amendment (2026-07-26): the jail widens on a running session (`/add-dir`)

`Host::add_root` adds a jail root at runtime, so `/add-dir <path>` widens a
**live** session. The root lists moved behind an `Arc<RwLock<…>>` shared across
clones: tools already hold `Arc<Host>`, so the alternative — rebuilding the host
— means rebuilding the session and discarding the conversation. Claude Code
keeps its `additionalWorkingDirectories` mutable for the same reason.

The guard is taken and released inside the two membership checks, never held
across `resolve_in_jail`'s `await`. Adding an existing root is a no-op, so
repeating the command is harmless; a bad path errors and leaves the jail
untouched.

Nothing about the checks relaxes — this only changes *when* the allowed set can
grow. The command also registers the directory as a discovery root
(`Session::add_root`), which the per-turn instruction and skill rescans
(ADR-0023, ADR-0025) pick up on the next turn without any re-injection.

**Not persisted**, unlike `/model` and `/effort`. Those are preferences; a
working directory belongs to the task at hand, and carrying it into every future
session would keep widening the jail of unrelated runs. `--add-dir` is how a
root becomes part of a session's startup.
