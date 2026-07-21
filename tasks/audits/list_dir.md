# list_dir — fidelity audit vs Grok Build

Audited: `crates/locode-packs/src/grok/list_dir.rs` (ours, "ours file" below) vs
`~/dev/coding-cli-survey/submodules/grok-build/crates/codegen/xai-grok-tools/src/implementations/grok_build/list_dir/mod.rs`
("gb file" below). Grok ships two behavior versions (`Current` and `Legacy0_4_10`,
gb file:58-74); this audit targets **Current** — the legacy path is opt-in via a
`"legacy-0.4.10"` behavior-version contract (gb file:65-70) and is out of scope
for our port.

## Verdict

DRIFT (0 schema issues, 8 behavior issues) — the arg schema is faithful, but the tool description and nearly all model-visible output behavior (walker filters, sorting, format, budget/summarization, truncation notices, error messages) diverge. Our source flags some of this as intentional simplification (ours file:5-6), but it is drift against the faithful-mimicry rule.

## Schema comparison

| # | Grok wire name | Grok type | Grok default | Grok description (verbatim) | Grok cite | Ours wire name | Ours type | Ours default | Ours description (verbatim) | Ours cite | Status |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `target_directory` (`pub target_directory: String`, no serde rename) | `String` | none (required) | "Path to directory to list contents of, relative to the workspace root or absolute." | gb file:33-38 | `target_directory` (Rust field `directory` + `#[serde(rename = "target_directory")]`; schemars honors the rename, so the wire name matches) | `String` | none (required) | "Path to directory to list contents of, relative to the workspace root or absolute." | ours file:36-42 | MATCH |

That is grok's entire model-facing Args struct — `ListDirInput` has exactly one field (gb file:32-38). Grok additionally has a **non-model-facing** per-tool config struct `ListDirParams { max_output_chars: Option<usize> }` stored in Resources, not in the tool schema (gb file:41-48); see Quirks.

## Tool description comparison

**Grok** (`description_template`, gb file:584-592, verbatim):

```
Lists files and directories in a given path.
The '${{ params.list.target_directory }}' parameter can be relative to the workspace root or absolute.

Other details:
    - The result does not display dot-files and dot-directories.
    - Respects .gitignore patterns (files/directories ignored by git are not shown).
    - Large directories are summarized with file counts and extension breakdowns instead of listing all files.
```

The `${{ params.list.target_directory }}` marker is a MiniJinja template resolved by `TemplateRenderer` to the client-facing param name of the registered list-kind tool (template_renderer.rs:169, 182 in the same crate); for the grok_build namespace that renders to `target_directory` (a grok test asserts the template marker is present un-hardcoded, gb file:1341-1345).

**Ours** (ours file:109-111, verbatim):

```
List the contents of a directory as an indented tree.
```

**Status: DRIFT.** Ours is a one-line placeholder; grok's three "Other details" bullets (no dot-files, gitignore, summarization) also describe behavior we do not implement (see below), so today our description would be *dishonest* if copied verbatim without the behavior fixes.

## Behavior comparison

1. **Walker filters — hidden files & gitignore.** Grok builds an `ignore::WalkBuilder` with `standard_filters(true)` and `git_ignore/git_global/git_exclude` gated on a `RespectGitignore` resource that defaults to **true** (gb file:283-291, 552). So dot-files/dot-dirs are hidden and gitignored entries excluded (asserted in tests, gb file:710-726, 1028-1072). Ours does a raw `host.read_dir` walk with **no hidden-file and no gitignore filtering** (ours file:79, flagged at ours file:5). **DRIFT.**

2. **Traversal shape & item caps.** Grok: depth-1 **seed pass** capped at `MAX_SEED_ITEMS` = 100,000 (gb file:114, 294-322), then an unlimited-depth deep walk where **only depth ≥ 2 items** count against `MAX_GLOBAL_ITEMS` = 100,000 (gb file:107-109, 323-362) — so a fat early sibling cannot starve later top-level dirs (gb file:7-11, test gb file:961-992). Ours: one recursive DFS with a single `MAX_ITEMS` = 100,000 counter over **all** depths and a hard stop mid-walk (ours file:21, 84-87). **DRIFT** (constant value matches; structure does not).

