# Task 7 — `locode-host`: the injectable side-effect seam

> **Resolved / pending:** the shared `truncate_for_model` is applied in the **engine loop**
> (post-dispatch, before appending each `tool_result`), since `locode-host` and
> `locode-tools` are siblings — the Task 6 loop already marks this seam. **Pending user
> approval:** the new deps this plan proposes (`nix` for the `unsafe`-free process-group
> kill, dev-only `tempfile`) before implementation. See `tasks/plans/README.md`.

Detailed implementation plan. Refines `tasks/todo.md` → Task 7 and `tasks/plan.md`
Phase 2. Written **before** implementation, grounded in the actual harness source
(Grok Build primary; Codex + Claude Code for contrast). Every non-obvious decision
cites `file:line`.

Repo paths are absolute where they matter. Harness source lives under
`~/dev/coding-cli-survey/submodules/{grok-build,codex,claude-code}`.

---

## 1. Purpose & scope

`locode-host` is the **one place** the whole system is allowed to touch the OS.
Per ADR-0008, every side effect funnels through a single dispatch door and the
host seam; tools never call `std::fs`/`Command` directly (`SPEC.md:123`). The host
gives us, for v0:

1. **Path jail** — resolve a model-supplied path against a `cwd` under a fixed
   `workspace_root`, rejecting `..`/absolute escapes **and** symlink escapes, for
   both existing and not-yet-existing paths.
2. **Shell exec with limits** — run a command through the platform shell, capture
   stdout+stderr+exit code, enforce a **hard timeout** (kill the whole process
   *group*, not just the direct child), a **max-output-byte cap** with a
   truncation marker, and honor **cooperative cancellation** via the
   `CancellationToken` already on `ToolCtx` (`crates/locode-tools/src/ctx.rs:24`).
3. **FS helpers** — jailed read / write / stat (with mtime, for the Task 10 edit
   freshness invariants), all resolving through the jail first.
4. **Shared `truncate_for_model`** — one tool-agnostic post-process that bounds any
   tool's model-facing text before the model re-enters (ADR-0008: "a shared
   post-process applied before the model re-enters, not per-tool ad hoc"
   `ADR-0008:13`).

`locode-host` depends **only** on `locode-protocol` (`SPEC.md:83`,
`crates/locode-host/Cargo.toml`). It is a *sibling* of `locode-tools`, so it
**cannot** reference `ToolError`/`ToolCtx`; it defines its own plain error types
and the **pack** (which depends on both, Task 8/9) maps them to
`ToolError::Respond`. See §3.6.

### Deferred (seams reserved, not built here)

- **OS sandbox** (Seatbelt / Landlock / seccomp). Explicitly deferred by ADR-0008
  (`ADR-0008:19-20`) and SPEC assumption 4 (`SPEC.md:15`). **The seam:** shell exec
  and path resolution are the only OS-touching functions; an OS-sandboxed `Host`
  arrives as an alternative construction/impl behind the same call surface — "a
  change to one function, not six tools" (`ADR-0008:26`). Codex is the deepest
  reference here (`codex-rs/core/src/safety.rs`, `windows_sandbox.rs`); we take its
  *shape* (one policy checkpoint) without its depth.
- **`rg` resolver** — ADR-0011 / Task 11. It lives in this crate
  (`crates/locode-host/src/rg.rs`) but is out of Task 7 scope. This plan reserves
  the module and the `Host` field so Task 11 is a drop-in (§2, §8).
- **Windows first-class support.** v0 targets macOS/Linux (SPEC tech stack). We
  keep the code `cfg`-portable and note Windows fallbacks, but the process-group
  kill and shell selection are Unix-first (§4.2, §5).
- **PTY / interactive / streaming shell, background tasks.** Grok has all of these
  (`xai-grok-shell/src/terminal/{pty_session,streaming_local_terminal,background_task}.rs`);
  v0 needs only the fire-and-forget capture path, modeled on Grok's
  `LocalTerminalRunner` (`xai-grok-shell/src/terminal/local_terminal.rs`).

---

## 2. Module layout

```
crates/locode-host/src/
├── lib.rs        // Host struct, HostConfig, ExecLimits, re-exports, crate docs
├── path.rs       // jail: resolve_in_jail(), PathError, lexical-normalize + canonical-ancestor check
├── shell.rs      // exec(): spawn, capture (bounded), timeout, process-group kill, cancel, ExecOutput/ExecError
├── fs.rs         // jailed read_file/write_file/stat; FileRead/FileStat; canonicalize-with-timeout helper
├── truncate.rs   // truncate_for_model() + byte budget const + char-boundary helpers
└── rg.rs         // (Task 11 — reserved; not in Task 7)
```

`lib.rs` owns the public `Host` and wires the four modules. Tests are inline
`#[cfg(test)]` per module (SPEC testing strategy `SPEC.md:119`), with the
subprocess/tempdir integration tests in `shell.rs`/`path.rs`/`fs.rs`.

---

## 3. Key types & signatures (Rust sketches)

