//! `read_file` — a faithful port of Grok Build's `ReadFile` tool (`gb/read_file/mod.rs`),
//! current default configuration, text path only.
//!
//! Fidelity notes (Task 26; see `tasks/audits/read_file.md`):
//! - Frozen grok defaults (constants, not config): `max_lines_read` = 1000,
//!   token cap = 25 000, path-not-found hints OFF (`PathNotFoundHints` default
//!   false), no `SKILL.md` exemption (skills out of scope for this pack).
//! - Deferred per user decision 2026-07-20: binary sniffing, image/PDF/PPTX
//!   handling, base64-image line extraction — `pages`/`format` are accepted and
//!   schema-visible (faithful) but their PDF behavior is deferred; non-text
//!   files degrade to lossy UTF-8.
//! - Description is grok's `DESCRIPTION_FULL` (`gb:103-110`) with
//!   `{max_lines_read}` → 1000 and the PDF/PPTX/image bullets trimmed
//!   (recorded deviation — never advertise deferred behavior).
//! - The path jail is our deliberate deviation (grok is jail-less); jail
//!   rejections keep our `PathError` text.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{FsError, Host};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// grok's default line cap for a single read (`gb/read_file/mod.rs:55`,
/// `context.rs:3` `MAX_LINES_READ_DEFAULT`).
const MAX_LINES: usize = 1_000;
/// grok's token cap (`gb/read_file/mod.rs:55` `MAX_NUM_TOKENS`).
const MAX_TOKENS: usize = 25_000;

/// Clean `{"type":"integer"}` schema — grok's `GrokIntegerSchema`
/// (`types/schema.rs:3-15`): suppresses schemars' default `format`/`minimum`
/// annotations on integer fields.
struct GrokIntegerSchema;

impl JsonSchema for GrokIntegerSchema {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "grok_integer_schema".into()
    }
    fn inline_schema() -> bool {
        true // inline: the wire field is bare {"type":"integer"}, no $ref/$defs
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "integer" })
    }
}

/// grok's `read_file` tool.
pub(crate) struct GrokReadFile {
    host: Arc<Host>,
}

impl GrokReadFile {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `read_file` — grok's real `ReadFileInput`, field for field
/// (`gb/read_file/mod.rs:111-144`). Type-strict (no lenient coercion — explicit
/// user deviation, 2026-07-20).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ReadFileArgs {
    #[serde(rename = "target_file")]
    #[schemars(
        description = "The path of the file to read. You can use either a relative path in the workspace or an absolute path. If an absolute path is provided, it will be preserved as is."
    )]
    path: String,
    #[serde(default)]
    #[schemars(
        with = "GrokIntegerSchema",
        description = "The line number to start reading from. Only provide if the file is too large to read at once."
    )]
    offset: Option<i64>,
    #[serde(default)]
    #[schemars(
        with = "GrokIntegerSchema",
        description = "The number of lines to read. Only provide if the file is too large to read at once."
    )]
    limit: Option<usize>,
    /// Accepted + schema-visible (grok has no `schemars(skip)` here); PDF
    /// behavior deferred — ignored for text files, faithful to grok's
    /// "Ignored for non-PDF files".
    #[serde(default)]
    #[schemars(
        description = "Page range for PDF files (e.g. '1-5', '3', '10-'). Required for PDFs with more than 10 pages. Max 20 pages per call. Ignored for non-PDF files."
    )]
    #[allow(dead_code)] // PDF tier deferred; field kept for schema fidelity.
    pages: Option<String>,
    #[serde(default)]
    #[schemars(
        description = "Output format for PDF files. 'image' (default) renders pages as images. 'text' extracts text content. Ignored for non-PDF files."
    )]
    #[allow(dead_code)] // PDF tier deferred; field kept for schema fidelity.
    format: Option<String>,
}

/// The structured (report) face; the numbered body is the prompt face (ADR-0003).
#[derive(Debug, Serialize)]
pub(crate) struct ReadFileOutput {
    /// The jail-resolved absolute path.
    path: String,
    /// Total lines, grok's `matches('\n') + 1` semantics (`gb:442`).
    lines: usize,
    /// Whether the returned window is a subset of the file.
    truncated: bool,
    /// The sparse `N→` projection (prompt face only).
    #[serde(skip)]
    body: String,
}

