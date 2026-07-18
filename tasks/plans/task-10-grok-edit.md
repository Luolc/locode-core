# Task 10 — grok pack: `search_replace` (grok's real edit; **no `write`**)

> **⚠️ SUPERSEDED BODY:** the sections below that design a **separate `write` tool** and
> make an **empty `old_string` a soft error** are DROPPED and NOT implemented. Verified in
> source: grok has **no `write` module** (`implementations/grok_build/` has none), and an
> **empty `old_string` IS grok's file-creation path** (`handle_new_file_creation`,
> `search_replace/mod.rs:203,273`). The grok pack ships **only `search_replace`**, faithful
> to grok: `old==new` → soft "same string"; empty `old_string` → create the file;
> not-found / multiple-matches-without-`replace_all` → soft errors; `replace_all` replaces
> all. No `write`, no mtime freshness.

> **Resolved (user-confirmed):** **skip the standalone `write` tool** in the grok pack —
> grok has no `write` (it creates files via `search_replace` with empty `old_string`);
> revisit a dedicated `write` when implementing other harness packs. **Faithfully mimic
> Grok Build's real `search_replace`:** runtime enforces #2 (exact+unique) + #4 (reject
> no-op) + file-creation on empty `old_string`; #1 read-before-edit is grok's prompt/
> contract expectation; **do NOT add a runtime mtime-freshness check (#3)** — grok has
> none, so neither do we (faithful mimicry, not locode hardening). SPEC criterion 3
> reworded to match. See `tasks/plans/README.md`.

> HIGHEST RISK. Port grok's exact-string edit + create-via-empty-`old_string` faithfully.
> Runtime enforces grok's real guards only: exact + unique match (or `replace_all`), reject
> `old==new`. One unit test per grok behavior.

**Grok source root (`gb/…`):**
`coding-cli-survey/submodules/grok-build/crates/codegen/xai-grok-tools/src/implementations/grok_build/`

**locode target file:** `crates/locode-packs/src/grok/search_replace.rs`.

---

## 0. The load-bearing finding — read this first

**Grok has no standalone `write` tool, and grok's `search_replace` does NOT check
read-before-edit or mtime at runtime.** Both facts are verified against source:

- File creation is `search_replace` with an **empty `old_string`** →
  `handle_new_file_creation` (`gb/search_replace/mod.rs:203-216,273-384`). A repo grep
  for a separate write tool finds only `opencode/write` (a different harness's pack).
  The survey's canonical table lists `write` for grok as an *idealization*
  (`…minimal-headless-rust-agent.md:248`, "grok … `read_file` / `write` / `search_replace`");
  the real impl folds create into `search_replace`.
- `search_replace` does exact + unique matching and rejects no-op, but **never compares
  mtime**: `file_snapshot_at_edit` is hard-coded `None`
  (`gb/search_replace/mod.rs:651,674`); a repo grep for `mtime|snapshot|freshness` in the
  edit path is empty. Grok's "read-before-edit" is only (a) prompt guidance in the tool
  description (`gb/search_replace/mod.rs:59-63`) and (b) a **config-time** requirement
  that a Read-kind tool exists in the toolset (`requires_expr`,
  `gb/search_replace/mod.rs:768-781`) — not a runtime gate. Staleness is caught
  *implicitly*: a stale `old_string` simply won't match, and a hint nudges a re-read
  ("The user may have changed the file since you last read it.", `gb/…:639-643`).

**Consequence for this task.** Our SPEC mandates all four invariants (read-before-edit,
exact+unique, mtime freshness, reject no-op). Invariants **2 and 4 are grok-faithful**
(port them exactly, with grok's messages). Invariants **1 and 3 are a locode addition**
layered *in front of* grok's matching logic, using the host `Freshness` store from Task 9
(Claude Code's model — `errorCode 6` / `FILE_UNEXPECTEDLY_MODIFIED`, design-doc `:258-260`).
Document this split; it is the honest core of the port.

**Design choice for `write`.** We expose a separate `write` tool (SPEC/todo demand it)
whose behavior is grok's `handle_new_file_creation` create/overwrite semantics
(`gb/search_replace/mod.rs:273-384`) lifted into its own tool. `search_replace` then
*only* handles replacement in an existing file (empty `old_string` there becomes a soft
error, "use `write` to create a file"). See §5.

---

## 1. Purpose & scope

- **`write`** — create or overwrite a file with `contents`; update freshness. Grounded
  in grok's create path (`gb/search_replace/mod.rs:307,358-361`).
- **`search_replace`** — replace an exact, unique `old_string` with `new_string` in an
  existing file (or all occurrences with `replace_all`); enforce the four invariants;
  update freshness after a successful write. Grounded in `gb/search_replace/mod.rs`
  `run_search_replace` + `handle_replacement` (`:128-249,500-754`).

**Out of scope (flag):** unicode-confusable normalized fallback
(`gb/search_replace/mod.rs:410-499,564-605`; grok default-off,
`unicode_normalized_fallback:false` `:109-110`), gitignore edit-blocking (`:176-185`),
legacy-0.4.10 downgrade (`:821-826`), the `empty_old_string_does_not_override` param
(`:99-102`). CRLF preservation IS in scope (cheap + correctness-relevant, §4.2).

---

## 2. Module layout

```
crates/locode-packs/src/grok/
├── write.rs           # GrokWrite { host: Arc<Host> }
└── search_replace.rs  # GrokSearchReplace { host: Arc<Host> }
```

Both hold `Arc<Host>` (host = fs + path jail + `Freshness`; see Task 9 §2). Shared
matching helpers (position finding, replace-at-positions) mirror grok's
`gb/search_replace/helpers.rs` (`find_normalized_match_positions`,
`replace_using_positions`) — for v0 we only need the exact-match subset:
`str::match_indices` gives positions; a single-pass rebuild does the replacement.

---

## 3. Key types & signatures

### 3.1 `write`

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteArgs {
    /// grok's search_replace uses `file_path` (gb/search_replace/mod.rs:70); reuse it.
    pub file_path: String,
    /// New file contents (grok writes `new_string` as the whole file, :307).
    pub contents: String,
}
#[derive(Debug, Serialize)]
pub struct WriteOutput { pub path: String, pub created: bool /* vs overwrote */ }
impl ToolOutput for WriteOutput {
    // "The file {path} has been created successfully." (gb/search_replace/mod.rs:358-361)
    // or "…has been updated successfully." when it existed.
    fn to_prompt_text(&self) -> String { … }
}
// kind() = ToolKind::Write
```

### 3.2 `search_replace`

Grok input verbatim (`gb/search_replace/mod.rs:65-85`):

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchReplaceArgs {
    /// "The path to the file to modify…relative…or absolute." (:67-70)
    pub file_path: String,
    /// "The text to replace" (:71-72)
    pub old_string: String,
    /// "The text to replace it with (must be different from old_string)" (:73-76)
    pub new_string: String,
    /// "Replace all occurrences of old_string (default false)" (:81-84)
    #[serde(default)]
    pub replace_all: bool,
}
#[derive(Debug, Serialize)]
pub struct SearchReplaceOutput {
    pub path: String,
    pub replacements: usize,   // 1, or N for replace_all
}
impl ToolOutput for SearchReplaceOutput {
    // 1: "The file {path} has been updated successfully." (:724-728)
    // N: "…updated. All occurrences were successfully replaced." (:732-735)
    fn to_prompt_text(&self) -> String { … }
}
// kind() = ToolKind::Edit
```

Both register under grok's real wire names — `write` and **`search_replace`**
(`gb/search_replace/mod.rs:787` `ToolId::new("search_replace")`). `kind()` for
`search_replace` is `ToolKind::Edit` (grok `:757`).

Every failure is `ToolError::Respond(String)` — never `Fatal` (a bad edit is recoverable;
ADR-0004). Grok's structured `SearchReplaceOutput::{NoMatchesFound, MultipleMatchesFound,
InvalidInput, …}` variants all collapse to `Respond(message)` on our side, preserving the
exact message text.

---

## 4. Behavior / invariants (one subsection per invariant)

Pre-checks that gate every `search_replace` call, in grok's order:
- `file_path` resolves under the jail (`..`/absolute escape → `Respond`, ADR-0008).
- Path is not a directory → `Respond("File path is a directory")` (grok `:168-172`).
- Path-component length ≤ 255 (`NAME_MAX`, grok `:252,257-271`) — optional in v0.

### Invariant 4 — reject no-op (do this first, grok does)

`old_string == new_string` → `Respond("Old string and new string are the same")`
(grok `gb/search_replace/mod.rs:187-191`, checked **before** create-vs-replace branching).
Applies to `search_replace` only. This is grok-faithful; port the message verbatim.

### Invariant 2 — exact match + uniqueness

The core, grok-faithful (`gb/search_replace/mod.rs:560-662`):
1. Read the file via host. CRLF-normalize a working copy for matching (`:554-559`).
2. `positions = match_text.match_indices(&old_string).map(|(i,_)| i)` (`:560-563`).
3. **No match** (`positions.is_empty()`, `:606-654`) →
   `Respond("The string to replace was not found in the file, use the read_file tool to
   see the correct string.{user_edit_hint}")`, where `user_edit_hint` = `" The user may
   have changed the file since you last read it."` (grok `:639-649`; drop the
   nearest-match/confusable hints in v0). *This is grok's staleness catch-all.*
4. **Multiple matches without `replace_all`** (`positions.len() > 1 && !replace_all`,
   `:655-662`) → `Respond("The string to replace was found multiple times in the file.
   Use replace_all to replace all occurrences, or include more context to only edit one
   occurrence.")` — **report the count** so the model can add context (SPEC Task 10:
   "soft-error with match count"). Grok's message names the tool arg; we inline
   `replace_all`.
5. Otherwise replace: 1 occurrence, or all when `replace_all` (grok
   `replace_using_positions`, `:681-687`).

### Invariant 1 — read-before-edit (locode addition; except new-file)

Runs **before** Invariant 2's file read, gating `search_replace` (not `write`, and not
the empty-`old_string` create path — the new-file exception, design-doc `:258`):
- If `host.freshness().get(canonical(file_path)).is_none()` →
  `Respond("File has not been read yet. Read it first before writing to it.")`
  (Claude Code's read-before-edit, `errorCode 6`; message ours since grok has none).
- **Why grok has no runtime equivalent:** grok enforces this at config time
  (`requires_expr`, `gb/search_replace/mod.rs:768-781`) + prompt. We make it a runtime
  gate because SPEC requires it and a headless loop can't rely on prompt compliance.

### Invariant 3 — mtime freshness re-check (locode addition)

After the read-before-edit gate passes and before writing:
- `recorded = freshness.get(path)`; `current = host.mtime(path)`.
- If `current != recorded` → `Respond("File has been modified since it was last read.
  Read it again before editing.")` (Claude's `FILE_UNEXPECTEDLY_MODIFIED`, design-doc
  `:260`).
- **After a successful write, update freshness** to the new mtime
  (`freshness.record(path, host.mtime(path))`) so chained edits stay valid without a
  re-read (design-doc `:260`, "Update the freshness record after a successful write").
- **Why grok lacks this:** see §0. Grok would let a same-string edit reapply against a
  changed file; we hard-stop.

### CRLF handling (grok-faithful, keep)

Match against a `\r\n`→`\n` normalized copy; if the original had CRLF, re-expand
`\n`→`\r\n` before writing (grok `:554-559,688-692`). Prevents a Windows file's line
endings from defeating an exact match and from being silently rewritten.

### `write` behavior

1. Jail-resolve `file_path`.
2. `created = !host.exists(path)` (or file empty — grok's `file_exists` check reads the
   file and treats empty as "creating", `:285-288`).
3. `host.write_file(path, contents.as_bytes())` (grok `:307`). Map io errors to `Respond`
   mirroring grok (`:308-336`): NotFound(parent missing) → not-found message;
   AlreadyExists(component is a file) → "A component of the path already exists as a file
   where a directory is expected."; else "failed to write {path}: {e}".
4. `freshness.record(path, host.mtime(path))` — so an immediate `search_replace` on a
   just-written file passes invariants 1 & 3.
5. `to_prompt_text` = grok's create/update message (`:358-361`).

---

## 5. Design decisions (grok `file:line` + why / why-not / diff)

- **Separate `write` tool, sourced from grok's create path.** Grok folds create into
  `search_replace` (empty `old_string` → `handle_new_file_creation`, `:203-216`). *Why
  split it out:* SPEC §Success-Criteria-2 and todo Task 10 list `write` as a first-class
  tool with `ToolKind::Write`, which the cross-pack A/B alignment wants (a grok `write`
  aligns with Claude `Write`/OpenCode `write`). *Why not keep it fused:* a fused tool
  can't carry `ToolKind::Write`. **Diff vs grok:** in `search_replace`, an empty
  `old_string` becomes `Respond("old_string is empty — use write to create a file")`
  instead of creating (we move creation to `write`). Alternative considered & rejected:
  faithfully fuse and drop the `write` tool — rejected because it violates SPEC's tool
  list and the A/B `ToolKind` mapping.

- **Invariants 1 & 3 added in front of grok's matcher.** *Why:* SPEC's four invariants
  (§Testing) and the design-doc convergence (`:256`, "every system converged on the same
  guardrails"). *Why-not skip (be grok-pure):* grok's implicit staleness catch (exact
  match won't hit) is weaker — a `replace_all` of a common token, or a create-overwrite
  race, can corrupt silently; the headless loop has no human to notice. **Diff vs grok:**
  we hard-stop on unread/stale files; grok soft-nudges via prompt text.

- **Invariants 2 & 4 ported verbatim (messages included).** `:187-191` (no-op),
  `:655-662` (multi-match + count), `:606-654` (no-match). *Why verbatim:* the model was
  trained/tuned against these exact strings; message fidelity is behavior fidelity
  (SPEC: behavior P0). **Diff vs Claude/OpenCode:** grok reports match *count* guidance
  and offers `replace_all`; OpenCode instead runs 5 fuzzy replacers (design-doc `:261`) —
  we deliberately resist fuzzy (invariant 4 / SPEC Open-Q 1: exact-only default).

- **Drop unicode-confusable fallback + nearest-match/confusable hints.** Grok gates the
  fallback behind a default-off param (`:109-110`) and builds elaborate hints
  (`:385-499`). *Why-not port:* off by default in grok, large surface, orthogonal to the
  invariants. Reserve as a later enhancement.

- **CRLF preserved.** Grok `:554-559,688-692`. *Why:* correctness — cheap and prevents
  spurious no-match + line-ending churn on Windows-authored files.

- **Freshness key = canonical jailed path.** So `write("a")` then `search_replace("./a")`
  share one freshness entry. (Host owns canonicalization; Task 9 §2.)

---

## 6. Tests — one per invariant + happy path (SPEC Task 10)

Each violation returns the correct `Respond` message; assert on the message text.

1. **Reject no-op (inv. 4):** `old_string == new_string` → `Respond("Old string and new
   string are the same")`. (`gb/…:187-191`)
2. **Exact + unique (inv. 2):**
   - a) `old_string` appears twice, `replace_all=false` → `Respond` containing "found
     multiple times" + the `replace_all` suggestion (`:655-662`).
   - b) `old_string` absent → `Respond` containing "was not found in the file" (`:606-654`).
   - c) happy: unique match → file updated, `replacements==1`.
   - d) `replace_all=true` with 3 matches → all replaced, `replacements==3`.
