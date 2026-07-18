# Task 9 — grok pack: `run_terminal_cmd` + `read_file`

> Faithful port of Grok Build's terminal + read tools onto our `Tool` trait, over
> `locode-host`. Behavior P0, exact names/descriptions P1 (SPEC §Success Criteria 2).

**Grok source root (all `gb/…` citations below are relative to it):**
`coding-cli-survey/submodules/grok-build/crates/codegen/xai-grok-tools/src/implementations/grok_build/`

**locode target files:** `crates/locode-packs/src/grok/{terminal.rs, read.rs, mod.rs}`,
plus a freshness-store addition in `crates/locode-host` (see §2).

---

## 1. Purpose & scope

Port two grok tools:

1. **`run_terminal_cmd`** — grok's Bash tool (`gb/bash/mod.rs:1581` → `ToolId::new("run_terminal_cmd")`).
   Runs a shell command, captures stdout+stderr+exit, hard timeout, byte-capped output
   with a truncation marker. Foreground-only in v0.
2. **`read_file`** — grok's ReadFile tool (`gb/read_file/mod.rs`). Reads a text file,
   returns a `LINE_NUMBER→CONTENT` body, line/token truncation, and **records
   `(path, mtime)` freshness** so a later `search_replace`/`write` can enforce the
   read-before-edit + mtime-freshness invariants (Task 10).

**In scope (v0):** text files; foreground shell; grok's real arg names/schema; grok's
output shaping and soft-error taxonomy mapped onto `ToolError::Respond`; freshness
recording on read.

**Out of scope (deferred seams, flagged):** background execution (`is_background`,
task_id, kill_task, monitor), streaming deltas (ADR-0014 handles the loop side; the
tool returns a buffered result), PDF/PPTX/IPYNB/image multimodal reads
(`gb/read_file/mod.rs:79-101,109-110`), auto-background-on-timeout, `cmd_prefix`,
gitignore filtering on read, cursor-rules-on-read, unicode-filename resolution.

**Why these two together:** they are the read half of the edit slice. `read_file` must
land its freshness hook *before* Task 10 can enforce invariants 1 & 3, so Task 9 owns
the freshness-store plumbing even though only Task 10 consumes it.

---

## 2. Module layout

```
crates/locode-packs/src/grok/
├── mod.rs        # pub mod terminal; pub mod read; (wiring lives in Task 8 pack)
├── terminal.rs   # GrokRunTerminalCmd { host: Arc<Host> }
└── read.rs       # GrokReadFile { host: Arc<Host> }
```

**How tools reach the host.** `ToolCtx` is deliberately tiny (`cwd, call_id,
workspace_root, cancel` — `crates/locode-tools/src/ctx.rs`) and does **not** carry the
host. So each grok tool **holds `Arc<Host>`**, injected by the pack builder (Task 8).
This is the seam that keeps "tools never touch `std::fs`/`Command` directly" (AGENTS.md
Boundaries; ADR-0008) true: the tool body only calls host methods.

**Freshness store — a host-owned concern (Task 7/9 addition).** Grok itself has **no
runtime freshness/mtime tracking** — verified: `file_snapshot_at_edit` is always `None`
(`gb/search_replace/mod.rs:651,674`) and a repo-wide grep for `mtime|snapshot|freshness`
finds nothing in the edit path. Freshness is a **locode addition** borrowed from Claude
Code (design doc `06-design-lessons/minimal-headless-rust-agent.md:233,258-260`). It must
be shared state visible across tool instances, so it lives in the `Host`:

```rust
// crates/locode-host/src/freshness.rs  (add in Task 7 or here)
#[derive(Clone, Default)]
pub struct Freshness(Arc<Mutex<HashMap<PathBuf, SystemTime>>>);
impl Freshness {
    pub fn record(&self, path: PathBuf, mtime: SystemTime);
    pub fn get(&self, path: &Path) -> Option<SystemTime>;
    pub fn forget(&self, path: &Path);            // optional
}
// Host exposes: fn freshness(&self) -> &Freshness;
```

Keyed by the **canonicalized jailed absolute path** (so `./a`, `a`, and an absolute
form collapse to one key). Ephemeral, per-session, in-memory (matches SPEC Assumption 6).

---

## 3. Key types & signatures

### 3.1 `run_terminal_cmd`

