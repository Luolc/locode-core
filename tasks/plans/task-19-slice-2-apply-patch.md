# Task 19 · Slice 2 — freeform `apply_patch` + instructions + responses-only wire

> Faithful port of codex's freeform `apply_patch` (V4A parser + 4-tier fuzzy apply +
> Add/Update/Delete/Move via the host), the always-appended apply_patch instructions
> block (D4), and the openai-responses-only wire requirement (D5). Follows
> [`../../docs/codex-pack-dev-process.md`](../../docs/codex-pack-dev-process.md) (S2).
> Source: codex submodule `f201c30c` (2026-07-24).

## Phase 0 — status analysis
- **Merged (S1):** `CodexPack` + `shell_command` + minimal prompt + `--harness codex`.
- **Next unit:** `apply_patch` — codex's *only* edit path (no read/write/edit tools). A
  **freeform** tool (Lark grammar, not JSON params): the model emits the raw V4A patch
  text; our wire delivers it as `Value::String` (native `custom_tool_call` on OpenAI, or
  the `{"input": string}` fallback on grok-on-responses — both unwrap to the raw string,
  `openai/responses/parse.rs:100-111,167-173`). `type Args = String`.
- **Why now / unblocks:** completes codex's editing surface; the freeform-tool +
  wire-requirement plumbing is the last non-prompt seam. Only the full prompt (S3) remains.
- **Prereqs (hold):** `ToolInputFormat::Freeform` + `GrammarSyntax::Lark` exist
  (Task 18); the responses wire already round-trips custom tools; `Host::create_dir`
  exists. **New seam needed:** `Host::remove_file` (Delete File + Move-source removal).
- **Risks:** (1) parser fidelity (ported line-for-line from `streaming_parser.rs`);
  (2) the 4-tier fuzzy matcher incl. Unicode-punctuation normalization (tier 4 — easy to
  drop); (3) the EOF-sentinel retry + trailing-newline normalization; (4) the
  instructions-append is a **deliberate deviation** from codex (see Phase 1).

## Phase 1 — source revisit (`f201c30c`, fresh citations)
- **Tool registration** (`core/src/tools/handlers/apply_patch_spec.rs:9-27`):
  `create_apply_patch_freeform_tool` — name **`apply_patch`**; `ToolSpec::Freeform` with
  `FreeformToolFormat { type:"grammar", syntax:"lark", definition: APPLY_PATCH_LARK_GRAMMAR }`
  (`include_str!("apply_patch.lark")`, `:5`). Registered only when the model's
  `apply_patch_tool_type` is set (`spec_plan.rs:782-786`); `include_environment_id` is
  true only for `ToolEnvironmentMode::Multiple` — **we ship single-environment**, so the
  **base grammar (no `*** Environment ID:`)** and drop environment_id from the parser (gap).
  The JSON-function variant was **deleted upstream** (`ApplyPatchToolType::Freeform` is the
  sole variant, `protocol/src/openai_models.rs:286-290`).
- **Description** (`apply_patch_spec.rs:20`, verbatim): ``The `apply_patch` tool can be
  used to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.``
- **Grammar** (`core/src/tools/handlers/apply_patch.lark`, 578 bytes) — pinned verbatim.
- **Parser** (`apply-patch/src/parser.rs` + `streaming_parser.rs`): markers
  `*** Begin Patch` / `*** End Patch` / `*** Add File: ` / `*** Delete File: ` /
  `*** Update File: ` / `*** Move to: ` / `*** End of File` (`@@`/`@@ ` context). Lenient
  mode (`PARSE_IN_STRICT_MODE=false`) + a heredoc-unwrap workaround. `Hunk` =
  Add{path,contents} / Delete{path} / Update{path,move_path,chunks}; `UpdateFileChunk`
  = {change_context, old_lines, new_lines, is_end_of_file}. Add bodies: each line is
  `+`-prefixed, content + `\n`. Update lines: ` `→both, `+`→new, `-`→old, empty→both-empty;
  header/marker detection on `trim_end()`. Ported line-for-line.
- **Fuzzy apply** (`apply-patch/src/seek_sequence.rs`): `seek_sequence` — Tier 1 exact,
  Tier 2 `trim_end`, Tier 3 `trim`, Tier 4 trim + Unicode-punctuation fold (dashes→`-`,
  curly quotes→`'`/`"`, exotic spaces→` `). `eof` anchors at file end. `compute_replacements`
  (`lib.rs:715-801`): per-chunk context seek + old_lines seek with the **trailing-empty
  retry** (drop the final `""` sentinel when a match fails); `apply_replacements`
  (`lib.rs:805-829`) edits in descending order. `derive_new_contents` (`lib.rs:690-706`):
  `split('\n')`, pop trailing empty, apply, re-ensure a single trailing newline.