3. **Sorting.** Grok sorts files and subdirs **merged, case-insensitively** (`sort_by_key(|a| a.to_ascii_lowercase())`, gb file:226-231, 233-242) — dirs are interleaved with files alphabetically (dir names carry a trailing `/` in the sort key). Ours sorts **directories first**, then case-sensitive name ascending (ours file:82). **DRIFT.**

4. **Output text format.** Grok: header line `- {display_path}/` followed by bullet lines `{indent}- {name}` where indent is `"  ".repeat(depth + 1)` (2 spaces per level starting at 2 for root children), dirs suffixed `/` (gb file:198, 243-246, 569); final output is `format!("- {}/\n{}", display_path.display(), trimmed_body)` (gb file:569). Collapsed dirs render an indented summary `[N files in subtree: 2 *.rs, 1 *.toml, ...]` with a `no-ext` bucket rendered as `N *no-ext`, top-3 extensions (`TOP_K_EXTENSIONS` = 3), ties broken alphabetically, `, ...` ellipsis when buckets remain, singular `file` for N=1 (gb file:92, 126-164). Ours: no header line, no `- ` bullet — plain `{indent}{name}{slash}` with indent starting at zero (ours file:89-91), and no summaries at all. **DRIFT.**

5. **Char budget & summarization.** Grok BFS-expands directories within a `max_output_chars` budget (default `DEFAULT_MAX_OUTPUT_CHARS` = 10,000, gb file:88-90): every dir is always *listed*; dirs that don't fit stay **collapsed to a summary line** instead of being cut (gb file:364-423), refunding the summary cost when expansion succeeds (gb file:407-412). Output length is bounded by the budget (asserted gb file:923-928). Ours: hard mid-stream truncation the moment the string reaches 10,000 chars — later siblings vanish entirely (ours file:20, 84-87). Constant matches (ours file:19-20 cites grok's); mechanism does not. **DRIFT.**

6. **Truncation notices (model-visible).** Grok emits two distinct notices: (a) item-cap cutoff appended to the body — verbatim `"\nNote: there are more than {MAX_GLOBAL_ITEMS} items in the directory, so not all files may be shown.\n"` i.e. rendering as `Note: there are more than 100000 items in the directory, so not all files may be shown.` (gb file:371-379, test gb file:781-785); (b) root-children-exceed-budget notice — template verbatim `"    ...\n\n    Note: this directory is too large to list fully. Try ${{ tools.by_kind.list }} on a narrower path, or use ${{ tools.by_kind.search }} / ${{ tools.by_kind.execute }}."` (gb file:95-97), fallback `"    ...\n\n    Note: this directory is too large to list fully. Try list_dir on a narrower path, or use grep / bash."` (gb file:99-101), appended after as many root items (+ per-child summaries) as fit (gb file:432-453). Ours: **no model-visible notice at all** — only a `truncated: bool` in the structured report (ours file:50, 143), invisible in `to_prompt_text` (ours file:56-58). **DRIFT.**

7. **Error messages.** Grok returns structured soft outputs with exact texts: not-found → `"Error: {display_path} does not exist."` plus optional path hints (gb file:509-520; base string at util/path_suggestions.rs:120); permission → `"Permission denied: {display_path}"` (gb file:521-526); file path → `"Error: {display_path} is a file, not a directory."` (gb file:527-530); other → `"Error: {display_path} is not a valid directory."` (gb file:531-534). Ours surfaces the raw host `FsError` display, e.g. `"read_dir failed for {path}: No such file or directory (os error 2)"` (ours file:113-124; locode-host/src/fs.rs:43-51, 111-119). **DRIFT.**

