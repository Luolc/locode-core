# Task 11 — grok pack: `grep` (ripgrep) + `list_dir` (grok's walker)

> **Resolved (user-confirmed, supersedes the framing below):** faithful mimicry wins for
> ported packs (AGENTS.md; ADR-0011 amendment). The grok pack ships grok's **`grep`**
> (ripgrep-backed — grok uses rg too) **+ grok's real `list_dir`** (an fs tree walker,
> ported as-is). It does **not** ship an `rg --files` glob — that's the `locode` pack's
> choice (next milestone). Where this plan below says "Glob via `rg --files`", read it as
> the `locode`-pack design, not the grok pack. See `tasks/plans/README.md`.

> Port grok's search tools; ripgrep-backed, resolved through the host (ADR-0011). No
> hand-rolled walker. Soft `Respond` if `rg` is unresolvable.

**Grok source root (`gb/…`):**
`coding-cli-survey/submodules/grok-build/crates/codegen/xai-grok-tools/src/implementations/grok_build/`

**locode target files:** `crates/locode-host/src/rg.rs` (resolver) + a process runner,
`crates/locode-packs/src/grok/{grep.rs, list_dir.rs}` (`grep` = rg; `list_dir` = grok's
self-implemented fs walk — **no `glob` tool** in the grok pack).

---

## 0. The dir/glob tension — read first

Grok's directory tool is **`list_dir`** (`gb/list_dir/mod.rs`), and it is a **std::fs
BFS tree walker**, not ripgrep-backed and **not a glob matcher**: its only arg is
`target_directory` (`gb/list_dir/mod.rs:32-38`), it walks children within a char budget
(`DEFAULT_MAX_OUTPUT_CHARS=10_000`, `:46,90`; `MAX_GLOBAL_ITEMS=100_000`, `:109`) and
prints an extension-summarized tree. There is **no glob-pattern file-finder** in grok's
main namespace; grok does glob-style file finding via `grep`'s `glob` param
(`gb/grep/mod.rs:59-62`) or via `run_terminal_cmd` + `rg --files` (survey tool-surface:
"Glob | list_dir / packs").

**ADR-0011 overrides the faithful port here.** ADR-0011 §Decision-1 is explicit:
"`Glob` uses `rg --files` + glob filtering… **no hand-rolled walker**… If `rg` cannot be
resolved, the tools return a soft `Respond` error." Task 11 (todo `:200-208`) says the
same: "dir/glob tools implement `Tool` over the resolved `rg` (glob via `rg --files` +
filter)." So our dir/glob tool is a **deliberate, ADR-sanctioned deviation** from grok's
`list_dir`: same *affordance* (enumerate files under a path), different *engine*
(`rg --files`, gitignore-aware) and a **glob-pattern** capability grok's `list_dir` lacks.

**This plan's resolution:** ship a **`glob`** tool (`pattern` + optional `path`) backed
by `rg --files` + glob filtering (the ADR-0011 contract; `ToolKind::Glob`), and keep
grok's **`list_dir`** *name* as an optional thin alias/mode if we want directory-listing
ergonomics — but its enumeration is also `rg --files` (no fs walker). See §5 + §8; this
is the one place to confirm with the user.

---

## 1. Purpose & scope

1. **`rg` resolver in `locode-host`** (`crates/locode-host/src/rg.rs`): cached, order
   `LOCODE_RG_PATH` → host-provided bundled path → bare `rg` on PATH. Mirrors grok's
   `rg_path()` (`gb/grep/ripgrep.rs:43-81`) and ADR-0011 §Decision-2.
2. **`grep`** — regex file-content search over resolved `rg` (grok `gb/grep/mod.rs`,
   `ToolId "grep"` `:264`). Port grok's arg schema, rg flags, defaults, truncation.
3. **`glob`** (dir/glob) — file-name/path matching via `rg --files <path>` + glob filter
   (ADR-0011). `ToolKind::Glob`.

Both search tools: path-jailed, output-truncated, and return a **soft `Respond`** if `rg`
can't be resolved or spawned (grok soft-fails too, §4.1).

**Out of scope (flag):** grok's `list_dir` fs-tree walker with extension summaries
(`gb/list_dir/mod.rs`) — replaced by rg-backed enumeration per ADR-0011; grok's hidden
`output_mode`/count/files modes may be reduced (§5); streaming; WSL timeout bump.

---

## 2. Module layout