> Sketches, not final code. `unsafe_code = "forbid"` is set workspace-wide
> (`Cargo.toml [workspace.lints.rust]`) — every signature below is achievable
> **without `unsafe`** (see §5, Decision D3 on why we can do process-group kill
> with zero `unsafe`).

### 3.1 The host and its config

```rust
// lib.rs
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// The single OS-touching seam (ADR-0008). One per session; holds the jail root
/// and the exec/truncation limits. Cheap to clone-behind-Arc; construct once.
pub struct Host {
    workspace_root: PathBuf,   // canonicalized once at construction (jail authority)
    limits: ExecLimits,
    model_output_budget: usize,
    // rg: RgResolver,         // Task 11
}

/// Construction-time knobs. Defaults mirror Grok Build (the primary model).
#[derive(Debug, Clone)]
pub struct HostConfig {
    pub workspace_root: PathBuf,
    pub exec: ExecLimits,
    /// Byte budget for `truncate_for_model` (default `truncate::MODEL_OUTPUT_BUDGET`).
    pub model_output_budget: usize,
}

#[derive(Debug, Clone)]
pub struct ExecLimits {
    /// Default per-call timeout when a call passes none. Grok/Codex default = 10s
    /// (`xai-grok-shell/.../mod.rs:21`, `codex-rs/core/src/exec.rs:58`).
    pub default_timeout: Duration,
    /// Hard ceiling; a caller-supplied timeout is clamped to this. ~Claude's
    /// BASH_MAX_TIMEOUT (10 min) is a reasonable ceiling for build/test commands.
    pub max_timeout: Duration,
    /// Max retained output bytes before truncation. Grok = 30_000
    /// (`xai-grok-shell/.../mod.rs:22`).
    pub max_output_bytes: usize,
    /// SIGTERM→SIGKILL grace after a timeout/cancel. Grok bash ≈ 1s
    /// (`.../bash/mod.rs:1429`); Codex IO-drain grace = 2s (`exec.rs:89`).
    pub kill_grace: Duration,
}

impl Default for ExecLimits {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(10),
            max_timeout: Duration::from_secs(600),
            max_output_bytes: 30_000,
            kill_grace: Duration::from_secs(2),
        }
    }
}

impl Host {
    /// Canonicalizes `workspace_root` up front (jail root must be a real, symlink-
    /// resolved absolute path). Errors if it does not exist / cannot canonicalize.
    pub fn new(config: HostConfig) -> Result<Self, PathError>;

    pub fn workspace_root(&self) -> &Path;
    pub fn limits(&self) -> &ExecLimits;
    pub fn model_output_budget(&self) -> usize;
}
```

### 3.2 Path jail (`path.rs`)

```rust
/// Resolve `candidate` (absolute or relative-to-`cwd`) to a concrete absolute
/// path **guaranteed to live under `workspace_root`**, or reject it.
///
/// Works for paths that do not yet exist (needed by `write`/create-new-file):
/// we lexically normalize, then symlink-check the deepest *existing* ancestor.
///
/// `cwd` itself must already be inside the jail (the loop guarantees this).
impl Host {
    pub async fn resolve_in_jail(&self, cwd: &Path, candidate: &Path)
        -> Result<PathBuf, PathError>;
}

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("path escapes the workspace root: {0}")]
    Escape(String),
    #[error("workspace root is invalid: {0}")]
    InvalidRoot(String),
    #[error("io error resolving path {path}: {source}")]
    Io { path: String, source: std::io::Error },
}
```

### 3.3 Shell exec (`shell.rs`)

```rust
/// A shell command to run through the platform shell. Mirrors Grok's
/// `TerminalRunRequest` (`xai-grok-shell/src/terminal/runner.rs:16`) minus the
/// PTY/streaming/background fields v0 doesn't need.
#[derive(Debug, Clone)]
pub struct ExecRequest {
    pub command: String,          // run as `sh -c <command>` / `bash -lc <command>`
    pub cwd: PathBuf,             // must be jail-resolved by the caller (pack)
    pub timeout: Option<Duration>,// None → limits.default_timeout; clamped to max_timeout
    pub env: Vec<(String, String)>, // extra env on top of inherited
}

/// Result of a completed (or killed) command.
#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    /// stdout followed by stderr (Grok combines this way,
    /// `local_terminal.rs:112-113`); the convenience most tools render.
    pub combined: String,
    pub exit_code: Option<i32>,   // None when killed by signal
    pub timed_out: bool,
    pub cancelled: bool,
    pub truncated: bool,          // any stream hit max_output_bytes
    pub duration: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("failed to spawn shell: {0}")]
    Spawn(String),
    #[error("io error during exec: {0}")]
    Io(String),
}

impl Host {
    /// Never returns `Err` for "the command failed / timed out / was killed" —
    /// those are *successful captures* with `exit_code`/`timed_out`/`cancelled`
    /// set (the pack turns a non-zero exit into whatever it wants). `ExecError`
    /// is only for our own inability to spawn/capture.
    pub async fn exec(&self, req: ExecRequest, cancel: &CancellationToken)
        -> Result<ExecOutput, ExecError>;
}
```

### 3.4 FS helpers (`fs.rs`)