3. **Read-before-edit (inv. 1):** edit a file with no freshness record → `Respond("…not
   been read yet…")`. Then `read_file` it → same edit succeeds. New-file exception:
   `write` needs no prior read.
4. **mtime freshness (inv. 3):** read → mutate the file on disk (bump mtime) → edit →
   `Respond("…modified since it was last read…")`. And: successful edit updates freshness
   so a **second** chained edit (same session, no re-read) succeeds.
5. **`write` happy path:** create new file → `created==true`, freshness recorded, body
   "created successfully"; overwrite existing → `created==false`, "updated successfully".
6. **Chained edit happy path (SPEC):** `read_file` → `search_replace` → `search_replace`
   again on the same file with no intervening read (freshness carried by invariant-3's
   post-write update).

CRLF test: a `\r\n` file, `old_string` written with `\n` → matches and rewrites with
`\r\n` preserved.

---

## 7. Deps to add

None beyond Task 9's `Freshness` host addition and `serde/schemars/async-trait` on
`locode-packs`. Exact matching uses `str::match_indices` (std). No fuzzy-match or diff
crate (invariant 4 — resist).

---

## 8. Open questions

1. **Edit strictness = exact-only in v0?** SPEC Open-Q 1 default is exact-only; this plan
   assumes it. Confirm we do **not** port grok's unicode-normalized fallback or any
   OpenCode fuzzy replacer in v0.
2. **`write` as a separate tool vs faithful fusion into `search_replace`.** This plan
   splits it (per SPEC/todo). Confirm — it is the one place we deviate from grok's real
   tool boundary.
3. **`search_replace` on empty `old_string`**: soft-error pointing at `write` (this plan)
   vs. replicate grok's in-tool create. Recommend soft-error. Confirm.
4. **Invariant 1/3 message wording**: grok has none (they're our additions). Proposed
   Claude-flavored strings above — accept, or tune to match a specific harness's copy?
5. **`NAME_MAX`/path-length + directory checks**: include grok's `:257-271,168-172`
   guards in v0, or defer? Recommend include the directory check, defer NAME_MAX.
