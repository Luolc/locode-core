# Task 35 — session picker

Per [`docs/autonomous-workflow.md`](../../docs/autonomous-workflow.md). Design and
rationale: [ADR-0029](../../docs/decisions/ADR-0029-session-picker.md) (Accepted
2026-07-29, with the three review resolutions recorded in it).

## Phase 0 — status analysis

- **State**: `--resume` requires a session id (`cli.rs:63`); `-c` continues the
  newest rollout in this directory. `locode-host` can find the latest
  (`trace.rs:363`) or find one by id (`:377`), but cannot **list**. The TUI has an
  overlay class that owns input (`approval.rs`) and a composer-anchored dropdown
  (`ui/dropdown.rs`) — no full-screen chooser.
- **Minimal next unit**: the listing function. It is pure, host-side, and every
  later slice consumes it.
- **Why now**: the user asked for it ahead of P0.5, and it is small enough to land
  between the doc work and the pack workstream.
- **Prereqs**: rollout headers carry everything a row needs except the title
  (`SessionMeta`, `trace.rs:33-61`) — verified by reading the struct, not assumed.
- **Risks**: (1) listing cost grows with session count — one open per file;
  (2) the title needs a second read, which must not block the first paint;
  (3) a picker is the first UI in this repo whose *appearance* is the behavior,
  so it needs a render snapshot or it will regress invisibly.

## Phase 1 — source revisit

Recorded in ADR-0029 §Context with fresh citations: Claude Code
(`main.tsx:988`, `ResumeConversation.tsx`, `LogSelector.tsx:671-679,1477`), codex
(`tui/src/resume_picker.rs:67-80,772`), grok (`app/modals.rs:766`,
`app_view.rs:302-320`). Consensus shape and the three deliberate refusals are in
the ADR's Decision; not restated here.

## Phase 2 — design

### Slice 1 — `list_sessions` (host, pure)

`locode-host/src/trace.rs`:

```rust
pub struct SessionSummary {
    pub id: String,
    pub path: PathBuf,
    pub cwd: PathBuf,
    pub harness: String,
    pub model: String,
    pub branch: Option<String>,
    pub last_active: SystemTime,
}

pub enum SessionScope<'a> { Cwd(&'a Path), All }

pub fn list_sessions(sessions_root: &Path, scope: SessionScope<'_>) -> Vec<SessionSummary>
```

- Reads **line 1 only** plus the file's mtime — never the whole rollout.
- `kind != "main"` is skipped (ADR-0029 resolution 2), as is anything whose header
  will not parse (ADR-0024 §2.4 tolerance — a bad rollout is invisible, not an
  error row).
- **No harness filter** (resolution 1): every harness's sessions are listed, and
  the caller switches the pack from `harness` on resume.
- Sorted by `last_active`, newest first.

**Test matrix (slice 1)**

| Target | Test |
|---|---|
| Newest first | three rollouts with distinct mtimes → order asserted |
| Cwd scoping | two cwd dirs → `Cwd` sees one, `All` sees both |
| Unreadable rollouts are invisible | empty file, non-JSON line 1, line 1 that is not `session_meta` → all skipped, the good one still listed |
| Non-main kinds are skipped | `kind: "subagent"` → absent |
| Header fields survive | harness/model/branch read back from a written rollout |
| Missing root | no directory → empty vec, no error |

### Slice 2 — the picker overlay + `-r` with no argument

`-r [SESSION_ID]` (clap `num_args(0..=1)`); no value → open the picker. New
`ui/picker.rs` + app state, input owned while open (approval overlay's pattern).
Two lines per row: title, then a dim metadata line. Keys: `↑↓` move, `↵` resume,
`/` filter, `a` widen to all directories, `esc` cancel. Titles fill in on a second
pass so the first paint does not wait.

**Targets**: reducer table tests for every key; a `TestBackend` snapshot of the row
shape (reusing the composer/dropdown pattern); an empty-list state that says so rather
than rendering a blank box.

### Slice 3 — `/resume`

A `SlashCommand` that refuses mid-run inside its own `execute` via `ctx.is_running`,
copying `/new` (`builtin.rs:328-334`) — not a new rule, not a caller-side check.

## Phase 3 — preset targets

- [ ] `list_sessions` passes the slice-1 matrix above.
- [ ] `-r` with an id behaves exactly as today; a nonexistent id **errors** (does
      not open the picker — resolution 3).
- [ ] `-r` with no value opens the picker; `↵` resumes the highlighted session and
      the run rehydrates with that rollout's harness.
- [ ] `/resume` opens the same picker when idle and refuses with a notice mid-run.
- [ ] The row-shape snapshot exists and is stable across a re-render.

## Result — slices 1-3 (2026-07-29)

**Slice 3.** `/resume` is a `SlashCommand` that refuses mid-run inside its own
`execute` (`ctx.is_running`), copying `/new` — the rule stayed where `/new` put it,
and ADR-0026 needed no amendment. Mid-session switching needed one new engine
command, `UiCommand::Resume(id)`, which rebuilds through the same `build_session`
path startup uses and then replays the recovered transcript; a failed resume reports
and leaves the current session intact rather than stranding the user.

Deviation worth recording: the picker is **not** an App overlay. `Cmd::ResumePicker`
sets a flag the loop reads at the top of its next iteration, because the picker needs
the terminal and the filesystem and the reducer may touch neither. That also means
`/resume` and the startup `-r` run *exactly the same* picker code.

**Slice 2.** Shipped as planned (#260).

## Result — slice 1 (2026-07-29)

`list_sessions` + `SessionSummary` + `SessionScope` shipped (#259), with
`read_session_title` following in slice 2 for the lazy second pass. Deviation from the
plan: none in shape. One correction to the plan's premise — the repo already uses
`TestBackend` in five files, so slice 2's snapshot follows an existing pattern rather than
introducing one (see META-AGENTS F1's dated correction).
