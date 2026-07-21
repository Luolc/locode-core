# read_file — fidelity audit vs Grok Build

Ours: `crates/locode-packs/src/grok/read.rs` (registered as `read_file` in
`crates/locode-packs/src/grok/mod.rs:39`).
Original: `submodules/grok-build/crates/codegen/xai-grok-tools/src/implementations/grok_build/read_file/mod.rs`
(referred to below as `gb`), plus shared helpers under
`…/src/implementations/read_file/` (`pdf.rs`, `image.rs`, `metadata.rs`),
`…/src/types/schema.rs`, `…/src/types/context.rs`, `…/src/util/binary.rs`,
`…/src/util/truncate.rs`, `…/src/util/path_suggestions.rs`. Grok has a
`legacy-0.4.10` behavior version (`gb …/read_file/versions/legacy_0_4_10.rs`);
this audit targets the **Current** version (`gb:40-54`).

## Verdict

DRIFT (4 schema issues, 10 behavior issues) — arg descriptions for the three
ported fields are verbatim-faithful, but `pages`/`format` are missing, the
integer schema shape differs, the tool description is fully rewritten, and the
output projection (sparse line numbering), negative offset, error texts, and
non-text file handling all diverge.

## Schema comparison

Wire names include serde renames. Grok's struct is `ReadFileInput`
(`gb:111-144`); ours is `ReadFileArgs` (`read.rs:31-48`). Neither side sets
`deny_unknown_fields` on the input struct (grok's `ReadFileParams` config
struct at `gb:27-32` does, but that is tool *config*, not the wire input).

| Wire field | Grok type / default / description (citation) | Ours (citation) | Status |
|---|---|---|---|
| `target_file` | `String`, required. Rust field `path` with `#[serde(rename = "target_file")]` (`gb:113`). Description: "The path of the file to read. You can use either a relative path in the workspace or an absolute path. If an absolute path is provided, it will be preserved as is." (`gb:114-116`) | `String`, required, same `#[serde(rename = "target_file")]` on field `path` (`read.rs:33`). Description byte-identical (`read.rs:34-36`). | MATCH |
| `offset` | `Option<i64>`, default `None`. `#[serde(default, deserialize_with = "crate::types::schema::deserialize_lenient_i64", skip_serializing_if = …)]` (`gb:118-122`); schema forced to bare `{"type":"integer"}` via `#[schemars(with = "GrokIntegerSchema")]` (`gb:124`, `schema.rs:7-15`). Description: "The line number to start reading from. Only provide if the file is too large to read at once." (`gb:125`) — grok pins this string with a source-grep test (`gb:2258-2266`). | `Option<i64>`, `#[serde(default)]` (`read.rs:38-42`). Description byte-identical (`read.rs:40`). No `GrokIntegerSchema` equivalent: schemars' default `i64` schema carries a `format` annotation grok deliberately suppresses ("By default schemars adds \"format\": \"uint\" and \"minimum\": 0.0 which we don't want." — `schema.rs:4`). No lenient coercion: grok accepts `-3`, `"42"`, `"-3"`, `100.0` (`schema.rs:130-141`; tests `gb:2336-2344`, `schema.rs tests:326-346`); ours rejects string/float offsets. | DESCRIPTION MATCH; schema-shape + coercion DRIFT |
| `limit` | `Option<usize>`, default `None`. `#[serde(default, skip_serializing_if = …)]` (`gb:128`); schema `{"type":"integer"}` via `GrokIntegerSchema` (`gb:130`) — note: **no** lenient deserializer on `limit` (only `offset` has one). Description: "The number of lines to read. Only provide if the file is too large to read at once." (`gb:131`) | `Option<usize>`, `#[serde(default)]` (`read.rs:43-47`). Description byte-identical (`read.rs:45`). Schemars default for `usize` emits `format: "uint", minimum: 0` (exactly what `schema.rs:4` says grok suppresses). | DESCRIPTION MATCH; schema-shape DRIFT |
| `pages` | `Option<String>`, default `None`. `#[serde(default, skip_serializing_if = …)]` (`gb:134`). Fully schema-visible — no `schemars(skip)` anywhere on the struct. Description: "Page range for PDF files (e.g. '1-5', '3', '10-'). Required for PDFs with more than 10 pages. Max 20 pages per call. Ignored for non-PDF files." (`gb:135-137`) | Absent (`read.rs:31-48`; header comment "`pages`/`format` dropped in v0", `read.rs:30`). | MISSING |
| `format` | `Option<String>`, default `None`. `#[serde(default, skip_serializing_if = …)]` (`gb:139`). Schema-visible. Description: "Output format for PDF files. 'image' (default) renders pages as images. 'text' extracts text content. Ignored for non-PDF files." (`gb:140-142`) | Absent (`read.rs:31-48`). | MISSING |