impl ToolOutput for ReadFileOutput {
    fn to_prompt_text(&self) -> String {
        self.body.clone()
    }
}

/// grok's negative-offset resolution (`resolve_read_start_line`, `gb:150-170`):
/// 1-indexed; `0` → 1; negatives count from the `split('\n')` field total plus
/// a phantom field when non-empty without a trailing `\n`, clamped to ≥ 1.
fn resolve_read_start_line(file_content: &str, offset: Option<i64>) -> usize {
    let offset_raw = offset.unwrap_or(1);
    if offset_raw == 0 {
        return 1;
    }
    if offset_raw > 0 {
        return usize::try_from(offset_raw).unwrap_or(usize::MAX);
    }
    let mut total_fields = file_content.split('\n').count();
    if !file_content.is_empty() && !file_content.ends_with('\n') {
        total_fields += 1;
    }
    let computed = i64::try_from(total_fields).unwrap_or(i64::MAX) + offset_raw + 1;
    usize::try_from(computed.max(1)).unwrap_or(1)
}

/// The extracted window: grok's `ExtractedContent`, minus the deferred
/// base64-image extraction (`gb:178-291`).
struct Extracted {
    /// Sparse-numbered projection (`N→` on the first visible line and every
    /// line number divisible by 10; bare lines otherwise — `gb:249-256`).
    content: String,
    /// Raw unformatted window (single-long-line detection for the token guard).
    raw_output: String,
}

/// grok's `extract_file_content_lines` (`gb:191-291`), ported verbatim minus
/// image extraction.
fn extract_file_content_lines(
    file_content: &str,
    offset: Option<i64>,
    limit: Option<usize>,
    total_lines: usize,
) -> Extracted {
    use std::fmt::Write as _;

    fn strip(s: &str) -> &str {
        let Some(s) = s.strip_suffix('\n') else {
            return s;
        };
        let Some(line) = s.strip_suffix('\r') else {
            return s;
        };
        line
    }

    let mut output = String::new();
    let (mut start, mut end) = (0, 0);
    let mut first_line: Option<usize> = None;
    let split_count = file_content.split_inclusive('\n').count();
    let has_trailing_empty = !file_content.is_empty() && file_content.ends_with('\n');
    let skip = resolve_read_start_line(file_content, offset).saturating_sub(1);
    let take = limit.unwrap_or(usize::MAX);

    if file_content.is_empty() && total_lines > 0 && skip == 0 && take > 0 {
        let _ = write!(&mut output, "1→");
        first_line = Some(1);
    }
    for (i, (pos, line_len, line)) in file_content
        .split_inclusive('\n')
        .scan(0, |pos, line| {
            let out = *pos;
            let line_len = line.len();
            *pos += line_len;
            Some((out, line_len, strip(line)))
        })
        .enumerate()
        .skip(skip)
        .take(take)
    {
        let is_first_visible = first_line.is_none();
        if is_first_visible {
            start = pos;
            first_line = Some(i + 1);
        } else {
            output.push('\n');
        }
        end = pos + line_len;
        let line_num = i + 1;
        if is_first_visible || line_num.is_multiple_of(10) {
            let _ = write!(&mut output, "{line_num}→{line}");
        } else {
            output.push_str(line);
        }
    }
    if has_trailing_empty {
        let trailing_line_idx = split_count;
        if trailing_line_idx >= skip && trailing_line_idx < skip.saturating_add(take) {
            let line_num = trailing_line_idx + 1;
            let is_first_visible = first_line.is_none();
            if is_first_visible {
                first_line = Some(line_num);
            } else {
                output.push('\n');
            }
            if is_first_visible || line_num.is_multiple_of(10) {
                let _ = write!(&mut output, "{line_num}→");
            }
        }
    }
    let mut raw_output = if first_line.is_none() || file_content.is_empty() {
        String::new()
    } else {
        file_content[start..end].to_owned()
    };
    if raw_output.ends_with("\r\n") {
        raw_output.truncate(raw_output.len().saturating_sub(2));
        raw_output.push('\n');
    }
    Extracted {
        content: output,
        raw_output,
    }
}

/// grok's byte/4 token estimate, rounded down (`truncate.rs:190-191`).
fn estimate_tokens(s: &str) -> usize {
    s.len() / 4
}