- **Hunk → fs** (`lib.rs:379-560`): Add → write (mkdir-parent-on-ENOENT retry,
  `write_file_with_missing_parent_retry:629-665`, `create_dir recursive`); Delete →
  `ensure_not_directory` then remove; Update → read→derive→write; Update+Move → write
  dest (mkdir retry) then remove source; a moved file is summarized under **M** by its
  **original** path (`modified.push(affected_path)`, `:538`).
- **Summary** (`lib.rs:870-885` `print_summary`): `Success. Updated the following files:\n`
  then `A <p>\n` (added), `M <p>\n` (modified), `D <p>\n` (deleted), paths as written.
- **Errors:** parse → `Invalid patch: {msg}` / `Invalid patch hunk on line {n}: {msg}`
  (`lib.rs:287-303`); apply → `Failed to find context '{ctx}' in {path}` /
  `Failed to find expected lines in {path}:\n{old}` (`lib.rs:735,790`); empty →
  `No files were modified.` (`lib.rs:368`).
- **Instructions block** (`prompts/templates/apply_patch_tool_instructions.md`, **3084
  bytes**, sha256 `061ad079…`): codex appends it (single `\n`) to base instructions
  **only when the freeform tool is ABSENT** (`core/tests/suite/prompt_caching.rs:212-216`)
  — for gpt-5.6-sol (which HAS the tool) it is NOT appended. **D4 deliberately overrides
  this: we ALWAYS append**, because we run non-codex models whose base prompt does not bake
  the V4A format in ("统一 append…肯定不会让它变更差", user 2026-07-24). Faithful *form* of
  the append (join into the system instructions with `\n`) is preserved; the *condition*
  (only-when-absent) is intentionally dropped — recorded as a gap.

## Design
```
crates/locode-packs/src/codex/
├── apply_patch/
│   ├── mod.rs      # CodexApplyPatch: Tool (freeform), run() = parse → apply → summary
│   ├── parser.rs   # StreamingPatchParser port → Hunk/UpdateFileChunk AST + ParseError
│   ├── seek.rs     # seek_sequence (4 tiers, incl. Unicode fold)
│   └── apply.rs    # compute_replacements + apply_replacements + derive_new_contents
├── apply_patch.lark                          # grammar, pinned (include_str! + sha256/len)
├── templates/apply_patch_tool_instructions.md# 3084-byte block, pinned
└── descriptions/apply_patch.md               # freeform description, pinned
```
- **Tool:** `type Args = String` (raw patch); `kind() = ToolKind::Edit`;
  `input_format() = Freeform { syntax: Lark, definition: LARK }`; `description()` = pinned.
- **run():** `parser::parse(&patch)` → for each hunk hit the host (Add: write + mkdir-retry;
  Delete: `remove_file`; Update: read→derive→write; Move: write dest + remove src) →
  collect `{added, modified, deleted}` → render the summary. All fs via `Host` (jail).
  Parse/apply failures → `ToolError::Respond` with the verbatim codex message.
- **New host seam:** `Host::remove_file(cwd, path)` (jail-resolved `tokio::fs::remove_file`;
  `FsError::Io { op:"remove", .. }`) — additive, same pattern as `create_dir`.
- **preamble()** (D4/D7): a **single** `System` message = `render(ctx) + "\n" +
  APPLY_PATCH_INSTRUCTIONS`. (S1's single-System-message test updates to assert the block
  is appended.) The `<environment_context>` User item + full prompt land in S3.
- **Wire requirement (D5):** new default trait method `Pack::required_api_schemas()
  -> Option<&'static [&'static str]>` (`None` = agnostic; grok/claude unchanged). Codex
  returns `Some(&["openai-responses"])`. `run.rs` checks it after building the provider:
  a real schema (≠ `mock`) not in the set → `PreRunError` naming pack + allowed set + got.
  `mock` is the universal keyless-CI escape hatch (stays allowed). Additive default →
  no break to `Tool`/`Provider`; pre-approved by D5.

## Gap log (this slice)
- **Instructions always appended** (D4) — codex only appends when the freeform tool is
  absent; we append unconditionally (we test non-codex models). Faithful append *form* kept.
- **environment_id dropped** — single-environment grammar only; the parser does not accept
  `*** Environment ID:` (unreachable given the base grammar we ship).
- **Delta/turn-diff tracking dropped** (`AppliedPatchDelta`) — loop-adjacent machinery
  (ADR-0023 fidelity boundary), not tool surface; we apply hunks and summarize.