```
crates/locode-host/src/
└── rg.rs        # RgResolver: cached rg PathBuf or ResolveError

crates/locode-packs/src/grok/
├── grep.rs      # GrokGrep { host: Arc<Host> }
└── glob.rs      # GrokGlob { host: Arc<Host> }   (dir/glob)
```

Host gains `fn rg(&self) -> Result<&Path, RgUnavailable>` (cached via `OnceLock`) plus a
helper to *spawn* rg with jail-relative cwd + truncation. Tools call the host, never
`Command` directly (ADR-0008).

---

## 3. Key types & signatures

### 3.1 Host `rg` resolver (`crates/locode-host/src/rg.rs`)

```rust
pub struct RgResolver { cell: OnceLock<Option<PathBuf>> }
impl RgResolver {
    /// Order (ADR-0011 §Decision-2; grok gb/grep/ripgrep.rs:43-81):
    ///  1. env LOCODE_RG_PATH (tests/packaging override; grok uses RG_BIN_PATH :54)
    ///  2. host-provided bundled/self-extracted path (bundle-rg feature; grok :47-49)
    ///  3. bare `rg` on PATH — invoked BY NAME, not a cwd-relative absolute path
    ///     (PATH-hijack hygiene, ADR-0011 §Decision-2c / Claude Code; grok :77)
    /// Returns None only when step 3's bare `rg` also isn't spawnable.
    pub fn resolve(&self) -> Option<&Path>;
}
```

Grok always falls back to the bare string `"rg"` and never returns an error
(`gb/grep/ripgrep.rs:77`); the spawn simply fails later. **We differ (per ADR-0011):**
`resolve()` may yield `None` (or the tool detects spawn failure) and the tool returns
`Respond`. Step-3 "by name" = pass `Path::new("rg")` to `Command::new` so the OS PATH
lookup applies (no `workspace_root.join("rg")`).

### 3.2 `grep`