/// grok's token-overflow message (`gb:463-509`), tool names resolved for the
/// grok pack (`search` → `grep`, `execute` → `run_terminal_cmd`). The
/// no-range variant's missing "tool" word is grok's own text, kept verbatim.
fn too_large_message(
    token_count: usize,
    offset: Option<i64>,
    limit: Option<usize>,
    single_content_line: bool,
) -> String {
    let single_line_hint = if single_content_line {
        "\nNote: the requested read is a single very long line, so line-based offset/limit cannot narrow it further. Use the 'run_terminal_cmd' tool to extract the parts you need (e.g. `jq`, `python3`, or `cut -c`)."
    } else {
        ""
    };
    if offset.is_some() || limit.is_some() {
        let off = offset.map_or_else(|| "1".to_string(), |v| v.to_string());
        let lim = limit.map_or_else(|| "to end".to_string(), |v| v.to_string());
        format!(
            "The requested line range (offset={off}, limit={lim}) contains {token_count} tokens, \
             which exceeds the maximum allowed tokens ({MAX_TOKENS} tokens).\n\
             Try a smaller `limit`, a different starting `offset`, \
             or use the 'grep' tool to search for specific content.{single_line_hint}"
        )
    } else {
        format!(
            "File content ({token_count} tokens) exceeds maximum allowed tokens ({MAX_TOKENS} tokens).\n\
             Please use offset and limit parameters to read a shorter range, \
             or use the 'grep' to search for specific content.{single_line_hint}"
        )
    }
}

/// Map a host read failure to grok's error texts (`gb:357-380`), with the
/// model-supplied path as the display path. Jail rejections keep our text
/// (deliberate deviation — grok is jail-less).
fn read_error_text(display_path: &str, err: &FsError) -> String {
    match err {
        FsError::Io { source, .. } => match source.kind() {
            std::io::ErrorKind::NotFound => format!("Error: {display_path} does not exist."),
            std::io::ErrorKind::IsADirectory => {
                format!("Error: {display_path} is a directory, not a file.")
            }
            std::io::ErrorKind::PermissionDenied => {
                format!("Permission denied: {display_path}")
            }
            _ => format!("Failed to read file: {display_path}, {err}"),
        },
        FsError::Path(_) => err.to_string(),
    }
}