```rust
#[derive(Debug, Clone)]
pub struct FileRead {
    pub contents: String,         // lossy UTF-8 (String::from_utf8_lossy)
    pub stat: FileStat,
}

#[derive(Debug, Clone)]
pub struct FileStat {
    pub len: u64,
    pub modified: Option<std::time::SystemTime>, // freshness token for Task 10 edits
}

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error(transparent)]
    Path(#[from] PathError),      // jail rejection surfaces as an fs error too
    #[error("{op} failed for {path}: {source}")]
    Io { op: &'static str, path: String, source: std::io::Error },
}

impl Host {
    pub async fn read_file(&self, cwd: &Path, path: &Path) -> Result<FileRead, FsError>;
    pub async fn write_file(&self, cwd: &Path, path: &Path, contents: &str)
        -> Result<FileStat, FsError>;         // creates parents? see §4.3
    pub async fn stat(&self, cwd: &Path, path: &Path) -> Result<FileStat, FsError>;
}
```

### 3.5 Truncation (`truncate.rs`)

```rust
/// Default model-facing byte budget. Matches Grok's shell cap
/// (`xai-grok-shell/.../mod.rs:22`). Codex's *retained* cap is 1 MiB
/// (`codex-rs/utils/pty/src/lib.rs:12`) but its model-format budget is per-model
/// and far smaller; 30 KB is the pragmatic middle.
pub const MODEL_OUTPUT_BUDGET: usize = 30_000;

/// Bound `text` to `budget` bytes for model consumption. Head+tail
/// (middle-truncation), UTF-8-safe, with a byte-count marker in the seam.
/// Returns `(text, was_truncated)`. Idempotent below the budget (returns the
/// input untouched, no marker) — signal truncation only when it happened
/// (Grok's rule, `truncate.rs:123`).
pub fn truncate_for_model(text: &str, budget: usize) -> (String, bool);
```

### 3.6 Error mapping to `ToolError::Respond` (pack boundary, not this crate)

`locode-host` cannot depend on `locode-tools`, so the soft-error mapping lives in
the pack. The convention (documented here, implemented in Task 9+):

```rust
// in locode-packs (depends on host + tools):
host.resolve_in_jail(&ctx.cwd, &args.path).await
    .map_err(|e| ToolError::Respond(e.to_string()))?;   // jail escape → soft
host.read_file(&ctx.cwd, &path).await
    .map_err(|e| ToolError::Respond(e.to_string()))?;   // not-found etc. → soft
```

All host errors are **soft by default** (ADR-0004 `error.rs:6-9`). The only
`Fatal` cases belong to the loop/registry, not the host. Host `thiserror` messages
are written to read well as a model-facing `tool_result{is_error}` string.

---

## 4. Behavior & algorithms (with edge cases)

### 4.1 Path jail — resolve + reject

The hard requirement (`SPEC.md:132`, Task 7 acceptance): reject `..` escapes
**and** absolute escapes **and** symlink escapes, while still allowing paths that
do not yet exist (create-new-file).

Two candidate primitives, and why neither alone is enough:

- **Lexical normalization** — walk `Path::components()`, pop on `ParentDir`, skip
  `CurDir`, no filesystem access. This is exactly Codex's jail check
  (`codex-rs/core/src/safety.rs:149-163`) followed by a `starts_with(root)` prefix
  test (`safety.rs:168-180`). Pro: works for non-existent paths. Con: a *symlink
  inside* the workspace pointing outward is **not** caught — lexically the path is
  still under root.
- **Canonicalization** — `fs::canonicalize` resolves symlinks (Grok's
  `canonicalize_with_timeout`, `xai-file-utils/... wait` →
  `xai-grok-tools/src/util/fs.rs:19-42`). Pro: catches symlink escapes. Con: errors
  on a path whose leaf doesn't exist yet, so it can't validate a `write` target.

**Chosen algorithm (hybrid — catches all three escape classes, allows new files):**

1. Make `candidate` absolute: if relative, join onto `cwd`. (`cwd` is already
   inside the jail — the loop sets `ToolCtx.cwd`/`workspace_root` from session
   config.)
2. **Lexical normalize** the absolute path (Codex `normalize`): resolve `.`/`..`
   syntactically. Reject early if the normalized path does not `starts_with`
   `self.workspace_root` → `PathError::Escape`. (Catches `../etc/passwd`,
   `/etc/passwd`, `a/../../b`.)
3. **Symlink-check the deepest existing ancestor.** Walk from the normalized path
   upward to the first ancestor that exists; `canonicalize` *that* ancestor
   (symlink-resolving) and confirm the canonical ancestor still
   `starts_with(self.workspace_root)` (the root is already canonical from
   `Host::new`). Re-append the non-existent tail. This catches a symlink that
   sits on an existing path component and points out of the jail, **and** still
   returns a usable path when the leaf file doesn't exist yet.
4. Return the resolved absolute `PathBuf`.

Edge cases:
- `../etc/passwd`, `/etc/passwd`, `foo/../../bar` → rejected at step 2.
- `symlink_to_tmp/evil.txt` where `symlink_to_tmp -> /tmp` lives in the workspace →
  rejected at step 3 (canonical ancestor `/tmp` not under root).