Grok input verbatim (`gb/grep/mod.rs:47-128`), snake_case + rg-flag-named fields:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrepArgs {
    /// "The regular expression pattern…(rg --regexp)" (:49-52)
    pub pattern: String,
    /// "File or directory to search in…Defaults to workspace path." (:54-57)
    #[serde(default)] pub path: Option<String>,
    /// r#"Glob pattern (rg --glob GLOB) to filter files (e.g. "*.js", "*.{ts,tsx}")."# (:59-62)
    #[serde(default)] pub glob: Option<String>,
    #[serde(rename = "-B", default)] pub before_context: Option<usize>,   // rg -B (:70-76)
    #[serde(rename = "-A", default)] pub after_context: Option<usize>,    // rg -A (:78-85)
    #[serde(rename = "-C", default)] pub context: Option<usize>,          // rg -C (:87-94)
    #[serde(rename = "-i", default)] pub case_insensitive: Option<bool>,  // rg -i (:96-105)
    #[serde(rename = "type", default)] pub r#type: Option<String>,        // rg --type (:107-110)
    #[serde(default)] pub head_limit: Option<usize>,                      // |head -N (:112-117)
    #[serde(default)] pub multiline: Option<bool>,                        // rg -U --multiline-dotall (:119-127)
}
#[derive(Debug, Serialize)]
pub struct GrepOutput { pub matches: usize, pub truncated: bool, #[serde(skip)] body: String }
impl ToolOutput for GrepOutput { fn to_prompt_text(&self) -> String { self.body.clone() } }
// kind() = ToolKind::Grep   (grok's own tag is ToolKind::Search, :240-242)
```

Drop grok's hidden `output_mode` (files/count) in v0 or keep it hidden-default-Content
(`:64-67,40-45`); recommend Content-only for v0 (§5).

### 3.3 `glob` (dir/glob)

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GlobArgs {
    /// Glob to match file paths, e.g. "**/*.rs", "src/*.ts". (ADR-0011)
    pub pattern: String,
    /// Directory to search under; defaults to workspace root. (canonical Glob arg,
    /// design-doc …minimal-headless-rust-agent.md:236 `pattern, path?`)
    #[serde(default)] pub path: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct GlobOutput { pub files: Vec<String>, pub truncated: bool }
impl ToolOutput for GlobOutput { fn to_prompt_text(&self) -> String { self.files.join("\n") } }
// kind() = ToolKind::Glob
```

Wire names: **`grep`** (grok `:264`) and **`glob`** (see §8 — vs keeping grok's
`list_dir` name).

---

## 4. Behavior / invariants

### 4.1 `grep`

1. Resolve `rg` via host. If unresolved → `Respond("ripgrep (rg) is not available; set
   LOCODE_RG_PATH or install rg")`. (ADR-0011 §Decision-1: no silent divergent fallback.)
2. Resolve `path` (default = `workspace_root`) under the jail; escapes → `Respond`.
3. Build the rg command exactly as grok (`gb/grep/mod.rs:758-827`):
   `--heading --with-filename --line-number --color=never --max-columns 1000
   --max-columns-preview`; then `--ignore-case` (if `-i`), `--glob GLOB` (if `glob`),
   `--type T` (if `type`), `-U --multiline-dotall` (if `multiline`),
   `-C/-B/-A N`, `-e <pattern>`, `<workdir>`, `--max-filesize 5M`.
4. Spawn with stdout/stderr piped, stdin null (grok `:829-831`). Honor `ctx.cancel`
   (kill the child). Apply grok's wall-clock timeout (20s default, `:174`).
5. On spawn failure → grok returns a soft output `stderr:"Error calling tool: {e}",
   exit_code:-1` (`:833-843`); we map to `Respond("Error calling tool: {e}")`.
6. Read stdout up to `MAX_STDOUT_BYTES=5MB` (`:165`); truncate per line at
   `max_chars_per_line=1000` (`:143,162`) and total at `DEFAULT_TOOL_OUTPUT_BYTES=40KB`
   (`:139-140`; `lib.rs` const). Head-limit to `head_limit.unwrap_or(200).min(2000)`
   content lines (`:153-157,197-203`).
7. `to_prompt_text`: grok wraps as `<workspace_result workspace_path="…">…</workspace_result>`
   with a "Found N matching lines" / "No matches found" header (`:1028,1366`). v0 may keep
   the grok wrapper or simplify to rg's `--heading` output + a count line; recommend keep
   the "Found N…"/"No matches found" summary (behavior fidelity). `exit_code == 1` from rg
   means "no matches" (normal, not an error).
8. `truncated` set if any cap fired.

### 4.2 `glob` (dir/glob)

1. Resolve `rg` (soft `Respond` if unresolved — same as grep).
2. Resolve `path` (default workspace root) under the jail.
3. Run `rg --files <path>` (ADR-0011; gitignore-aware; **no walker**). Optionally
   `--glob <pattern>` to let rg filter, or filter the returned paths in-process against
   `pattern` (a `globset`-style match). Recommend `rg --files --glob <pattern> <path>`
   so rg does the filtering (one process, consistent gitignore semantics).
4. Jail every returned path (defensive) and make them workspace-relative for output.
5. Truncate to a max file count + a byte cap; set `truncated`.
6. Soft `Respond` on spawn failure. Empty result is a normal empty `Vec`, not an error.

### Path jail + truncation (both tools)

All paths resolve under `workspace_root`; `..`/absolute escapes → `Respond` (ADR-0008).
Output truncation goes through the host's shared `truncate_for_model` where possible
(ADR-0008: "shared post-process, not per-tool ad hoc").

---

## 5. Design decisions (grok/ADR `file:line` + why / why-not / diff)

- **Search engine is `rg`, unconditionally; unresolved → soft `Respond`.** ADR-0011
  §Decision-1; grok resolver `gb/grep/ripgrep.rs:43-81`. *Why:* determinism (pinned rg
  semantics for gitignore + output) vs a divergent walker (ADR-0011 §Context/Alternatives).
  **Diff vs grok:** grok silently falls back to bare `"rg"` and lets the spawn fail
  mid-tool (`ripgrep.rs:77`; `grep/mod.rs:833-843`); we surface a clear soft error up
  front (ADR-0011: "not a silent divergent fallback").

- **Resolver order `LOCODE_RG_PATH → bundled → bare rg by name`.** ADR-0011 §Decision-2;
  grok `RG_BIN_PATH → bundle → RUNFILES → "rg"` (`gb/grep/ripgrep.rs:54,47-49,77`).
  *Why "by name" for PATH:* PATH-hijack hygiene (Claude Code; ADR-0011 §Decision-2c) —
  never `cwd.join("rg")`. **Diff:** we rename grok's `RG_BIN_PATH` env to `LOCODE_RG_PATH`.

- **grep arg schema ported verbatim, incl. rg-flag field names (`-A/-B/-C/-i`).**
  `gb/grep/mod.rs:47-128`. *Why:* behavior P0 = the model's mental model of grep is
  grok's schema; renaming args changes behavior. **Diff vs Claude/OpenCode grep:** those
  use different arg shapes; the pack layer keeps each harness's real names (ADR-0012).

- **rg flag set + `--max-filesize 5M` + caps ported.** `gb/grep/mod.rs:761-827,165,
  139-143,153-161,174`. *Why:* these bounds (5MB stdout, 40KB output, 1000 chars/line,
  200/2000 line head-limit, 20s timeout) are grok's tuned guardrails against a giant-repo
  walk flooding the context; reproduce them.

- **Drop grok's `output_mode` (files/count) in v0.** Grok hides it from the schema
  (`gb/grep/mod.rs:64-67`) and defaults to Content (`:40-45`). *Why-not port:* it's a
  hidden power-user mode; Content mode is the model-facing default. Keep the door open
  (the enum can return). Confirm in §8.

- **dir/glob = rg-backed `glob`, not grok's fs `list_dir`.** ADR-0011 §Decision-1 +
  §Consequences ("Task 11 simplifies… there is no walker to build or test"); grok's
  `list_dir` is a std::fs walker (`gb/list_dir/mod.rs:1-17,32-38`). *Why deviate:* the
  ADR is explicit and supersedes the faithful port for this affordance; a rg-backed glob
  is gitignore-consistent with grep and needs no walker. **Diff vs grok:** we expose
  glob-pattern matching (`**/*.rs`) grok's `list_dir` cannot do, and we lose grok's tree
  rendering + extension summaries (acceptable: behavior P0 is "find files under a path").
  *Alternative considered:* faithfully port `list_dir`'s fs walker — **rejected**, it
  violates ADR-0011's "no hand-rolled walker".

---

## 6. Tests (temp-tree + resolver; SPEC Task 11)

**Resolver (`rg.rs`):**
- `LOCODE_RG_PATH` honored: point it at a stub script, assert the resolver returns it
  (SPEC: "resolver honors LOCODE_RG_PATH pointed at a stub").
- Unresolvable: clear PATH + unset override → `resolve()` is `None`; both tools return
  `Respond` (SPEC: "soft-error path when rg is unresolvable").
- Bare-name PATH resolution: rg on a temp PATH dir resolves by name.

**`grep`:** temp tree with known content →
- matches expected lines; `matches == N`; body contains the file+line (SPEC: "grep
  matches lines").
- no-match pattern → "No matches found", `matches == 0`, not an error.
- `glob:"*.rs"` filter restricts to `.rs` files.
- output over the byte cap → `truncated == true`.
- jail: `path:"../etc"` → `Respond`.

**`glob`:** temp tree →
- `pattern:"**/*.rs"` finds exactly the `.rs` files (SPEC: "glob finds expected paths").
- gitignored files excluded (rg respects `.gitignore`).
- unresolved rg → `Respond`.

**Engine integration:** MockProvider under `--harness grok` invokes `grep`/`glob` and
produces a valid report (after Tasks 6+8).

---

## 7. Deps to add

- `locode-host`: `tokio` process/io (likely already present from Task 7). Optionally
  `globset` **only if** we filter glob results in-process rather than via `rg --glob`;
  recommend `rg --glob` to avoid a new dep (adding one is "ask-first", AGENTS.md).
- `locode-packs`: `serde`/`schemars`/`async-trait` (shared with Tasks 9/10).
- The `bundle-rg` cargo feature + `build.rs` embedding is **Task 14**, not here (ADR-0011
  §Decision-3); Task 11 only consumes the resolver seam.

---

## 8. Open questions

1. **dir/glob: ship `glob` (rg `--files`, `ToolKind::Glob`) — confirm we deviate from
   grok's `list_dir` fs-walker per ADR-0011.** This is the biggest call in Task 11.
   Sub-question: keep the wire name `glob`, or keep grok's `list_dir` name with a
   `target_directory` arg but rg-backed enumeration (more grok-surface-faithful, less
   glob-capable)? Recommend `glob` + `pattern`/`path`.
2. **grep `output_mode` (files-with-matches / count):** drop in v0 (recommended) or keep
   hidden-default-Content like grok?
3. **grep output rendering:** keep grok's `<workspace_result …>` wrapper + "Found N…"
   summary (recommended, behavior-faithful) or simplify to plain rg `--heading` output?
4. **Glob filtering:** `rg --glob <pattern>` (no new dep, recommended) vs in-process
   `globset` (a new dependency — ask-first)?
5. **rg exit-code semantics:** treat rg exit 1 (no match) as success/empty (recommended);
   exit ≥ 2 (bad regex/path) → `Respond` with rg's stderr. Confirm.