Schemars-skip check: there are **no** `schemars(skip)` quirks on
`ReadFileInput` — all five fields, including `pages` and `format`, appear in
the wire schema (`gb:111-144`). The only schema quirk is `GrokIntegerSchema`
flattening integers to `{"type":"integer"}` (`schema.rs:3-15`).

## Tool description comparison

Grok (`DESCRIPTION_FULL`, `gb:103-110`), verbatim:

```
Read a file.

Usage:
- The target_file parameter can be a relative path in the workspace or an absolute path
- By default, it reads up to {max_lines_read} lines starting from the beginning of the file
- Results are returned with line numbers starting at 1. The format is: LINE_NUMBER→LINE_CONTENT
- This tool can read PDF files (.pdf), PowerPoint files (.pptx), Jupyter notebooks (.ipynb files), and image files (e.g. PNG, JPG, etc).
- When reading an image file the contents are presented visually as this tool uses multimodal LLMs.
```

On the wire, `{max_lines_read}` is interpolated to the configured cap —
default `1000` (`registry/types.rs:1115-1121` →
`context.rs:76-91`, `context.rs:3` `MAX_LINES_READ_DEFAULT: usize = 1_000`).

Ours (`read.rs:81`), verbatim:

```
Read a text file from the workspace, returned as numbered lines (`N→content`).
```

Status: **DRIFT** — a one-line paraphrase replaces grok's five-bullet
template; the interpolated 1000-line default, the `LINE_NUMBER→LINE_CONTENT`
format statement, and the PDF/PPTX/ipynb/image claims are all absent.

## Behavior comparison

- **offset semantics (1-based, 0, negative = tail).** Grok: 1-based;
  `offset == 0` resolves to line 1 (`gb:158-161`, test `gb:2280-2282`).
  **Negative offset is tail-read**: start = `total_fields + offset + 1`,
  clamped to ≥1, where `total_fields` is the `split('\n')` field count plus a
  phantom field when the file is non-empty without a trailing `\n`
  (`gb:150-170`). So `-3` on `"a\nb\nc\n"` → start line 2 (`gb:2268-2270`);
  `-2, limit 2` on a 5-line file yields `"5→line5\n"` (`gb:2284-2289`);
  `-999` clamps to 1 (`gb:2275-2278`); `-1` on `"a\nb\nc"` lands on the
  phantom-only field and returns empty content (`gb:2320-2335`). Negative
  offsets are never echoed back on the output — `stored_read_offset` maps
  them to `None` (`gb:171-176`, `gb:2345-2351`). Ours: 1-based, but negative
  offset is a hard soft-error `"negative offset is not supported in v0"`
  (`read.rs:86-90`); `0` clamps to 1 via `.max(1)` (`read.rs:91`). DRIFT
  (known gap, now with the exact target semantics).