- `newdir/newfile.txt` (nothing exists yet) → step 2 passes, step 3 canonicalizes
  the workspace root (deepest existing ancestor), tail re-appended → allowed.
- Root itself / `.` → resolves to `workspace_root`, allowed.
- Wrap the ancestor `canonicalize` in a **timeout** (Grok guards hung
  overlayfs/`stat`, `fs.rs:12` `FS_SYSCALL_TIMEOUT = 30s`, `fs.rs:19-41`). Cheap
  robustness; recommended.

### 4.2 Shell exec — capture, timeout, kill, cancel

Baseline modeled on Grok's `LocalTerminalRunner`
(`xai-grok-shell/src/terminal/local_terminal.rs`). Flow:

1. **Build the command.** Unix: `Command::new(shell); cmd.arg("-c").arg(&command)`.
   Grok uses `bash -lc` via a resolved bash path (`local_terminal.rs:52-56`,
   `mod.rs:29-38`). **Decision (D5):** use `sh -c` by default (POSIX, always
   present, no login-profile surprises); make the shell path a `HostConfig`/env
   knob for callers who want `bash -lc`. `-l` (login) runs profile files — Grok
   wants that for user parity; a headless core is better off *not* sourcing
   arbitrary rc files by default. Document the tradeoff.
2. **Stdio:** `stdin(Stdio::null())`, `stdout(piped())`, `stderr(piped())`
   (`local_terminal.rs:64-69`). Null stdin so a command that reads stdin returns
   immediately instead of hanging (matches Grok).
3. **New process group:** `cmd.process_group(0)` (Unix) so the child leads its own
   group and we can kill the *whole tree* on timeout, not just the shell
   (`xai-tty-utils/src/lib.rs:147-157`; `new_process_group`). This is a **safe**
   `std::os::unix::process::CommandExt` method — no `unsafe` needed (see D3).
4. **Spawn**, take `stdout`/`stderr`. Spawn a **bounded** reader task per stream
   (§4.2a). Grok reads to end unbounded then trims (`local_terminal.rs:88-89`,
   `read_stream`); we bound during read to survive a runaway `yes` flood.
5. **Race** `child.wait()` against the timeout and the cancel token in a
   `tokio::select!` (Grok's streaming runner uses a `select!` with a pinned
   `sleep`, `streaming_local_terminal.rs:552-557,713`; the simple runner uses
   `tokio::time::timeout`, `local_terminal.rs:93`). We add the cancel arm:
   ```
   tokio::select! {
       status = child.wait()        => normal,
       _ = sleep(effective_timeout) => timed_out = true,  kill_group(),
       _ = cancel.cancelled()       => cancelled  = true, kill_group(),
   }
   ```
6. **kill_group()** = SIGTERM the group → wait up to `kill_grace` for exit →
   SIGKILL the group if still alive → reap (`child.wait()`), all bounded by a
   short reap timeout. See §4.2b. Grok's bash documents exactly this: "kills the
   child process group (SIGTERM, escalated to SIGKILL after a ~1s grace period)"
   (`bash/mod.rs:1429`).
7. **Join** the reader tasks (they finish when the pipes close post-kill), assemble
   `ExecOutput`: `combined = stdout + stderr`, set flags. Codex prepends a
   `command timed out after {ms}` line on timeout (`tools/mod.rs:116-126`); we can
   let the *pack* do that shaping, or do it in `combined` — recommend the pack owns
   presentation, host just reports `timed_out`.

#### 4.2a Bounded capture (memory-safe output cap)

Reading `read_to_end` (Grok, `local_terminal.rs:16-18`) is unbounded — a `yes`
flood OOMs us before the trim at `local_terminal.rs:114`. Codex reads in 8 KiB
chunks (`READ_CHUNK_SIZE`, `exec.rs:69`) and caps retained bytes
(`retained_bytes_cap`, `exec.rs:270-275`; truncate at `exec.rs:735-744`).

**Chosen:** each reader loops `read(&mut [0; 8192])` and retains at most
`max_output_bytes` per stream using a **tail-retention ring** (keep the *last* N
bytes, drop oldest) — this is Grok's `truncate_buffer` semantics
(`local_terminal.rs:25-45`: keep last `limit`, drop oldest, UTF-8-safe boundary)
but applied *during* read so peak memory is O(cap), not O(total). Track a
`truncated` flag when we drop bytes. Rationale for tail-over-head: for a shell, the
error/exit summary is at the end — Grok keeps the tail deliberately. When truncated,
prepend a marker line, e.g. `[... {n} earlier bytes truncated ...]`.

> Alternative considered: head-retention (`.take(cap)`) is simpler and naturally
> memory-bounded but discards the tail (the errors). Rejected for shell; matches
> neither Grok (tail) nor Codex (middle). Middle-retention (head+tail) is best for
> readability but needs two buffers; deferred — tail-retention is the v0 choice,
> and the shared `truncate_for_model` (middle) still runs centrally on top.

#### 4.2b Process-group kill without `unsafe`