- **No unified-diff preview** — codex computes a `unified_diff` for the TUI approval card;
  headless has no approval, so we skip it (the summary text is the model-facing result).
- **Partial-success left on disk** — faithful: hunks apply sequentially, a later failure
  does not roll back earlier writes (codex is the same; delta.exact tracking is dropped).

## Test matrix
- **Parser:** Add/Delete/Update round-trip to the AST; `@@` context + ` `/`+`/`-` lines;
  `*** End of File`; `*** Move to:`; malformed (bad first line, missing End Patch, bad
  hunk header, empty update hunk) → the verbatim `ParseError` message.
- **seek:** exact / trailing-ws / both-trim / Unicode-dash match; pattern-longer-than-file
  → None.
- **apply (tempdir host):** Add creates a file (+ nested path mkdirs parents); Update
  replaces a matched region; Update at EOF (trailing-empty retry); Delete removes; Move
  writes dest + removes src; context-not-found → `Failed to find context …`; lines-not-found
  → `Failed to find expected lines …`. Trailing newline normalized.
- **Summary:** `Success. Updated the following files:\nA …\nM …\nD …` ordering + prefixes.
- **Tool wiring:** registry has `["apply_patch","shell_command"]`; `apply_patch` kind =
  `Edit`; `input_format` = `Freeform{Lark}`; description pinned (len + sha256).
- **Provenance pins:** `apply_patch.lark` (len+sha256), instructions md (3084 + sha256),
  description md.
- **Preamble:** single System message contains BOTH the identity render AND the
  apply_patch instructions block (append verified).
- **Wire:** `required_api_schemas()` → `Some(["openai-responses"])`; a `run()`-level or
  unit check that `anthropic` is rejected pre-run and `mock`/`openai-responses` pass.

## Preset target
`locode -p --harness codex --api-schema mock "hi"` → `harness:"codex"` (mock stays allowed);
four-part gate green.

## Result

**Shipped** (2026-07-24, branch `feat/task-19-slice-2-apply-patch`). Codex's freeform
`apply_patch` is ported and wired; the pack now registers both stock tools.

- **Tool** (`codex/apply_patch/`): `parser.rs` (streaming V4A state machine → `Hunk`/
  `UpdateFileChunk` AST, ported line-for-line, minus environment_id), `seek.rs`
  (4-tier `seek_sequence` incl. the Unicode-punctuation fold), `apply.rs`
  (`compute_replacements`/`apply_replacements`/`derive_new_contents` with the
  trailing-empty EOF retry + single-trailing-newline normalization), `mod.rs`
  (`CodexApplyPatch`: `type Args = String`, `input_format = Freeform{Lark}`,
  `kind = Edit`; `run()` = parse → Add/Update/Delete/Move via the host → summary).
- **New host seam:** `Host::remove_file(cwd, path)` (jail-resolved; Delete + Move-source).
- **Add mkdirs parents** on `NotFound` via `Host::create_dir` (codex's
  `write_file_with_missing_parent_retry`). Summary is the verbatim
  `Success. Updated the following files:` + `A/M/D`; parse/apply errors are the verbatim
  codex messages, all soft (`ToolError::Respond`).
- **Instructions block** (D4): the 3084-byte `apply_patch_tool_instructions.md` is pinned
  and **always appended** to the base prompt in the preamble (single System message,
  `\n`-joined — codex's own append form; the only-when-tool-absent *condition* is
  deliberately dropped, gap-logged).
- **Wire requirement** (D5): `Pack::required_api_schemas()` (new default-`None` method);
  codex returns `["openai-responses"]`; `run.rs::enforce_wire_requirement` rejects a real
  mismatched `--api-schema` pre-run (`mock` always allowed).
- **Pins:** `apply_patch.lark` (578 B, sha256 `d6367f48…`),
  `apply_patch_tool_instructions.md` (3084 B, sha256 `061ad079…`),
  `descriptions/apply_patch.md` (108 B, sha256 `1d2e0992…`).
- **Tests:** +21 codex tests (parser round-trip + 4 malformed cases + heredoc unwrap;
  seek 4 tiers + too-long; derive; Add/Add-nested/Update/Update-EOF/Delete/Move; context-
  and lines-not-found; empty; summary ordering; freeform surface; 3 pins) + 2 exec
  wire-enforcement tests. Full workspace green (`fmt · clippy · test · doc`); preset
  `--harness codex --api-schema mock "hi"` → `"harness":"codex"`; `--api-schema anthropic`
  rejected pre-run.
- **Deferred to S3 (unchanged):** the full 17730-byte gpt-5.6-sol prompt + `strip_identity`
  swap + the `<environment_context>` User preamble item + the env renderer.