Grok's input (`gb/bash/mod.rs:250-289`): `command`, `timeout` (ms), `description`,
`is_background`. We keep `command`, `timeout`, `description`; drop `is_background` in v0
(document as a reserved seam — dropping an arg is faithful-behavior-P0 since background
is a whole subsystem).

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunTerminalCmdArgs {
    /// "The bash command to run." (gb/bash/mod.rs:252)
    pub command: String,
    /// "Optional timeout in milliseconds (max 300000). Default: 120000 (2 minutes)."
    /// (gb/bash/mod.rs:260-261) — grok clamps to max_timeout; we clamp to 300_000.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// "One sentence explanation as to why this command needs to be run…"
    /// (gb/bash/mod.rs:273-277) — required in grok; kept for schema fidelity, unused at runtime.
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct RunTerminalCmdOutput {
    pub exit_code: i64,       // -1 sentinel when killed (grok: gb/bash/mod.rs:439 unwrap_or(-1))
    pub output: String,       // combined, already byte-capped by the host
    pub truncated: bool,
    pub total_bytes: usize,   // pre-truncation size, for the marker
}
impl ToolOutput for RunTerminalCmdOutput {
    fn to_prompt_text(&self) -> String { /* grok header format, see §4.1 */ }
}

#[async_trait]
impl Tool for GrokRunTerminalCmd {
    type Args = RunTerminalCmdArgs;
    type Output = RunTerminalCmdOutput;
    fn kind(&self) -> ToolKind { ToolKind::Shell }
    fn description(&self) -> &str { /* grok's rendered template, §5 */ }
    async fn run(&self, ctx, args) -> Result<Self::Output, ToolError> { … }
}
```

Registered under wire name **`run_terminal_cmd`** (grok's real `ToolId`,
`gb/bash/mod.rs:1581`). See §8 open question re: SPEC's `run_terminal_command`.

### 3.2 `read_file`

Grok's input (`gb/read_file/mod.rs:111-144`): `target_file` (serde-renamed from `path`),
`offset: Option<i64>`, `limit: Option<usize>`, `pages`, `format`. We keep
`target_file/offset/limit`; drop `pages`/`format` (PDF-only, out of scope).

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    /// serde(rename="target_file"); "The path of the file to read…absolute preserved as is."
    /// (gb/read_file/mod.rs:113-116)
    #[serde(rename = "target_file")]
    pub path: String,
    /// "The line number to start reading from." (gb/read_file/mod.rs:123-127) — 1-indexed.
    #[serde(default)]
    pub offset: Option<i64>,
    /// "The number of lines to read." (gb/read_file/mod.rs:129-133)
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ReadFileOutput {
    pub path: String,        // jailed absolute path (structured face)
    pub lines: usize,        // total_lines in the file (gb/read_file/mod.rs:442,539)
    pub truncated: bool,     // limit/line-cap applied
    #[serde(skip)]
    body: String,            // the LINE_NUMBER→CONTENT projection (prompt face only)
}
impl ToolOutput for ReadFileOutput {
    fn to_prompt_text(&self) -> String { self.body.clone() }  // dual output, ADR-0003
}
```

`kind()` = `ToolKind::Read`. The **structured `{path, lines, truncated}`** goes to the
report's `tool_calls[]`; the **file body** is the prompt text — exactly the dual-face
example ADR-0003 §Alternatives calls out.

---

## 4. Behavior / invariants, step by step

### 4.1 `run_terminal_cmd`

1. Resolve effective timeout: `min(args.timeout.unwrap_or(120_000), 300_000)` ms
   (grok default 120000, max 300000 — `gb/bash/mod.rs:261`, `BashParams::timeout_secs`
   default None→120s `gb/bash/mod.rs:137,209`).
2. Call `host.run_shell(&ctx, command, timeout, byte_cap)`. The host (Task 7) runs
   `bash -lc <command>` (or `sh -c`), merges stdout+stderr, enforces the hard timeout
   (SIGTERM→SIGKILL, grok `gb/bash/mod.rs:1445`), and byte-caps at
   `DEFAULT_TOOL_OUTPUT_CHARS = 20_000` (`grok-build/.../xai-grok-tools/src/lib.rs:11`,
   `BashParams::output_byte_limit` None→20k `gb/bash/mod.rs:156`). The host returns
   `{exit_code, output, truncated, total_bytes}`.
3. **Honor `ctx.cancel`**: cooperative cancellation kills the child (host's job).
4. `to_prompt_text()` reproduces grok's header (`gb/bash/mod.rs:408-439`):
   - normal: `exit: {code}{annotations}\n{output}`
   - killed: `exit: killed ({reason}){annotations}\n{output}`
   - truncation marker (grok `gb/bash/mod.rs:387-396,421-438`), mid-output first/last form:
     ` [truncated: showing first/last {shown} of {total}]` (drop grok's
     "full output at: {path}" tail — we don't persist an output file in v0).
5. **Errors are soft.** A non-zero exit is **not** a `ToolError` — it's a normal
   `Output` with `exit_code != 0` (the model reads it and reacts). Only a spawn failure
   / host-jail violation maps to `ToolError::Respond`. Never `Fatal` (a failed command
   is recoverable — ADR-0004, `crates/locode-tools/src/error.rs`).

Note grok truncates **in the middle** (keeps head+tail) rather than head-only; the host's
`truncate_for_model` (ADR-0008 shared post-process) should do the same to stay faithful.

### 4.2 `read_file`

1. Jail-resolve `target_file` under `workspace_root` via `host.resolve_path` (`..`/absolute
   escapes → `ToolError::Respond`, ADR-0008). Absolute paths inside the jail are allowed
   ("preserved as is", `gb/read_file/mod.rs:115`).
2. `host.read_file(path)` → bytes. Map io errors to soft `Respond` mirroring grok's
   variants (`gb/read_file/mod.rs:363-379`): NotFound → "file not found" message;
   IsADirectory → "…is a directory"; PermissionDenied → "permission denied".
3. `total_lines = content.matches('\n').count() + 1` (`gb/read_file/mod.rs:442`).
4. Line projection: `LINE_NUMBER→LINE_CONTENT`, 1-indexed (`gb/read_file/mod.rs:108`,
   `extract_file_content_lines`). Default line cap `MAX_LINES_READ = 1000`
   (`gb/read_file/mod.rs:56`); effective limit = `min(limit.unwrap_or(MAX), 1000)`
   (`gb/read_file/mod.rs:449-455`). Resolve `offset` (1-indexed; negative = from end,
   `gb/read_file/mod.rs:156-169`) — v0 may support positive offset only and flag negatives.
5. Token cap: if `estimate_tokens(projection) > MAX_NUM_TOKENS (25_000)`
   (`gb/read_file/mod.rs:55,464`), return grok's `FileTooLarge` guidance text as a
   **successful** `Output` whose body is the guidance (grok returns it as tool output,
   not an error — `gb/read_file/mod.rs:510`) OR as `Respond`; recommend `Respond` so it
   surfaces as `is_error` and nudges a narrower read. Flag in §8.
6. `truncated = (effective line window < total_lines) || token-capped`.
7. **Record freshness**: `host.freshness().record(canonical_path, host.mtime(path))`.
   This is the hook grok emits as a `FileRead` notification (`gb/read_file/mod.rs:547`)
   but which we repurpose into the mtime store for Task 10.
8. Return `ReadFileOutput { path, lines: total_lines, truncated, body }`.

---

## 5. Design decisions (grok `file:line` + why / why-not / diff)

- **Foreground-only; drop `is_background`.** Grok's Bash is a full fg/bg system
  (`BashToolOutput::{Foreground,Background}` `gb/bash/mod.rs:296-301`; params
  `enabled_background`, `auto_background_on_timeout`, budgets `gb/bash/mod.rs:160-203`).
  *Why not port it:* background needs a task registry + `kill_task` + `monitor` +
  completion notifications (`gb/kill_task/`, `gb/monitor/`) — a subsystem, not a tool.
  SPEC scopes v0 to non-streaming, serial (Assumptions 5; ADR-0005). **Diff vs grok:**
  we advertise no `is_background`; a long command simply hits the timeout.

- **Wire name `run_terminal_cmd`** (`gb/bash/mod.rs:1581`), not `run_terminal_command`.
  *Why:* the mandate is a faithful port of grok's *actual* schema; the real `ToolId` is
  `run_terminal_cmd`. SPEC/todo say `run_terminal_command` (design-doc idealization,
  `…minimal-headless-rust-agent.md:248`). **Flagged — §8.**

- **Combined stdout+stderr, byte cap 20k, mid-output truncation.** Grok caps at
  `DEFAULT_TOOL_OUTPUT_CHARS=20_000` (`lib.rs:11`) and truncates first/last
  (`gb/bash/mod.rs:387-396`). *Why:* keeps the head (command intent) and tail (exit
  status/error) — the two highest-signal regions. Shared `truncate_for_model` (ADR-0008)
  centralizes it. **Diff vs Claude/OpenCode:** they head-truncate; grok keeps both ends.

- **read: `target_file` serde-rename kept** (`gb/read_file/mod.rs:113`). *Why:* the
  model-facing schema must match grok's real wire arg. Internally we call it `path`.

- **Dual output `{path,lines,truncated}` + body** (ADR-0003; grok `FileContent`
  `gb/read_file/mod.rs:532-541` carries `content` + `total_lines` + `offset/limit`).
  *Why:* the report wants structured metadata, the model wants the body — collapsing
  them loses information (ADR-0003 §Alternatives). **Diff:** grok also streams the body
  (`gb/read_file/mod.rs:64-76`); we buffer (v0 non-streaming).

- **Freshness recorded on read is a locode addition, not grok's.** Grok never checks
  mtime (`file_snapshot_at_edit: None`, `gb/search_replace/mod.rs:651`). *Why add it:*
  SPEC's four edit invariants (§Testing) require read-before-edit + mtime freshness
  (Claude's model, design-doc `:258-260`). *Why host-owned:* it must be visible across
  two different tool instances (`read_file` writes it, `search_replace`/`write` read it).
  **Diff vs grok:** grok relies on exact-match failure + a prompt hint ("The user may
  have changed the file…", `gb/search_replace/mod.rs:640`) to catch staleness; we add a
  hard mtime gate. This is the single most important cross-harness deviation to document.

- **Line format `N→CONTENT` reproduced verbatim** (`gb/read_file/mod.rs:108`). *Why:*
  `search_replace`'s description tells the model the `→` prefix is not part of the file
  (`gb/search_replace/mod.rs:62`); the two tools are a matched pair, so the read
  projection must match what the edit description assumes.

---

## 6. Tests (inline `#[cfg(test)]`)

**`run_terminal_cmd`:**
- `echo` round-trips: `command:"echo hi"` → `exit_code==0`, output contains `hi`,
  `prompt_text` starts `exit: 0` (SPEC Task 9 verification: "terminal tool runs echo").
- Non-zero exit is soft output, not `ToolError`: `command:"exit 3"` → `Ok(output)` with
  `exit_code==3`.
- Timeout kills a sleeper (host-level test in Task 7; a thin pack-level assertion that a
  tiny `timeout` on `sleep 5` returns a killed/`exit: killed` result).
- Byte cap: command emitting > 20k chars → `truncated==true` and marker present.
- Jail: `command` is fine, but a jail violation on the *shell cwd* is a `Respond` (host).

**`read_file`:**
- Body + line numbers: temp file, N lines → `prompt_text` has `1→…`, `lines==N`,
  `truncated==false`.
- Truncation: file with > 1000 lines → `truncated==true`, body ≤ 1000 numbered lines
  (SPEC Task 9: "read returns body + truncation note").
- Freshness recorded: after a read, `host.freshness().get(path)` is `Some(mtime)`.
- NotFound → `Err(Respond(_))` with a not-found message.

**Engine integration (SPEC Task 9 verification):** a `MockProvider` script under
`--harness grok` that calls `read_file` then `run_terminal_cmd` and produces a valid
report with both `tool_calls[]` records. (Lands once Tasks 6+8 are green.)

---

## 7. Deps to add

None beyond the workspace baseline. `locode-host` already needs `tokio`
(process/timeout), and gains a `Freshness` type using `std::sync::Mutex` +
`std::collections::HashMap` (no new crate). `schemars`/`serde`/`async-trait` are already
present in `locode-tools` and will be added to `locode-packs`' `Cargo.toml` when Task 8
wires the pack (flag: `locode-packs` currently has no `serde`/`schemars`/`async-trait`
dep — Task 8/9 must add them; this is an "ask-first: adding a dependency" per AGENTS.md,
though these are already in-tree workspace deps).

---

## 8. Open questions

1. **Tool name: `run_terminal_cmd` (grok source) vs `run_terminal_command` (SPEC/todo).**
   Recommend the real source name `run_terminal_cmd`; confirm.
2. **`read_file` token-cap result: soft `Respond` vs successful guidance `Output`?**
   Grok returns it as tool output (`FileTooLarge`, not an error); our `is_error` framing
   argues for `Respond`. Recommend `Respond`. Confirm.
3. **Negative `offset` support** (grok's "from end" semantics `gb/read_file/mod.rs:156-169`)
   — support in v0 or positive-only + reject negatives with a soft error? Recommend
   positive-only for v0.
4. **`description` arg**: grok makes it required. Keep required (schema fidelity) even
   though unused at runtime? Recommend keep required.
5. **Freshness lives in the host** — confirm this is the right home vs a pack-level shared
   struct. (Host is correct: it's the injected side-effect seam and the only object both
   `read` and `edit` share.)