8. **Display path & edge cases.** Grok special-cases `"."`, `""`, whitespace, and `"./foo"` so the header never contains `/./` (`compute_display_path`, gb file:75-85, tests gb file:615-644), and uses `DisplayCwd` remapping when present (gb file:491-500). Empty dir: current version yields just the header `- {path}/` (empty trimmed body, gb file:565-570; the `no children found` filler is **legacy-only**, gb file:566-567). Ours prints no header, so an empty dir produces an empty prompt string (ours file:56-58, 126-145). **DRIFT** (folded into item 4's format fix).

9. **Points that MATCH.**
   - Non-directory / missing target fails before any listing: grok gb file:501-536; ours fail-early probe ours file:120-124. MATCH (in structure; message texts differ per item 7).
   - Unreadable *sub*directories mid-walk are silently skipped: grok skips `Err` walker entries (gb file:339); ours skips failed `read_dir` (ours file:79-81). MATCH.
   - Symlinks are not followed: grok's `ignore` walker default (`follow_links` off; `entry.file_type()` is the symlink's own type, gb file:300-321, 335-342); ours `tokio` `DirEntry::file_type` likewise doesn't follow, so symlink-to-dir has `is_dir=false` and is not descended (locode-host/src/fs.rs:126). MATCH (best-effort; UNVERIFIED that grok never enables `follow_links` elsewhere — no such call in gb file).
   - Both are read-only tools: grok `is_read_only: true`, `ToolScope::Read` (gb file:469-475); ours `ToolKind::Glob` which is a read-only kind in our taxonomy (ours file:104-106). MATCH in effect.

## Quirks

- **`ListDirParams.max_output_chars` is a resource, not a schema field** (gb file:41-48, 546-555): host-configurable per registration/gRPC, `#[serde(deny_unknown_fields)]`, defaulting to 10,000. It never appears in the model-facing JSON schema. Do not add it to our Args.
- **Depth-1 seed is exempt from the item cap** (gb file:107-115): `MAX_SEED_ITEMS` is *pinned equal* to `MAX_GLOBAL_ITEMS` via a const assert (gb file:115) so the shared cutoff-notice count stays correct whichever cap fires.
- **Partial-output quirk is deliberate**: when the walk truncates, a seed-listed sibling may appear by name with its descendants absent, and the notice copy is intentionally *not* adjusted (gb file:13-17). Reproduce as-is.
- **Sort keys include the trailing `/`** on dir names (`format!("{name}/")` before sorting, gb file:198, 227-228), so e.g. `foo.txt` sorts before `foo/` (`.` < `/`).
- **`no-ext` bucket** for extensionless files, lowercased extension keys (gb file:141-146, 166-170).
- **Non-UTF-8 names are skipped**, not lossy-converted (seed gb file:312-314; walk gb file:349); our host lossy-converts (locode-host/src/fs.rs:128).
- **Item-cap message interpolates the constant** (`more than 100000 items`), not the actual count (gb file:371-379).
- **Notices are template-rendered tool names** (`${{ tools.by_kind.* }}`), with a hardcoded fallback when no renderer exists (gb file:98-106) — our port has no TemplateRenderer, so the fallback copy (`list_dir` / `grep` / `bash`) is the faithful rendering for the grok pack, whose registered tools are exactly those names.
- **Two behavior versions exist** (gb file:58-74, versions/legacy_0_4_10.rs); we port Current only. The legacy error string `"Error: {} is not a valid directory"` **without trailing period** (gb file:55-57) differs from Current's `"...is not a valid directory."` **with period** (gb file:531-534) — do not mix them up.
- **`tracing::debug!` on root-budget truncation** (gb file:386-390) — observability only, optional.

## Fixing task

Rewrite `GrokListDir` (ours file) from the flat DFS into grok's tree-build + budget-expand pipeline. Grok's core (`DirNode`, `build_tree`, `budget_expand`, `render_truncated_root`, gb file:116-453) is pure sync code over a materialized tree, portable almost verbatim; only entry *collection* touches the host seam.

Acceptance criteria:

1. **(Gap: description)** `description()` returns grok's template with `${{ params.list.target_directory }}` rendered as `target_directory` (we have no TemplateRenderer): exactly `"Lists files and directories in a given path.\nThe 'target_directory' parameter can be relative to the workspace root or absolute.\n\nOther details:\n    - The result does not display dot-files and dot-directories.\n    - Respects .gitignore patterns (files/directories ignored by git are not shown).\n    - Large directories are summarized with file counts and extension breakdowns instead of listing all files."` (gb file:584-592).
2. **(Gap: behavior 1)** Dot-files/dot-dirs are excluded, and gitignore (`.gitignore`, git global, git exclude) is respected by default, matching `standard_filters(true)` + gitignore flags (gb file:283-291, 552). *Host-seam constraint:* our jailed `Host::read_dir` (locode-host/src/fs.rs:111-133) has no ignore support — either (a) add the `ignore` crate to `locode-packs` and walk the jail-resolved root directly (dep addition → **ask first** per AGENTS.md Boundaries), or (b) implement hidden-file + `.gitignore` matching over `Host::read_dir` output. Option (a) is the faithful one (same crate, same semantics: parse errors, precedence, global excludes).
3. **(Gap: behavior 2)** Depth-1 seed (cap 100,000, uncounted) + deep walk counting only depth ≥ 2 against `MAX_GLOBAL_ITEMS` = 100,000; seed-vs-walk starvation test ported (gb file:294-362, 961-992); keep the const-equality guard (gb file:115).
4. **(Gap: behavior 3)** Merged case-insensitive sort of files+subdirs, dir names keyed with trailing `/` (gb file:226-242).
5. **(Gap: behavior 4)** Output format: `- {display_path}/` header + `{("  ")*(depth+1)}- {name}` bullets, dirs suffixed `/`, collapsed-dir summary `[N file(s) in subtree: <top-3 ext buckets>[, ...]]` with `no-ext` bucket and count-desc/name-asc ordering (gb file:126-164, 243-280, 569). Empty dir → header only (gb file:565-570).
6. **(Gap: behavior 5)** BFS budget expansion at `DEFAULT_MAX_OUTPUT_CHARS` = 10,000 with summary-cost refund and always-listed-but-collapsed fat dirs; output length ≤ budget (gb file:364-423; test gb file:923-928). Hardcode the default; do not expose grok's `ListDirParams` resource in the schema (gb file:41-48).
7. **(Gap: behavior 6)** Truncation notices verbatim: item-cap → `"\nNote: there are more than 100000 items in the directory, so not all files may be shown.\n"` (gb file:371-379); root-over-budget → the fallback copy `"    ...\n\n    Note: this directory is too large to list fully. Try list_dir on a narrower path, or use grep / bash."` after as many root lines (+ child summaries) as fit (gb file:99-101, 432-453).
8. **(Gap: behavior 7)** Soft error texts matching Current: `"Error: {path} does not exist."`, `"Permission denied: {path}"`, `"Error: {path} is a file, not a directory."`, `"Error: {path} is not a valid directory."` (gb file:509-535; path_suggestions.rs:120), returned as `ToolError::Respond` so they stay soft. Path-hint suggestions (`PathNotFoundHints`) are gated off by default in grok (`is_some_and`, gb file:495) — omit them. *Host-seam constraint:* distinguish NotFound/PermissionDenied/IsAFile via a metadata probe (e.g. `Host::stat`, locode-host/src/fs.rs:101-104, or an added `ErrorKind`-aware helper) rather than string-matching `FsError` display text.
9. **(Gap: behavior 8)** Display-path normalization for `""`/`"."`/`"./foo"`/whitespace per `compute_display_path` (gb file:75-85), applied to the header and error texts. We have no `DisplayCwd`; use the jail-resolved cwd as the base.
10. **(Regression guard)** Port grok's behavioral tests: hidden-file exclusion (gb file:710-726), gitignore on/off (gb file:1028-1072), big-dir summarization (gb file:667-692), budget skip-and-expand-later-sibling (gb file:834-867), cutoff notice (gb file:770-785), display-path cases (gb file:615-644), error-text cases (gb file:1205-1249); update ours file's module docs (ours file:4-6) to drop the "simplifications" caveat; run the mandatory triangle.

Scope: **M** (one file rewrite ~450 lines of largely portable logic + tests; the only real design work is the ignore-filter host-seam decision in AC 2 and the dep-addition approval).

## Plan finalization (user interview, 2026-07-21)

- **Walker placement resolved (Q1 = Option A):** the gitignore-aware traversal
  becomes a `locode-host` API (walk with `WalkOptions { respect_gitignore,
  depth/budget, … }`); the **`ignore` crate is approved as a host dependency**
  (ask-first satisfied). The pack consumes entries and keeps grok's
  formatting/budgeting as pure logic — zero pack fs access, one-door invariant
  intact. The same host work exposes `is_path_ignored(path)` for the
  search_replace/read_file gitignore guards.
- Unblocked; implement after the host-groundwork slice.
