# ADR-0029: Resuming without an id — a session picker

## Status

**Draft — awaiting the user's OK.** The feature and its shape were agreed in
conversation (2026-07-28); this records the design so it can be read before the
code exists. Flip to Accepted on approval.

## Date

2026-07-28

## Scope

Covers **choosing** a session to resume in the interactive app: the entry points,
how sessions are listed, what a row shows, and the v0 feature line. It does **not**
change what resuming *does* — rehydration is ADR-0016/ADR-0024 and is untouched —
and it does not touch the headless path (`-p` keeps requiring an explicit id, where
a picker would have nothing to attach to).

## Context

`--resume` takes a session id today (`cli.rs:63`), and nobody remembers a session
id. The only escape is `--continue`, which is all-or-nothing: the newest session in
this directory, no choice. In practice that makes every older session unreachable
without going through `~/.locode/sessions/` by hand.

**All three studied harnesses ship a picker**, and they converge on its shape while
differing only on the entry point:

- **Claude Code** — the flag's value is *optional*: `-r, --resume [value]` is
  documented as "Resume a conversation by session ID, or **open interactive picker
  with optional search term**" (`src/main.tsx:988`). The picker is
  `src/screens/ResumeConversation.tsx` over `src/components/LogSelector.tsx`; each
  row is a **title** (custom name → summary → first meaningful user message,
  truncated to width) plus a **dim metadata line**, with ` · <project path>`
  appended when listing beyond the current directory
  (`LogSelector.tsx:671-679`). Sorted by `modified`, newest first (`:1477`).
- **Codex** — a subcommand, `codex resume`, backed by `tui/src/resume_picker.rs`
  (6406 lines): pagination (`PAGE_SIZE = 25`, prefetch at 5 from the end), a
  sort/filter toolbar, a density toggle, and **lazily loaded transcript previews**
  on a background channel (`load_transcript_preview`, `:772`). Rows carry a date, a
  git branch, and the cwd behind icons (`:70-76`).
- **Grok Build** — a slash command: `/resume` opens `ActiveModal::SessionPicker`
  (`app/modals.rs:766`). Its entry is the richest — `id / summary / updated_at /
  created_at / cwd / hostname / source / model_id / num_messages / last_active_at /
  branch / repo_name / worktree_label` (`app/app_view.rs:302-320`) — because grok
  also lists sessions from other machines.

Common to all three: **title + one dim metadata line, newest-activity first,
current directory by default with a toggle to everything.** The differences are
entry point and depth, not shape.

What we already have: `find_latest_rollout` and `find_rollout_by_id`
(`trace.rs:363,377`), a `SessionMeta` header on line 1 of every rollout
(`trace.rs:33-61`) carrying `session_id / kind / cwd / git / harness / api_schema /
model / cli_version`, and tolerant reading (ADR-0024 §2.4). What we lack: any way to
**list** sessions, and any UI class for a full-screen chooser.

## Decision

**1. Two entry points, no new command surface.**