- New group at spawn: `cmd.process_group(0)` — **safe**.
- Kill the group: `nix::sys::signal::killpg(Pid::from_raw(pgid), SIGTERM|SIGKILL)`.
  `pgid` == child pid (the child is its own group leader because of
  `process_group(0)`). `nix::killpg` is a **safe** wrapper (the `unsafe` is inside
  the `nix` crate, which our `forbid(unsafe_code)` does not police). This is
  exactly the primitive Grok validates and calls (`xai-tty-utils/src/lib.rs:344-353`,
  `terminate`=SIGTERM `:321-330`, `kill`=SIGKILL `:332-341`) and Codex calls via
  raw `libc::killpg` (`codex-rs/utils/pty/src/process_group.rs:90-103,121-124`).
  We take Grok's `nix` route precisely to stay `unsafe`-free.
- **Guard the pgid** before signaling: refuse `0`, `1`, and our own process group —
  a degenerate pgid turns a scoped kill into a broadcast (Grok's `ProcessGroupId`
  validation, `lib.rs:181-204`). A child from `process_group(0)` always has
  `pid > 1` distinct from our group, so a well-formed spawn never trips it; we
  still assert, cheaply, safe-by-construction.
- We deliberately **skip `setsid`/TTY-detach** (Grok's `detach_from_tty`,
  `lib.rs:65-78`, needs `pre_exec` = `unsafe`). TTY detach exists to protect a
  live TUI from child escape codes; a headless core has no TUI to corrupt, so we
  don't need it and thus avoid the only `unsafe` in Grok's spawn path.
- **Non-Unix fallback:** `child.start_kill()` (direct child only; `tokio`), best
  effort. v0 targets Unix; documented.

#### 4.2c Cancellation edge case

`ctx.cancel` (`crates/locode-tools/src/ctx.rs:24`) fires when the loop aborts a
turn / mid-batch. The `select!` cancel arm kills the group identically to a
timeout, sets `cancelled = true`, and returns promptly — no orphaned subprocess.
This satisfies the Task 7 "cancellation via the token" requirement and the
ADR-0004 mid-batch-abort invariant that every `tool_use` still gets one result
(the pack turns a cancelled exec into a soft result).

### 4.3 FS helpers

- Every helper calls `resolve_in_jail(cwd, path)` **first**; a jail rejection is a
  soft `FsError::Path`.
- `read_file`: `tokio::fs::read` → `String::from_utf8_lossy` (binary files degrade
  rather than error; Grok reads lossy too) + `stat` for the freshness token.
- `write_file`: create-or-overwrite via `tokio::fs::write`. **Decision (D6):** do
  **not** auto-create parent directories by default (a mistyped nested path
  silently creating dirs is a footgun) — return a clear error; revisit if the grok
  `write` port needs it. Update `FileStat.modified` after write for freshness.
- `stat`: `metadata().modified()` → the mtime the Task 10 edit invariants compare
  against (read-before-edit freshness re-check).
- Wrap `canonicalize`/`metadata` in the 30s syscall timeout (Grok `fs.rs:12,19-41`)
  to survive hung network/overlay filesystems.

### 4.4 `truncate_for_model`

One shared post-process (ADR-0008 `:13`). **Chosen strategy: middle-truncation**
(keep head + tail, elide the middle) with a UTF-8-safe seam marker — Codex's model
formatter (`truncate_middle_chars`, `codex-rs/utils/string/src/truncate.rs:6-7`;
split budget in half `:127-128`; marker `…{N} chars truncated…` `:131-135`;
assemble head+marker+tail `:147-150`). Codex also prepends a
`Warning: truncated output (original token count …)\nTotal output lines: …` header
(`utils/output-truncation/src/lib.rs:12-23`) — we keep the marker minimal for v0
and let packs add headers if wanted.

Why middle over Grok's per-tool zoo (`truncate.rs` has head+footer
`truncate_with_preview:100-116`, head+tail `truncate_front_and_back:234-261`,
tail-keep in the shell buffer): ADR-0008 mandates **one** shared post-process, and
middle-truncation is the least-bad universal default — it preserves the start
(headers/summaries, important for reads) *and* the end (errors/results, important
for shell). UTF-8 safety via floor/ceil char boundaries (Grok's polyfills
`truncate.rs:160-184`; Rust ≥1.97 here has `floor_char_boundary` stable, so we can
use std). Idempotent below budget: return input, `false`.

### 4.5 Where the shared truncation is *applied* (integration note — flag for Task 6/9)

This is the one genuinely load-bearing wiring decision and it crosses crate
boundaries, so it is called out explicitly rather than silently assumed:

- `Registry::dispatch` (`crates/locode-tools/src/registry.rs:194`) already builds
  the `tool_result` `ContentBlock` with `prompt_text`. But `locode-tools` **cannot
  depend on `locode-host`** (siblings), so truncation cannot happen *inside*
  dispatch.
- Therefore the **engine** (`locode-engine`, which depends on both) applies
  `truncate_for_model` centrally: after `dispatch` returns, before appending the
  `tool_result` to history, walk its `ResultChunk::Text` chunks
  (`crates/locode-protocol/src/lib.rs:107`) and truncate each to the host budget.
  This is the faithful embodiment of "shared post-process applied before the model
  re-enters, not per-tool ad hoc" (`ADR-0008:13`).