- **Default read window.** Grok: effective limit =
  `input.limit.unwrap_or(usize::MAX).min(max_lines)` with `max_lines` = 1000
  by default (`gb:449-456`, `context.rs:3,34-36`) — i.e. whole file capped at
  1000 lines, and a caller-supplied `limit` above 1000 is clamped. Ours:
  `args.limit.unwrap_or(MAX_LINES).min(MAX_LINES)` with `MAX_LINES = 1_000`
  (`read.rs:16,94`). MATCH (modulo grok's config override, host-seam).
- **Line-number projection — SPARSE, not per-line.** Grok writes the
  `N→content` prefix **only** on the first visible line and on lines whose
  number is a multiple of 10; all other lines are emitted bare
  (`gb:249-256`). Verified by grok's own test: 12 lines render as
  `"1→L1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\nL9\n10→L10\nL11\nL12\n"`
  (`gb:2302-2310`); an offset window starts with its own anchor, e.g.
  `"3→c\nd\ne\n"` (`gb:2296-2301`). Ours numbers **every** line
  (`read.rs:114-122`, asserted in `mod.rs:163-164` tests). DRIFT.
- **Trailing phantom line.** Grok counts the empty segment after a final
  `\n` as a line: `total_lines = matches('\n').count() + 1` (`gb:442`), and
  the extraction emits an anchored `"N→"` for it when it falls in the window
  (`gb:258-274`; `"hello\n"` with `offset=2` yields exactly `"2→"`,
  `gb:2314-2319`). Ours uses `str::lines()` so `"alpha\nbeta\ngamma\n"` is 3
  lines with no trailing anchor (`read.rs:108-109`; test `mod.rs:160`
  expects `lines == 3` — grok would say 4). DRIFT.
- **Per-line char truncation.** Grok: **none, deliberately** — "There is
  deliberately no per-line cap: clipping long lines silently corrupts
  single-line files … grok_build `read_file` never clips lines"
  (`context.rs:8-14,73`). Ours: none (`read.rs:113-122`). MATCH.
- **Total cap (tokens).** Grok: after extraction, `estimate_tokens(content)`
  (bytes/4, rounding down — `util/truncate.rs:190-191`, tests
  `truncate.rs:342-359`) must not exceed `MAX_NUM_TOKENS = 25_000`
  (`gb:55,463-464`). On overflow it returns `FileTooLarge` with one of two
  messages (`gb:487-509`): with a range specified —
  "The requested line range (offset={off}, limit={lim}) contains {n} tokens, which exceeds the maximum allowed tokens (25000 tokens).\nTry a smaller `limit`, a different starting `offset`, or use the '{grep}' tool to search for specific content." —
  and without —
  "File content ({n} tokens) exceeds maximum allowed tokens (25000 tokens).\nPlease use offset and limit parameters to read a shorter range, or use the '{grep}' to search for specific content."
  (note grok's own missing word "tool" in the second variant, `gb:503-508`);
  `{grep}`/`{execute}` are resolved tool names via the template renderer
  (`gb:465-475`). When the overflowing window is a single content line, a
  hint is appended steering to the execute tool (`jq`, `python3`, `cut -c`)
  (`gb:476-485`, tests `gb:2244-2256`). Ours: same 25k threshold and /4
  heuristic (`read.rs:17,125`) but a different message —
  "file is too large (~{n} tokens > 25000); read a narrower range with offset/limit, or use grep" (`read.rs:126-129`) — and no single-line hint, no
  offset/limit echo. DRIFT (message text + variants).
- **SKILL.md exemption.** Grok: files named exactly `SKILL.md` ignore
  offset/limit and the token cap entirely (`gb:328-331,449-456,464,512-516`).
  Ours: no exemption. DRIFT.
- **pages/format — what they DO.** PDF-only; ignored for other files (they
  are simply not consulted outside `handle_pdf`, `gb:397-412`).
  `format`: `None`/`"image"` → render pages as 150-DPI JPEG q85 images,
  base64, returned as a `PdfPageImages` output with per-page
  `page_number`/`mime_type` (`pdf.rs:13-15,217-245`); `"text"` → extract text
  with `--- Page N ---` headers, projected through
  `raw_text_to_file_content` (every line numbered `N→…` — the PDF text path
  does NOT use the sparse decade numbering; `pdf.rs:247-267,299-301,339-346`);
  any other value → error "Invalid format '{other}'. Supported values: 'image' (default), 'text'." (`pdf.rs:85-94`).
  `pages`: comma-separated 1-based pages and ranges incl. open-ended
  (`"1-5"`, `"3"`, `"10-"`), parsed/deduped (`pdf.rs:115-176`); errors:
  "invalid page number: '{s}'", "page {p} out of range (document has {n} pages)",
  "invalid page range: {a}-{b} (start must be ≤ end)",
  "requested {n} pages, maximum is 20 per call" (`PDF_MAX_PAGES_PER_READ = 20`,
  `pdf.rs:19,165-171`), "no pages specified". Without `pages`, PDFs over the
  10-page auto-read threshold error:
  "PDF has {n} pages which exceeds the 10 page auto-read limit. Use the `pages` parameter to specify which pages to read (e.g. pages=\"1-5\"). Maximum 20 pages per call." (`pdf.rs:13,199-212`).
  PDF caps: 50 MB (`MAX_PDF_BYTES`, `pdf.rs:12`), 60 s extraction timeout
  (`pdf.rs:16`), size/timeout errors from `run_document_extraction`
  (`pdf.rs:33-39,62-66`). PDF detection is three-tier: inferred MIME, magic
  `%PDF-`, or `.pdf` extension (`pdf.rs:349-353`, `metadata.rs:20-24`).
  Ours: neither field exists, no PDF path at all (`read.rs` whole file).
  DRIFT (known-dropped).
- **Image handling.** Grok: magic-byte MIME inference (`infer` crate,
  `metadata.rs:27-37`); any `image/*` file short-circuits to a multimodal
  `ImageContent` (`gb:383-391`) after compression to ≤768 KB base64
  (`MAX_IMAGE_PAYLOAD_BYTES`, `image.rs:26`), area budget 1,048,576 px, side
  clamp 2000 px, JPEG quality ladder [85, 70, 50, 40]
  (`image.rs:33-43`); failures return
  "Could not embed image in conversation: {e}" (`image.rs:84-88`). PPTX files
  get zip/DrawingML text extraction (50 MB / 60 s, `gb:77-101`). Ours:
  text-only by design (`read.rs:1-3`); no image or PPTX path. DRIFT
  (known-dropped scope, but not listed in the v0 note, which names only
  `pages`/`format` — `read.rs:30`).
- **Binary handling.** Grok: rejects with
  "Cannot read binary file: {path}" when the extension is in a 46-entry
  sorted list (`binary.rs:5-10` — `binary_search`, so the list must stay
  sorted) or the first 8 KiB contains a NUL byte or >30% control bytes
  (0-8, 14-31) (`binary.rs:12-43`; `gb:416-427`). Non-binary non-UTF-8 falls
  through `from_utf8_lossy` (`gb:428`). Ours: no binary detection; whatever
  `host.read_file` returns is projected as text (`read.rs:102-108`).
  UNVERIFIED how our host layer treats non-UTF-8 bytes (not examined here);
  the tool itself has no rejection path. DRIFT.
- **Missing-file / error texts (and error channel).** Grok returns errors as
  **soft output variants** (`Ok(ReadFileOutput::…)`), not tool errors:
  not-found → "Error: {display_path} does not exist."
  (`path_suggestions.rs:113-126`), plus optional did-you-mean/similar-file
  hints when the `PathNotFoundHints` resource is on (`gb:312,358-368`);
  directory → "Error: {path} is a directory, not a file." (`gb:369-372`);
  permission → "Permission denied: {path}" (`gb:373-375`); other →
  "Failed to read file: {path}, {e}" (`gb:376-380`); gitignored (when
  `RespectGitignore`) → "Error: {path} is ignored by .gitignore and cannot be read." (`gb:332-345`). (Legacy-0.4.10 collapses all of these to
  "Failed to read file: {path}" and allows gitignored reads —
  `versions/legacy_0_4_10.rs:19-26`.) Ours: every failure is
  `ToolError::Respond(host_error_text)` (`read.rs:97-106`), surfaced as
  `is_error: true` (`mod.rs:193-218` tests assert `!record.ok`). Both text
  and channel drift.
- **Out-of-range offset.** Grok: an offset past EOF yields empty content (or
  a lone phantom anchor), not an error (`gb:216-274,2320-2335`). Ours: empty
  body, no error (`read.rs:110-122`) — channel MATCH, but ours lacks the
  anchor quirk above.
- **Empty file.** Grok: `FileContent` with empty `content`, `total_lines: 0`
  (`gb:429-441`). Ours: empty body, `lines: 0`, ok (`read.rs:108-109`).
  MATCH.
- **Path resolution.** Grok joins against cwd via `resolve_model_path`,
  preserving absolute paths as-is (`gb:314`; schema description `gb:115`),
  canonicalizes, and falls back to Unicode-normalized filename resolution on
  NotFound (`gb:315-324`). No jail. Ours resolves through the host jail and
  rejects paths outside the workspace (`read.rs:96-101`; test
  `mod.rs:206-218`). Host-seam divergence (deliberate; locode-host owns
  sandboxing), but note the schema description we ship still promises
  absolute paths "will be preserved as is".
- **Base64-image lines in text files.** Grok scans each emitted line for
  embedded base64 images and lifts them into multimodal follow-ups
  (`gb:186-188,242-248`). Ours: none. DRIFT (minor, multimodal-dependent).
- **Streaming.** Grok streams the formatted projection as ≤4 KiB
  char-aligned deltas when the client opts in (`gb:61-76,584-619`).
  Loop-adjacent (session layer), out of pack scope per the fidelity boundary.

## Quirks

1. **Sparse decade numbering** is the single most surprising quirk: the tool
   description says "Results are returned with line numbers starting at 1"
   (`gb:108`) but the code numbers only the first visible line and every
   10th line (`gb:249-256`, test `gb:2302-2310`). Faithful port must
   reproduce the sparse form, description and all.
2. **Phantom trailing line**: `total_lines` counts the empty segment after a
   trailing `\n`; a window landing only on it emits a bare `"N→"` anchor
   (`gb:213,258-274,2314-2319`).
3. **Asymmetric leniency**: `offset` accepts strings and whole floats via
   `deserialize_lenient_i64`; `limit` does not (`gb:118-133`).
4. **`GrokIntegerSchema`** flattens integer fields to `{"type":"integer"}`,
   suppressing schemars' `format`/`minimum` (`schema.rs:3-15`).
5. **Negative offsets are resolved but never stored**: output echoes
   `offset: None` for them (`gb:171-176,2345-2351`).
6. Grok's own second too-large message reads "or use the '{grep}' to search"
   — the word "tool" is missing (`gb:506-507`). Faithful means keeping the typo.
7. CRLF: line strip removes `\n` then `\r` (`gb:196-204`); `raw_output`
   normalizes a trailing `\r\n` to `\n` (`gb:279-283`).
8. `BINARY_EXTENSIONS` is consulted via `binary_search` — it only works
   because the list is sorted (`binary.rs:5-10,20-21`).
9. Errors are **soft outputs**, not protocol errors — the model sees them as
   ordinary tool results (`gb:340-380,423-426,510`).
10. Description template placeholder `{max_lines_read}` is interpolated at
    registry time (default 1000) — the source constant is not inlined
    (`gb:107`, `registry/types.rs:1115-1121`, `context.rs:76-91`).

## Fixing task

Host-seam constraints: locode tools go through `locode-host` for all fs
access (no direct fs), the jail stays (grok's no-jail absolute-path behavior
is a deliberate deviation — record it per pack notes), multimodal
tool-result content requires `ResultChunk::Image` support end-to-end
(protocol + wire), and streaming stays out (loop-adjacent, per the fidelity
boundary). PDF/PPTX rendering would add dependencies (`pdf_oxide`, zip/XML,
`image`, `infer`) — "ask first" per AGENTS.md before pulling any of them in.

Acceptance criteria:

1. Wire schema exposes `pages` (`Option<String>`) and `format`
   (`Option<String>`) with grok's verbatim descriptions (`gb:135-137,140-142`),
   or — if PDF support stays out of v0 — the drop is recorded in the pack's
   fidelity notes/ADR with these citations, not just a code comment.
2. `offset` and `limit` schemas render as bare `{"type":"integer"}` (port a
   `GrokIntegerSchema` equivalent; `schema.rs:3-15`). ~~`offset` accepts
   lenient values (whole float, numeric string) via `deserialize_lenient_i64`~~
   — **DECLINED (explicit user call, 2026-07-20): type-strict.** `offset`
   stays a plain `Option<i64>`: negative *integers* still work (the tail-read
   semantics of criterion 4 are unaffected — that's typed-value handling, not
   coercion), but `"42"`/`100.0` forms error instead of coercing
   (grok: `schema.rs:130-141`). Recorded deviation; a test pins the strict
   rejection.
3. Tool description equals `DESCRIPTION_FULL` (`gb:103-110`) with
   `{max_lines_read}` interpolated to 1000, trimmed only of claims for
   behavior we genuinely don't ship (if PDF/PPTX/image stay out, the bullet
   list must be adjusted and the deviation logged — do not ship claims the
   tool can't honor).
4. Negative offset implements grok's tail semantics: start =
   `split('\n')`-field count (+ phantom field when non-empty w/o trailing
   `\n`) + offset + 1, clamped to ≥1; `0` → 1; resolved-but-not-stored
   (`gb:150-176`). Tests reproduce `gb:2268-2289,2320-2335`.
5. Output projection reproduces sparse numbering: `N→` on the first visible
   line and every line number divisible by 10, bare lines otherwise;
   trailing phantom line anchored `"N→"` when in-window; empty-window
   anchor case (`gb:216-274`; golden tests mirroring `gb:2290-2319`).
6. `lines`/total-lines counting switches to `matches('\n') + 1` semantics
   (`gb:442`) so the structured face agrees with grok's `total_lines`.
7. Too-large handling emits grok's two message variants verbatim (incl. the
   missing-"tool" typo) with offset/limit echo, resolved grep/execute tool
   names, and the single-long-line hint (`gb:463-509`); token estimate =
   bytes/4 rounded down (`truncate.rs:190-191`).
8. Error texts match grok's Current variants verbatim — not-found
   ("Error: {path} does not exist."), is-a-directory, permission-denied,
   generic read failure (`gb:357-380`, `path_suggestions.rs:120`) — and are
   delivered as soft (non-protocol-error) results; decide and document
   whether `record.ok` stays false for them (our envelope) while the prompt
   face carries grok's text.
9. Binary rejection: port `BINARY_EXTENSIONS` + NUL/control-ratio sniff and
   the "Cannot read binary file: {path}" text (`binary.rs`, `gb:416-427`);
   text decode goes through `from_utf8_lossy` (`gb:428`).
10. `SKILL.md` exemption from window and token caps (`gb:328-331,449-464`)
    — or an explicit recorded deviation if skills are out of scope for the
    grok pack in v0.
11. Optional/deferred (each needs an explicit deviation note if skipped):
    image multimodal output with compression caps (`image.rs:26-50`), PDF
    `pages`/`format` execution (`pdf.rs:79-245`), PPTX extraction
    (`gb:77-101`), gitignore enforcement (`gb:332-345`), not-found hints
    (`path_suggestions.rs:105-126`), base64-image line extraction
    (`gb:242-248`).

Scope: **M** for the faithful text path (criteria 1-10 — schema shape,
description, negative offset, sparse numbering, caps/messages, binary
sniff; all pure-Rust, no new deps except possibly `infer`); **L** if the
multimodal/PDF/PPTX tier (criterion 11) is pulled in, since it adds
dependencies and protocol-level image results.

## Split: immediate vs deferred (user decision, 2026-07-20)

**Deferred — binary/image/PDF/PPTX handling** (user call, recorded here as the
required deviation note):
- Criterion 9 (binary sniff + "Cannot read binary file" rejection) and
  criterion 11 (image multimodal output, PDF `pages`/`format` execution, PPTX
  extraction, base64-image line extraction). **Known consequence until this
  lands:** reading a binary file emits `from_utf8_lossy` garbage where grok
  errors cleanly — acceptable for text-repo A/B runs, not for mixed-content
  repos.
- `pages`/`format` schema fields stay **immediate** (criterion 1): they are
  fully schema-visible in grok and "Ignored for non-PDF files" by behavior, so
  verbatim schema + ignore-on-text is faithful today; only their PDF execution
  is deferred.

**Immediate (the faithful text path):** criteria 1–8, plus criterion 10
resolved as a recorded deviation (skills are out of scope for the grok pack
v0 — no `SKILL.md` exemption; deviation note in the pack docs). Criterion 3's
description must trim the PDF/PPTX/image bullets per its own rule (never ship
claims the tool can't honor) and log the trim as part of this deferral.

Immediate scope: **M**, pure-Rust, no new dependencies.

## Plan finalization (user interview, 2026-07-21)

- Description trimming confirmed (Q3 = Option A): deferred-capability bullets
  (PDF/PPTX/image) are trimmed from `DESCRIPTION_FULL`, deviation logged —
  never advertise behavior the tool can't honor.
- Sequencing: this tool's text path is host-independent and starts immediately,
  in parallel with the host-groundwork slice.