- `-r, --resume [SESSION_ID]` — the value becomes optional. With an id, resume
  directly (today's behavior, unchanged). Without one, open the picker. This is
  Claude Code's shape, chosen because we already have `-r`: making its value
  optional is additive, where a codex-style subcommand would add a second way to
  say the same thing.
- `/resume` — opens the same picker mid-session (grok's entry).
- **`--continue` is untouched and gets no picker.** Its entire value is *not*
  choosing.

**2. `/resume` refuses itself while a run is active — the rule `/new` already
established.** `/new` returns `CommandResult::Error("finish or cancel the run
before /new")` when `ctx.is_running`, with the reason stated in place: rebuilding
the session under a live turn strands the run's events (`commands/builtin.rs:328-334`).
`/resume` is the same class of action and takes the same path. **This is not a new
rule and needs no change to ADR-0026** — the seam is the command's own `execute`,
not the caller. It does not queue: making the user pick a session and then wait
would be a worse interaction than telling them to finish the turn.

**3. Listing scans the directory; the reserved index stays reserved.** A new
`list_sessions(sessions_root, scope) -> Vec<SessionSummary>` in `locode-host`
enumerates `rollout-*.jsonl`, reads **line 1 only** plus the file's mtime. ADR-0024
reserved "a rebuildable sessions listing index"; it stays reserved, because an index
is state that can disagree with the files while a directory scan *is* the truth, and
"land on demand" is exactly what that reservation said. One `open` + one line per
session is milliseconds for hundreds of sessions; revisit with a measurement, not a
guess.

**4. Row shape follows the three-harness consensus.** Title line + dim metadata
line; newest activity first; current directory by default with a key to widen to all
directories. The title is the first user message's first line — which is *not* in
the header, so it needs a second read of a few records. That runs as a **second
pass**: the list paints from headers + mtimes immediately, titles fill in after
(codex's lazy-preview pattern, minus the preview pane).

**5. v0 stops here.** Deferred, each with who has it: transcript preview pane
(codex), deep/agentic search (claude, grok), rename (claude), tag tabs and
branch filtering (claude), cross-machine sources (grok), fork-on-resume
(claude's `--fork-session`). Substring filtering over the visible fields is in;
everything else waits for a reason to exist.

## Alternatives Considered

### A `codex resume`-style subcommand
- Pros: a clean place to hang `--last`, `--all`, and future flags.
- Rejected: we already have `-r`; a subcommand would make two spellings of the same
  intent, and the flag form is what our `-c`/`-r` pair already teaches.

### Build the ADR-0024 listing index now
- Pros: O(1) listing regardless of session count; the seam is already reserved.
- Rejected **for now**: an index must be maintained, and a stale index shows
  sessions that no longer exist or hides ones that do. The scan is self-correcting.
  Revisit when a measurement says the scan is slow, which is what the reservation
  anticipated.

### Show the picker for `--continue` too
- Rejected: `-c` exists precisely so you don't have to choose. Two flags that both
  prompt would leave no way to say "just continue".

### Port codex's preview pane in v0
- Rejected for v0: it needs a background loader, a cache, and a two-pane layout —
  roughly double the picker's own work. The title + metadata line is enough to
  recognize a session; add the preview if that turns out to be false.

## Consequences

- **A new UI class in the TUI**: a full-screen overlay that owns input while open.
  The approval overlay (`approval.rs`) is the precedent for input ownership; the
  slash dropdown is not (it is a narrow strip anchored to the composer).
- **Listing cost grows with session count** — one file open per session. Acceptable
  at today's scale; the ADR-0024 index is the escape hatch, and the trigger for
  taking it is a measurement.
- **The title pass must not block the first paint.** If it ever does, the picker
  feels slower than the thing it replaced (`-c`, which reads one file).
- **A rollout that will not parse is skipped, not shown as an error row.**
  ADR-0024 §2.4's tolerant reading already governs this; the picker inherits it.
- `-p` headless is unchanged: an id or nothing.

## What pins these claims (META-AGENTS §5.1)

The invariants above are testable and must be tested with the code:

- ordering, current-directory scoping, and skipping unreadable rollouts →
  `list_sessions` unit tests over a temp `sessions_root`;
- `/resume` refusing mid-run → a command test mirroring `/new`'s;
- picker key handling (move / select / cancel / filter) → reducer table tests;
- the rendered row shape → a `TestBackend` snapshot. This is the first use of the
  render-snapshot idea in META-AGENTS §6.1, and a picker is a good first case: it is
  pure layout with no engine behind it.

## Open Questions

1. **Does the picker show sessions from other harnesses?** A rollout records its
   `harness`; resuming a `codex` session under `--harness claude` would rehydrate a
   history whose tool calls no pack in the run can pair. Options: list everything and
   switch the pack on resume (the header knows it), or list only the current
   harness's sessions. *Leaning: list all, switch the pack from the header — the
   metadata is already there, and hiding sessions is the more surprising behavior.*
2. **What does the picker do about subagent/workflow rollouts** once those exist
   (`SessionMeta.kind` is already an open string)? *Leaning: `kind == "main"` only,
   as `find_latest_rollout` already filters (`trace.rs:371`).*
3. **Should `-r` with an id that does not exist fall back to the picker** rather
   than erroring? *Leaning: no — a typo'd id silently opening a menu hides the typo.*