- **Task 7 delivers the pure function + budget only.** The central application is
  Task 6 (engine) wiring; noting it here so the seam is intentional and tested end
  to end at Checkpoint C. (Open question OQ-1 revisits whether packs *also* do
  semantic shaping like read_file line-capping — they may; the central cap is the
  safety net, not the only shaping.)

---

## 5. Design decisions (each: harness `file:line`, why, why-not-alternative, differences)

**D1 — Path jail = lexical normalize + canonical-ancestor symlink check (hybrid).**
- Source: Codex lexical `normalize` + `starts_with` prefix
  (`codex-rs/core/src/safety.rs:149-163,168-180`); Grok symlink-resolving
  `canonicalize_with_timeout` (`xai-grok-tools/src/util/fs.rs:19-42`).
- Why: Codex-lexical alone misses symlink escapes; Grok-canonical alone can't
  validate not-yet-existing `write` targets. The hybrid gets both.
- Why not either alone: see §4.1. Why not `cap-std`/`openat`-style true jail: heavier
  dep + API churn for v0's trusted-workspace threat model; the hybrid is sufficient
  and dependency-light.
- Difference across harnesses: **Grok does not strictly jail** — it is a trusted-
  workspace agent that accepts absolute paths (its `read_file`/`bash` resolve and
  canonicalize but do not reject out-of-root). **Codex** jails writes to
  `writable_roots` via the lexical check. **We are stricter than Grok** by ADR-0008
  (reject `..`/absolute/symlink escape) — a deliberate divergence from the primary
  model, justified by our headless "no human to approve" posture (`ADR-0008:10`).

**D2 — Shell = `sh -c` fire-and-forget capture, no PTY.**
- Source: Grok `LocalTerminalRunner` (`local_terminal.rs:47-125`); the PTY /
  streaming / background variants (`pty_session.rs`, `streaming_local_terminal.rs`,
  `background_task.rs`) exist but are out of v0 scope.
- Why: v0 needs stdout/stderr/exit capture with limits, nothing interactive
  (SPEC assumption 4-6). `sh -c` is the minimal, portable form.
- Why not `bash -lc` (Grok default, `local_terminal.rs:54`): `-l` sources login
  profiles → nondeterministic env in a headless core. Make it a knob (D5).