#[async_trait]
impl Tool for GrokReadFile {
    type Args = ReadFileArgs;
    type Output = ReadFileOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        // grok's `DESCRIPTION_FULL` (`gb:103-110`), `{max_lines_read}` → 1000;
        // PDF/PPTX/image bullets trimmed (deferred tier — recorded deviation).
        "Read a file.\n\nUsage:\n- The target_file parameter can be a relative path in the workspace or an absolute path\n- By default, it reads up to 1000 lines starting from the beginning of the file\n- Results are returned with line numbers starting at 1. The format is: LINE_NUMBER→LINE_CONTENT"
    }

    async fn run(&self, ctx: &ToolCtx, args: ReadFileArgs) -> Result<Self::Output, ToolError> {
        let path = Path::new(&args.path);
        let resolved = self
            .host
            .resolve_in_jail(&ctx.cwd, path)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;
        let read = self
            .host
            .read_file(&ctx.cwd, path)
            .await
            .map_err(|e| ToolError::Respond(read_error_text(&args.path, &e)))?;
        let file_content = read.contents;

        // grok's window math (`gb:442-456`): total via `matches('\n') + 1`;
        // limit defaults-and-clamps to the 1000-line cap.
        let total_lines = file_content.matches('\n').count() + 1;
        let effective_limit = Some(args.limit.unwrap_or(usize::MAX).min(MAX_LINES));
        let extracted =
            extract_file_content_lines(&file_content, args.offset, effective_limit, total_lines);

        // Token guard (`gb:463-509`): soft error with grok's exact texts.
        let token_count = estimate_tokens(&extracted.content);
        if token_count > MAX_TOKENS {
            let single_content_line = extracted.raw_output.lines().count() <= 1;
            return Err(ToolError::Respond(too_large_message(
                token_count,
                args.offset,
                args.limit,
                single_content_line,
            )));
        }

        let start = resolve_read_start_line(&file_content, args.offset);
        let limit = args.limit.unwrap_or(usize::MAX).min(MAX_LINES);
        let truncated = start > 1 || (start - 1).saturating_add(limit) < total_lines;
        Ok(ReadFileOutput {
            path: resolved.display().to_string(),
            lines: total_lines,
            truncated,
            body: extracted.content,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // grok's own fixture (`gb:2302-2310`): sparse anchors at first visible +
    // every 10th line.
    #[test]
    fn sparse_numbering_matches_grok_fixture() {
        let file = (1..=12).fold(String::new(), |mut acc, i| {
            use std::fmt::Write as _;
            let _ = writeln!(acc, "L{i}");
            acc
        });
        let extracted = extract_file_content_lines(&file, None, None, 13);
        assert_eq!(
            extracted.content,
            "1→L1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\nL9\n10→L10\nL11\nL12\n"
        );
    }

    // `gb:2296-2301`: an offset window starts with its own anchor.
    #[test]
    fn offset_window_anchors_first_visible() {
        let extracted = extract_file_content_lines("a\nb\nc\nd\ne\n", Some(3), None, 6);
        assert_eq!(extracted.content, "3→c\nd\ne\n");
    }

    // `gb:2268-2289`: negative offsets are tail reads over split('\n') fields.
    #[test]
    fn negative_offset_tail_semantics() {
        assert_eq!(resolve_read_start_line("a\nb\nc\n", Some(-3)), 2);
        assert_eq!(resolve_read_start_line("a\nb\nc\n", Some(-999)), 1);
        assert_eq!(resolve_read_start_line("", Some(0)), 1);
        // Non-empty without trailing newline adds the phantom field: 4 fields.
        assert_eq!(resolve_read_start_line("a\nb\nc", Some(-1)), 4);

        let five = "line1\nline2\nline3\nline4\nline5\n";
        let extracted = extract_file_content_lines(five, Some(-2), Some(2), 6);
        assert_eq!(extracted.content, "5→line5\n");
    }

    // `gb:2320-2335`: a start landing on the phantom-only field is empty.
    #[test]
    fn phantom_only_window_is_empty() {
        let extracted = extract_file_content_lines("a\nb\nc", Some(-1), None, 3);
        assert_eq!(extracted.content, "");
    }

    #[test]
    fn empty_file_renders_single_anchor() {
        let extracted = extract_file_content_lines("", None, Some(1_000), 1);
        assert_eq!(extracted.content, "1→");
    }

    #[test]
    fn schema_has_all_five_fields_with_bare_integers() {
        let schema = serde_json::to_value(schemars::schema_for!(ReadFileArgs)).unwrap();
        let props = schema["properties"].as_object().unwrap();
        for key in ["target_file", "offset", "limit", "pages", "format"] {
            assert!(props.contains_key(key), "missing schema field {key}");
        }
        // GrokIntegerSchema: bare integer, no format/minimum annotations.
        for key in ["offset", "limit"] {
            let field = props[key].as_object().unwrap();
            assert_eq!(field.get("type").and_then(|t| t.as_str()), Some("integer"));
            assert!(!field.contains_key("format"), "{key} carries format");
            assert!(!field.contains_key("minimum"), "{key} carries minimum");
        }
    }

    // Type-strict deviation (user call 2026-07-20): no lenient coercion.
    #[test]
    fn offset_rejects_string_and_float_forms() {
        for bad in [
            serde_json::json!({"target_file": "f", "offset": "42"}),
            serde_json::json!({"target_file": "f", "offset": 100.0}),
        ] {
            assert!(serde_json::from_value::<ReadFileArgs>(bad).is_err());
        }
    }

    // `gb:463-509` message variants, incl. grok's missing-"tool" wording in
    // the no-range variant.
    #[test]
    fn too_large_messages_match_grok() {
        let range = too_large_message(30_000, Some(5), None, false);
        assert_eq!(
            range,
            "The requested line range (offset=5, limit=to end) contains 30000 tokens, which exceeds the maximum allowed tokens (25000 tokens).\nTry a smaller `limit`, a different starting `offset`, or use the 'grep' tool to search for specific content."
        );
        let plain = too_large_message(30_000, None, None, true);
        assert_eq!(
            plain,
            "File content (30000 tokens) exceeds maximum allowed tokens (25000 tokens).\nPlease use offset and limit parameters to read a shorter range, or use the 'grep' to search for specific content.\nNote: the requested read is a single very long line, so line-based offset/limit cannot narrow it further. Use the 'run_terminal_cmd' tool to extract the parts you need (e.g. `jq`, `python3`, or `cut -c`)."
        );
    }
}
