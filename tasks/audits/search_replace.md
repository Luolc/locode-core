# search_replace — fidelity audit vs Grok Build

Audited 2026-07-20 directly against
`xai-grok-tools/src/implementations/grok_build/search_replace/` ("gb" below;
`gb/mod.rs` = `.../search_replace/mod.rs`, `gb/helpers.rs`, `gb/versions/legacy_0_4_10.rs`,
plus `xai-grok-tools/src/types/output.rs` ("gb-out") and
`xai-grok-tools/src/util/path_suggestions.rs` ("gb-hints")).
Ours: `crates/locode-packs/src/grok/search_replace.rs` ("ours"), registered as
`search_replace` in `crates/locode-packs/src/grok/mod.rs:40`.

Key structural finding up front: **ours is a faithful port of grok's
`legacy-0.4.10` error surface, not of grok's current default behavior.** Grok
carries an internal version switch (`SearchReplaceVersion`, gb/mod.rs:36-52);
the current (default) variant adds hints, structured errors, CRLF handling, and
a gitignore guard, and the legacy variant exists only for old contracts
(`gb/versions/legacy_0_4_10.rs:1-92`). Our error strings match the legacy
downgrade texts exactly, which is drift against what Grok Build ships by default.

## Verdict

**DRIFT (2 schema issues, 8 behavior issues).**

## Schema comparison

All four wire fields exist on both sides with matching names, types, defaults,
and (rendered) descriptions. Grok's descriptions contain MiniJinja placeholders
(`${{ params.edit.old_string }}` etc.) that the harness renders to the
client-facing param names; in the grok_build toolset those render to the
literal names `old_string` / `replace_all` (renderer fixture: gb/mod.rs:842-853;
read tool renders to `read_file`, gb/mod.rs:849). Rendered-form comparison used
below.

| Field (wire) | Grok (gb/mod.rs) | Grok description (verbatim) | Ours (search_replace.rs) | Our description (verbatim) | Status |
|---|---|---|---|---|---|
| `file_path` | `String`, required, :67-70 | "The path to the file to modify. You can use either a relative path in the workspace or an absolute path." | `String`, required, :29-32 | identical | MATCH |
| `old_string` | `String`, required, :71-72 | "The text to replace" | `String`, required, :33-34 | identical | MATCH |
| `new_string` | `String`, required, :73-76 | "The text to replace it with (must be different from ${{ params.edit.old_string }})" → renders "…different from old_string)" | `String`, required, :35-36 | "The text to replace it with (must be different from old_string)" | MATCH (rendered form) |
| `replace_all` | `bool`, `#[serde(default, deserialize_with = "…deserialize_lenient_bool")]` → default `false`, :77-84 | "Replace all occurrences of ${{ params.edit.old_string }} (default false)" → renders "…of old_string (default false)" | `bool`, `#[serde(default)]` → default `false`, :37-39 | "Replace all occurrences of old_string (default false)" | MATCH on name/type/default/description; **DRIFT on parsing** — see schema issue 2 |

Schema issues:

1. **Tool description drift** (see next section).
2. **`replace_all` lenient-bool coercion missing.** Grok deserializes
   `replace_all` through `deserialize_lenient_bool`
   (gb/mod.rs:77-80), which accepts `true/false`, `"true"/"false"`,
   `"yes"/"no"`, `"1"/"0"`, `1/0`, and `null`→false
   (`xai-tool-types/src/serde_lenient.rs:1-60`). Ours is plain
   `#[serde(default)] bool` (ours :37-39), so a model emitting
   `"replace_all": "true"` gets a deserialization error instead of a
   successful edit.

## Tool description comparison

**DRIFT.** Grok's real description is the 3-bullet `DESCRIPTION_FULL` template
(gb/mod.rs:59-63), verbatim:

```
Replace an exact string in a file.

- Read the file with `${{ tools.by_kind.read }}` before editing it.
- `${{ tools.by_kind.read }}` prefixes each line with "LINE_NUMBER→". That prefix is not part of the file: match only what comes after the →, with its exact indentation.
- `${{ params.edit.old_string }}` must match exactly one place in the file. If it appears more than once, add surrounding lines to make it unique, or set `${{ params.edit.replace_all }}` to change every occurrence (handy for renaming an identifier).
```

Rendered for the grok_build toolset (`read` → `read_file`, params literal;
renderer mapping gb/mod.rs:842-853):

```
Replace an exact string in a file.

- Read the file with `read_file` before editing it.
- `read_file` prefixes each line with "LINE_NUMBER→". That prefix is not part of the file: match only what comes after the →, with its exact indentation.
- `old_string` must match exactly one place in the file. If it appears more than once, add surrounding lines to make it unique, or set `replace_all` to change every occurrence (handy for renaming an identifier).
```

Ours (ours :76-78) is an invented one-liner:

```
Replace an exact, unique string in a file with new text (or create the file when old_string is empty). Set replace_all to change every occurrence.
```

(Aside: grok's full description never mentions the empty-`old_string` creation
path; only the concise variant does —
`grok_build_concise/search_replace.rs:6-10`: "To create a new file, set
${{ params.edit.old_string }} to an empty string." Ours mixes both.)

## Behavior comparison

Model-facing output plumbing on the grok side: `EditsApplied` renders
`tool_output_for_prompt`, every other variant renders its message string
(gb-out :738-748); every non-`EditsApplied` variant counts as a tool error
(gb-out :669-674). No snippet, diff, or context is ever echoed to the model —
the `SearchReplaceEditDetail` context (CONTEXT_LINES=3, gb/mod.rs:35,
gb/helpers.rs:97-128) feeds notifications/UI only. Ours likewise echoes only a
one-line prompt text (ours :53-64). Structure matches; texts don't.

### Match-uniqueness rules — MATCH (core), DRIFT (CRLF)

- Exact, non-overlapping substring match; 0 matches → no-match error, >1
  matches without `replace_all` → multiple-match error, `replace_all` replaces
  every occurrence. Grok: positions via `match_indices`
  (gb/mod.rs:560-563), multi-match gate gb/mod.rs:655-662, replacement via
  `replace_using_positions` (gb/helpers.rs:75-94). Ours: `matches().count()`
  (ours :131), gates ours :132-141, `replace`/`replacen` ours :143-147.
  Equivalent for LF files.
- **No fuzzy/indentation-insensitive/trimmed fallback exists on either side.**
  Matching is byte-exact. Grok's only non-exact path is the Unicode-confusable
  normalized fallback (gb/mod.rs:564-604, gb/helpers.rs:177-238), and it is
  **config-gated, default `false`** ("disabled until Stage 1 diagnostics are
  stable", gb/mod.rs:103-110). Ours has none — acceptable for the default
  config (see Quirks).
- **DRIFT — CRLF handling.** Grok matches against an LF-normalized copy when
  the file contains `\r\n` (gb/mod.rs:553-563) and writes back with all line
  endings re-expanded to CRLF (`new_text.replace("\r\n","\n").replace('\n',"\r\n")`,
  gb/mod.rs:688-692). Ours matches and writes raw bytes (ours :129-151), so an
  LF-only `old_string` fails to match a CRLF file where grok succeeds.

### Pre-flight validation — DRIFT (missing guards)

Grok, in order, before any matching:

1. Path-component length ≤ 255 → `"Error: file name exceeds the 255-character limit ({N} characters). Please use a shorter file name."`
   (gb/mod.rs:165-167, 250-271). Ours: absent.
2. Directory target → `"File path is a directory"` (gb/mod.rs:168-172); a
   directory hit at read time → `"Error: {file_path} is a directory, not a file."`
   (gb/mod.rs:531-533). Ours: absent (host read error string passes through,
   ours :127).
3. Gitignore guard (current version only): editing a gitignored file →
   `"Error: {file_path} is ignored by .gitignore and cannot be edited."`
   (gb/mod.rs:173-186; default on, `RespectGitignore` absent ⇒ true,
   gb/mod.rs:176). Ours: absent.
4. No-op edit → `"Old string and new string are the same"`
   (gb/mod.rs:187-191). Ours: identical text (ours :81-86). **MATCH** —
   ordering differs though: grok checks this *before* touching the file
   system's existence, after path checks; ours checks it first (ours :81),
   before path resolution — same observable text for the common case.

### Empty-`old_string` / file-creation semantics — DRIFT

- Both treat empty `old_string` as "create file" (gb/mod.rs:203-216;
  ours :97-116).
- **DRIFT — overwrite semantics.** Grok's default
  (`empty_old_string_does_not_override: false`, gb/mod.rs:99-102) **allows an
  empty `old_string` to completely overwrite an existing non-empty file**
  (gb/mod.rs:293, guard only fires when the param is true; test
  `empty_old_string_overrides_existing_file_by_default`, gb/mod.rs:1264-1282).
  Ours hard-errors on any non-empty existing file (ours :97-106).
- **DRIFT — guard error text.** Even in grok's guarded config the message is
  `"{old_string_param} is empty, which is only allowed when creating a new file or when the file is empty."`
  (gb/mod.rs:302-305, rendered: "old_string is empty, …"). Ours invented:
  `"File already exists: {display}. To edit it, provide the exact text to replace in old_string."`
  (ours :103-105).
- **DRIFT — creation success text.** Grok:
  `"The file {file_path} has been created successfully."` using the
  model-supplied path (gb/mod.rs:358-361). Ours: `"Created {resolved_abs_path}."`
  (ours :55-56).
- Grok's create path also maps write errors: missing parent dir → the
  not-found message (gb/mod.rs:309-321); path component is a file →
  `"Error: cannot create {file_path}. A component of the path already exists as a file where a directory is expected."`
  (gb/mod.rs:322-325). Ours: host error `to_string()` passthrough (ours :110).
  UNVERIFIED whether our host auto-creates parent dirs (host behavior not in
  scope of this file read); either way the error texts are not reproduced.

### Error texts (edit path) — DRIFT (ours = legacy wording)

| Case | Grok current (default) | Grok legacy-0.4.10 | Ours |
|---|---|---|---|
| File not found | `"Error: {display_path} does not exist."` (+ optional cwd/did-you-mean hints when `PathNotFoundHints` on; gb/mod.rs:517-529, gb-hints :113-126, base text :120) | `"File not found: {file_path}. Please check the path and try again."` (`gb/versions/legacy_0_4_10.rs:33-38`; exact-match test gb/mod.rs:1008-1016) | `"File not found: {file_path}. Please check the path and try again."` (ours :122-125) — **matches legacy, not current** |
| No match | `"The string to replace was not found in the file, use the {read_tool} tool to see the correct string. The user may have changed the file since you last read it.{nearest_hint}{confusable_hint}"` — user-edit hint default **on** (gb/mod.rs:111-117, 639-653); nearest hint `"\n\nNearest match: line {N}: {line}"` capped at 200 chars (gb/mod.rs:385-409); confusable diagnostic when the miss is explained by smart quotes/em-dashes (gb/mod.rs:423-499) | `"The string to replace was not found in the file, use the read_file tool to see the correct string."` (`legacy_0_4_10.rs:43-46`; test gb/mod.rs:1228-1236) | `"The string to replace was not found in the file, use the read_file tool to see the correct string."` (ours :132-136) — **matches legacy, not current** |
| Multiple matches | `"The string to replace was found multiple times in the file. Use {replace_all_param} to replace all occurrences, or include more context to only edit one occurrence."` (gb/mod.rs:655-662) | identical wording (`legacy_0_4_10.rs:39-42`) | identical (ours :137-141) — **MATCH** (wording is the same in both grok eras) |
| Same old/new | `"Old string and new string are the same"` (gb/mod.rs:187-191) | same | identical (ours :83-85) — **MATCH** |

### Success/confirmation output — DRIFT

Grok (gb/mod.rs:724-741), always keyed on the **model-supplied** `file_path`,
never a count:

- single replacement: `"The file {file_path} has been updated successfully."`
- replace_all with >1 hits: `"The file {file_path} has been updated. All occurrences were successfully replaced."`
- creation: `"The file {file_path} has been created successfully."` (gb/mod.rs:358-361)

(The concise toolset variant swaps in `"…has been updated." / "…All occurrences
were replaced." / "…has been created."` — `grok_build_concise/search_replace.rs:94-98`,
gb/mod.rs:362, 729, 736-739.)

Ours (ours :53-64): `"Created {abs_path}."` / `"Replaced {N} occurrence(s) in {abs_path}."`
— wrong wording, resolved-absolute path instead of the model's path, and leaks
a replacement count grok never reports.

### Not model-visible (no port required)

- `FileWritten` notifications (gb/mod.rs:342-357, 710-716) — UI/host seam.
- `SearchReplaceEditDetail` context snippets, `line_prefix`, `patch: None`
  (gb/helpers.rs:97-128, gb/mod.rs:742-753) — structured output for clients,
  not rendered to the model (gb-out :738-741 renders only
  `tool_output_for_prompt`).
- `edit.lines` tracing span (gb/mod.rs:233-247).
- Read-before-edit: **no runtime guard exists** — `skip_read_before_edit` is a
  "Deprecated runtime no-op" (gb/mod.rs:95-98); the Read-tool requirement is
  config-time toolset validation only (gb/mod.rs:768-781). Consecutive edits
  without reads succeed (test gb/mod.rs:912-945). Ours correctly has no guard.

## Quirks

- **`old_string == new_string` is rejected even when the strings would not
  match anything** — the check runs before file read (gb/mod.rs:187-191).
  Reproduced (ours :81-86).
- **Empty `old_string` overwrites existing files by default** — grok's
  "create" is really "write file", guard off by default (gb/mod.rs:99-102, 293).
- **Success text uses the path exactly as the model typed it** (relative stays
  relative), not the canonicalized path (gb/mod.rs:358-361, 724-741).
- **Multi-match error text is identical in both grok eras** — safe anchor.
- **Lenient booleans**: `"yes"`/`"1"`/`1`/`null` are accepted for
  `replace_all` (`serde_lenient.rs:12-60`).
- **CRLF files are matched LF-normalized and written back fully CRLF** — a
  mixed-endings file comes out uniformly CRLF after an edit (gb/mod.rs:553-559,
  688-692; tests gb/mod.rs "crlf_mixed_line_endings").
- Config-gated, default-off, **not** required for a faithful default port:
  Unicode-normalized fallback replacement (`unicode_normalized_fallback:
  false`, gb/mod.rs:103-110), path-not-found suggestion hints
  (`PathNotFoundHints` resource, gb/mod.rs:152, gb-hints :121-123). The
  **confusable diagnostic hint** and **nearest-match hint** on no-match are
  NOT gated — they are always active in the current version (gb/mod.rs:621-638).
- UNVERIFIED: which template renderings ship in xAI's production harness
  config (repo tests and the grok_build toolset consistently use
  `read_file`/`old_string`/`replace_all` — gb/mod.rs:842-853 — and our pack
  registers `read_file` at `grok/mod.rs:39`, so the rendered forms above are
  the right target).

## Fixing task

Scope estimate: **M** (one file plus tests; the hint builders are the only
genuinely new logic).

Acceptance criteria:

1. Tool description replaced with the rendered `DESCRIPTION_FULL` 3-bullet
   text (gb/mod.rs:59-63 with `read_file`/`old_string`/`replace_all`
   substituted), verbatim including the `LINE_NUMBER→` bullet.
2. ~~`replace_all` accepts grok's lenient boolean forms (`serde_lenient.rs:12-60`)~~
   — **DECLINED (explicit user call, 2026-07-20): not ported.** `replace_all`
   stays a strict `bool` (`#[serde(default)]`); string/number/null forms
   (`"true"`, `"yes"`, `1`, `null`) get a deserialization error instead of
   grok's coercion. Recorded deviation from schema issue 2 — a deliberate
   exception to pack faithfulness, per the AGENTS.md rule that such exceptions
   are noted explicitly.
3. Empty `old_string` overwrites an existing non-empty file (default grok
   behavior, gb/mod.rs:293 + test :1264-1282); the invented
   "File already exists…" error is removed.
4. Success texts match grok verbatim: `"The file {file_path} has been created
   successfully."`, `"The file {file_path} has been updated successfully."`,
   `"The file {file_path} has been updated. All occurrences were successfully
   replaced."` — using the model-supplied `file_path`, no occurrence counts in
   the prompt text (counts may stay in the structured report face).
5. No-match error upgraded to the current-era composite:
   base text + `" The user may have changed the file since you last read it."`
   (default-on) + nearest-match hint (`"\n\nNearest match: line {N}: {line}"`,
   longest-token-of-first-line heuristic, 200-char cap — port
   `build_nearest_match_hint`, gb/mod.rs:385-409). Confusable diagnostic
   (gb/mod.rs:423-499) may be deferred to a follow-up with an explicit
   deviation note, since it drags in the `unicode_confusables` table — if
   deferred, document it in this file.
6. File-not-found error becomes `"Error: {path} does not exist."` (hints
   remain off, matching grok's un-hinted default, gb-hints :120-123).
7. CRLF semantics ported: match against LF-normalized content when the file
   contains `\r\n`, write back with all `\n` expanded to `\r\n`
   (gb/mod.rs:553-559, 688-692).
8. Pre-flight guards ported with exact texts: directory target
   (`"File path is a directory"`), 255-char path-component limit
   (`"Error: file name exceeds the 255-character limit ({N} characters).
   Please use a shorter file name."`). Gitignore guard: port iff the pack/host
   exposes a gitignore filter seam; otherwise record as a documented deviation
   here (host-seam constraint — ADR-0008 puts policy in the host jail, and the
   pack currently has no gitignore resource).
9. Host-seam constraints respected: all reads/writes stay on `locode_host`
   (`resolve_in_jail`/`read_file`/`write_file`); no direct fs access; every
   error returned as `ToolError::Respond` so the dispatch loop pairs the
   `tool_use` with an error `tool_result` (mirrors gb-out :669-674 where every
   non-`EditsApplied` variant is an error).
10. Tests cover: creation, overwrite-by-empty-old-string, unique replace,
    replace_all, multi-match error text, no-match text incl. user-edit hint and
    nearest-match hint, CRLF roundtrip, strict `replace_all` parsing (a
    string-typed value errors — pins the declined-deviation behavior), same
    old/new rejection — asserting exact strings (grok's own tests do:
    gb/mod.rs:1008-1016, 1173-1180, 1228-1236).

## Plan finalization (user interview, 2026-07-21)

- **Target version resolved (Q2): current-default grok** — port exactly what a
  default-configuration session exhibits. Grok's DI/config toggles become
  constructor-time constants frozen at grok defaults with citation comments
  (no resource bag): user-edit hint ON (`mod.rs:110-115`), overwrite-by-empty-
  old-string ON (`empty_old_string_does_not_override = false`, `mod.rs:99-102`),
  nearest-match hint per its call-site gating.
- **Default-off subsystems are not ported** (unreachable at default config;
  each a one-line recorded deviation): path-not-found hints
  (`PathNotFoundHints` default false) and the confusable normalized-fallback
  matching (`unicode_normalized_fallback` default false, `mod.rs:103-109`).
  **Verify at implementation:** whether the confusable *diagnostic message*
  (`mod.rs:410-417`) is gated with the fallback or unconditional; port
  accordingly and size it before writing.
- **Gitignore guard** (default ON via `RespectGitignore`, `mod.rs:176`) ports
  via the new host `is_path_ignored` API — no pack fs access.
- Architecture confirmed: all hint injection is in-memory string logic inside
  the pack over existing `host.read_file`/`write_file` calls; no seam changes.