- Why not PTY: needed only for interactive TUIs and color/progress fidelity
  (Grok's `color_env`, `mod.rs:128-152`); irrelevant headless, big complexity.
- Difference: Grok has three runners routed by a `stream` flag
  (`mod.rs:216-230`); we ship one.

**D3 — Process-group kill via `process_group(0)` + `nix::killpg`, zero `unsafe`.**
- Source: Grok `new_process_group` (`xai-tty-utils/src/lib.rs:147-157`) +
  `killpg` (`:344-353`); Codex `libc::setpgid`/`killpg`
  (`codex-rs/utils/pty/src/process_group.rs:71-72,90-103`).
- Why: killing only the direct shell leaves grandchildren (the actual `cargo`,
  `node`, `sleep`) orphaned and still consuming the timeout budget. Group-kill
  reaps the tree (Grok's tree-kill test, `lib.rs:853-905`).
- Why this exact form: `Command::process_group` and `nix::killpg` are **safe**
  APIs, so we satisfy `unsafe_code = "forbid"` (`Cargo.toml`) — unlike `pre_exec`
  (`setsid`), which is `unsafe` and which we drop because a headless core needs no
  TTY detach (§4.2b). Codex's raw `libc` route *would* force `unsafe` in our crate;
  `nix` keeps it in the dependency.
- Why not `child.kill()`/`start_kill()` alone (Grok's *simple* runner does this,
  `local_terminal.rs:99`): it signals only the direct child, not the group — a
  `bash -c 'sleep 300 & wait'` leaks the `sleep`. Grok's simple runner accepts that
  (it's used for trusted git helpers); the *agent-facing* bash path uses the group
  runner, and so must we.
- Difference: Grok wraps the pgid in a validated `ProcessGroupId`
  (`lib.rs:159-210`); we replicate the 0/1/own-group guard inline.

**D4 — SIGTERM → grace → SIGKILL escalation.**
- Source: Grok bash prompt spec (`.../bash/mod.rs:1429`, "SIGTERM, escalated to
  SIGKILL after a ~1s grace"); Codex bounded IO-drain 2s (`exec.rs:89`).
- Why: SIGTERM lets a well-behaved child flush/cleanup; SIGKILL guarantees
  teardown. A bare SIGKILL can strand temp files / child state.
- Why not SIGKILL-only (Codex `kill_process_group_by_pid` sends SIGKILL directly,
  `process_group.rs:103`): fine for Codex's harder sandbox posture; we prefer
  Grok's gentler escalation for a dev workspace. `kill_grace` is configurable.

**D5 — `sh -c` default, shell path configurable.** (see D2 rationale.)

**D6 — `write_file` does not auto-create parents.** Footgun avoidance; revisit for
the grok `write` port (Task 10) if grok's tool creates parents.

**D7 — Output cap: tail-retention ring during read (30 KB).**
- Source: Grok `truncate_buffer` keep-last (`local_terminal.rs:25-45`), cap 30_000
  (`mod.rs:22`); Codex chunked read 8 KiB + retained-bytes cap
  (`exec.rs:69,270-275,735-744`, retained 1 MiB `utils/pty/src/lib.rs:12`).
- Why bound *during* read: unbounded `read_to_end` (Grok simple runner) OOMs on a
  flood; bounding during read is Codex's robustness win. Why tail: shell errors live
  at the end (Grok). Why 30 KB not 1 MiB: 30 KB is the model-facing budget; Codex's
  1 MiB is a *retained* cap it then re-truncates for the model. We collapse the two.
- Difference: Codex keeps middle for the model; Grok keeps tail for shell. We keep
  tail at the capture layer and let the central `truncate_for_model` (middle) run on
  top — belt and suspenders.

**D8 — `truncate_for_model` = middle-truncation, one shared fn.**
- Source: Codex `truncate_middle_chars` + `format_truncation_marker`
  (`utils/string/src/truncate.rs:6-7,127-135,147-150`), applied via
  `formatted_truncate_text` (`utils/output-truncation/src/lib.rs:12-30`).
- Why one shared fn: ADR-0008 `:13`. Why middle: preserves both ends (see §4.4).
- Why not Grok's per-tool strategies: ADR forbids per-tool ad hoc; middle is the
  universal least-bad default. Difference: Grok picks a strategy per tool
  (`truncate.rs`); we pick one for all.

**D9 — Concrete `Host` struct, not a trait, for v0.**
- Why: injectability here means "construct with config + hand to pack tools," not
  "swap a mock." Pack-tool tests run against a real temp workspace (Grok's own tests
  do, e.g. `fs.rs:210+`, `local_terminal.rs:128+`). A trait adds coherence/erasure
  overhead with no v0 payoff.
- The deferred OS-sandbox seam (`ADR-0008:19-26`) becomes an alternative `Host`
  construction/impl later; the call surface (`exec`, `resolve_in_jail`) is the seam.
  Trait-ify then if a second impl actually lands. (SPEC: "seams … reserved slots.")

---

## 6. Tests (per Task 7 verification + edge cases in §4)

Inline `#[cfg(test)]`, `tokio` dev-dep already available pattern
(`crates/locode-tools/Cargo.toml [dev-dependencies]`). Use `tempfile` for real
workspace trees (Grok precedent: `fs.rs`, `local_terminal.rs` tests).

Path jail (`path.rs`):
- `jail_rejects_parent_escape` — `resolve_in_jail(root, "../etc/passwd")` → `Escape`.
- `jail_rejects_absolute_escape` — `"/etc/passwd"` → `Escape`.
- `jail_rejects_sneaky_dotdot` — `"a/../../b"` → `Escape`.
- `jail_allows_nonexistent_leaf` — `"newdir/new.txt"` under root → `Ok`.
- `jail_rejects_symlink_escape` — create `root/link -> /tmp`, `resolve(root,
  "link/x")` → `Escape` (the symlink-ancestor canonical check; the headline
  edge case in the mandate).
- `jail_allows_normal_path` / resolves `.` to root.
- `relative_resolves_against_cwd` — cwd = `root/sub`, `"f.txt"` → `root/sub/f.txt`.

Shell (`shell.rs`):
- `exec_captures_stdout_and_exit` — `echo hello` → stdout "hello", exit 0.
- `exec_captures_stderr` — `>&2 echo boom` → stderr "boom".
- `exec_nonzero_exit_is_capture_not_error` — `exit 3` → `Ok(exit_code=Some(3))`.
- `timeout_kills_sleeper` — `sleep 30`, timeout 200ms → `timed_out=true`, returns
  well under the sleep; assert child reaped. (Task 7 headline test.)
- `timeout_kills_process_group_tree` — `sleep 30 & wait` (grandchild) times out and
  the grandchild is reaped too (mirrors Grok's tree-kill assertion,
  `xai-tty-utils/src/lib.rs:853-905`; poll that the pid is gone).
- `cancellation_kills_running_command` — spawn `sleep 30`, cancel the token after
  25ms → `cancelled=true`, prompt return. (Task 7 cancellation requirement.)
- `output_over_cap_is_truncated_with_marker` — `yes | head -c 200000` (or a
  Rust-side flood) with `max_output_bytes=1000` → `truncated=true`, output ≤ cap+
  marker, marker present. (Task 7 headline test.)
- `null_stdin_does_not_hang` — `cat` (reads stdin) returns promptly at EOF.

FS (`fs.rs`):
- `read_write_roundtrip_in_jail`; `write_updates_mtime`; `read_outside_jail_soft`
  (jail rejection surfaces as `FsError::Path`); `stat_returns_mtime`.

Truncate (`truncate.rs`):
- `below_budget_untouched_no_marker`; `over_budget_keeps_head_and_tail_with_marker`;
  `utf8_safe_no_panic_on_multibyte_seam` (emoji/CJK at the cut); `marker_reports_count`.

---

## 7. Dependencies to add (with justification + precedent)

Add to `crates/locode-host/Cargo.toml` (and pin versions in the root
`[workspace.dependencies]` per the "Ask first: adding a dependency" boundary —
**flag for user approval**, `AGENTS.md` Boundaries):

- **`tokio`** (workspace dep exists) with features `["process", "io-util", "time",
  "rt", "sync", "macros"]`. Precedent: both Grok (`tokio::process::Command`,
  `local_terminal.rs:5`) and Codex spawn via tokio. `process` = subprocess,
  `io-util` = `AsyncReadExt`, `time` = `timeout`/`sleep`, `sync` = re-export path
  for `CancellationToken` interop.
- **`nix`** with features `["signal"]` (Unix-only via `[target.'cfg(unix)']`).
  Purpose: `killpg` and pid guards **without writing `unsafe`** (Decision D3).
  Precedent: Grok uses `nix` for exactly this (`xai-tty-utils/src/lib.rs:66-67,
  351`). Alternative `libc` (Codex, `process_group.rs`) would force `unsafe` blocks
  our lints forbid — `nix` is the reason we stay `unsafe`-free.
- **`thiserror`** (workspace dep exists) — `PathError`/`ExecError`/`FsError`
  (matches the crate's existing error style, `locode-tools/src/error.rs`).
- **`tokio-util`** (workspace dep exists) — `CancellationToken` type used in the
  `exec` signature (same type as `ToolCtx.cancel`, `ctx.rs:5`).
- **dev-dep `tempfile`** — real temp workspace trees for jail/fs/shell tests.
  Precedent: Grok tests (`fs.rs:212`, `local_terminal.rs` tests). Dev-only, no
  shipped-surface impact.

Considered, **not** adding for v0:
- **`dunce`** (Grok uses `dunce::simplified` to avoid Windows `\\?\` verbatim paths,
  `fs.rs:22`). Windows-only nicety; v0 is Unix. Note it as the Windows follow-up.
- **`libc`** — avoided (would require `unsafe`; `nix` covers our needs safely).
- **`cap-std`/`openat`** — a heavier "true" jail; unjustified for the trusted-
  workspace threat model (ADR-0008). Revisit if the OS-sandbox seam lands.

Subprocess-with-timeout precedent summary: **both** primary refs use
`tokio::process::Command` + `tokio::time` (Grok `local_terminal.rs:6,93`; Codex
`exec.rs` select/sleep `:186,200`), not an external "timeout" crate — we follow
that. Path canonicalization: `std::fs::canonicalize` (Grok wraps it with a timeout;
Codex normalizes lexically) — no crate needed.

---

## 8. Open questions

- **OQ-1 (truncation ownership).** Confirm the split: host provides the pure
  `truncate_for_model` + budget; the **engine** applies it centrally post-dispatch
  (§4.5). Do packs *also* do semantic shaping (e.g., `read_file` line-limiting,
  Grok `truncate_with_preview` with a `read_file` footer hint,
  `truncate.rs:100-116`)? Proposal: yes — packs shape, the central cap is the safety
  net. Needs a one-line confirmation before Task 6/9 wire it.
- **OQ-2 (shell: `sh -c` vs `bash -lc`; login profiles).** Default to `sh -c`
  (deterministic) with a configurable shell path? Grok defaults `bash -lc`
  (`local_terminal.rs:54`). Confirm the default for the grok *pack* port (Task 9) —
  faithful-to-grok would argue `bash -lc`, determinism argues `sh -c`. (Latest-
  instruction / user call.)
- **OQ-3 (default timeout).** 10s (Grok/Codex) is short for `cargo build`/tests.
  Host `default_timeout` is only the fallback when a call passes none; the grok bash
  tool carries its own timeout arg. Confirm 10s default + 600s ceiling, or a higher
  headless default.
- **OQ-4 (output cap retention: tail vs middle at the capture layer).** §4.2a picks
  tail (Grok); the central `truncate_for_model` is middle (Codex). Is running both
  acceptable, or should capture also be middle to keep the head? Proposal: tail at
  capture is fine (belt-and-suspenders).
- **OQ-5 (`Host` shared vs per-call).** `Host` holds `workspace_root`; `ToolCtx`
  also carries `workspace_root`/`cwd` (`ctx.rs:20-24`). Treat `Host.workspace_root`
  as the jail authority and require the loop to set `ToolCtx.workspace_root` to the
  same value? (They must agree; the host is authoritative.) Confirm.
- **OQ-6 (`unsafe` boundary).** Confirmed we can avoid `unsafe` entirely via
  `process_group(0)` + `nix::killpg` (D3). If a future need forces `setsid`/`pre_exec`
  it would require a scoped `#![allow(unsafe_code)]` — which the workspace `forbid`
  blocks and which would need an ADR. v0 stays clean; flagging so it stays a
  conscious line.
- **OQ-7 (dependency approval).** Adding `nix` + dev-dep `tempfile` trips the
  "Ask first: adding a dependency" boundary (`AGENTS.md`). Needs explicit user OK
  before Task 7 implementation.
