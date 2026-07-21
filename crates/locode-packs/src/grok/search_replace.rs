//! `search_replace` — a faithful port of Grok Build's exact-string edit
//! (`gb/search_replace/mod.rs`), **current default configuration** (Task 26;
//! supersedes the accidental legacy-0.4.10 surface of the Task 11 port).
//!
//! Fidelity notes (see `tasks/audits/search_replace.md`):
//! - Frozen grok defaults (constants, not config): user-edit hint ON
//!   (`gb:110-115`), empty-`old_string` overwrite allowed
//!   (`empty_old_string_does_not_override = false`, `gb:99-102`), path-not-found
//!   hints OFF (`PathNotFoundHints` default false), confusable *fallback
//!   matching* OFF (`unicode_normalized_fallback` default false, `gb:103-110`
//!   — unported, unreachable at default). The confusable *diagnostic* is
//!   unconditional in the current era (`gb:626-638`) and IS ported.
//! - This is grok's **only** edit/create tool (empty `old_string` creates —
//!   or overwrites). No mtime freshness, no read-before-edit runtime guard
//!   (grok's `skip_read_before_edit` is a deprecated no-op, `gb:95-98`).
//! - Type-strict `replace_all` (no lenient bool — user deviation 2026-07-20).
//! - The gitignore guard (default ON via `RespectGitignore`, `gb:173-186`)
//!   goes through the host seam (`is_path_ignored`, Slice 0a).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{FsError, Host};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::confusables;

/// grok's truncation marker (`truncate.rs:10`).
const TRUNCATION_MARKER: &str = "…";
/// grok's cap on listed confusable lines (`gb:455`).
const MAX_LISTED_LINES: usize = 8;

/// grok's `search_replace` tool.
pub(crate) struct GrokSearchReplace {
    host: Arc<Host>,
}

impl GrokSearchReplace {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `search_replace` (grok's real schema; `${{…}}` template refs rendered).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SearchReplaceArgs {
    #[schemars(
        description = "The path to the file to modify. You can use either a relative path in the workspace or an absolute path."
    )]
    file_path: String,
    #[schemars(description = "The text to replace")]
    old_string: String,
    #[schemars(description = "The text to replace it with (must be different from old_string)")]
    new_string: String,
    #[serde(default)]
    #[schemars(description = "Replace all occurrences of old_string (default false)")]
    replace_all: bool,
}

/// The structured (report) face of an edit; the grok success text is the
/// prompt face.
#[derive(Debug, Serialize)]
pub(crate) struct SearchReplaceOutput {
    /// The jail-resolved absolute path written.
    path: String,
    /// Whether the file was created/overwritten via empty `old_string`.
    created: bool,
    /// Number of occurrences replaced (0 on create).
    replacements: usize,
    /// grok's exact success message (prompt face only).
    #[serde(skip)]
    message: String,
}

impl ToolOutput for SearchReplaceOutput {
    fn to_prompt_text(&self) -> String {
        self.message.clone()
    }
}

#[async_trait]
impl Tool for GrokSearchReplace {
    type Args = SearchReplaceArgs;
    type Output = SearchReplaceOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Edit
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        // grok's `DESCRIPTION_FULL` (`gb:59-63`), rendered for the grok_build
        // toolset (`read` → `read_file`, params literal; renderer fixture
        // `gb:842-853`).
        "Replace an exact string in a file.\n\n- Read the file with `read_file` before editing it.\n- `read_file` prefixes each line with \"LINE_NUMBER→\". That prefix is not part of the file: match only what comes after the →, with its exact indentation.\n- `old_string` must match exactly one place in the file. If it appears more than once, add surrounding lines to make it unique, or set `replace_all` to change every occurrence (handy for renaming an identifier)."
    }

    #[allow(clippy::too_many_lines)] // grok's validation + edit pipeline, ported in order
    async fn run(&self, ctx: &ToolCtx, args: SearchReplaceArgs) -> Result<Self::Output, ToolError> {
        // --- Pre-flight, grok's order (`gb:165-191`) ---
        // 1. Path-component length (`gb:165-167, 250-271`).
        if let Some(too_long) = Path::new(&args.file_path)
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .find(|c| c.len() > 255)
        {
            return Err(ToolError::Respond(format!(
                "Error: file name exceeds the 255-character limit ({} characters). Please use a shorter file name.",
                too_long.len()
            )));
        }
        let path = Path::new(&args.file_path);
        let resolved = self
            .host
            .resolve_in_jail(&ctx.cwd, path)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;
        // 2. Directory target (`gb:168-172`).
        if resolved.is_dir() {
            return Err(ToolError::Respond("File path is a directory".to_string()));
        }
        // 3. Gitignore guard — default ON (`gb:173-186`); model-supplied path.
        if self
            .host
            .is_path_ignored(&ctx.cwd, path)
            .await
            .unwrap_or(false)
        {
            return Err(ToolError::Respond(format!(
                "Error: {} is ignored by .gitignore and cannot be edited.",
                args.file_path
            )));
        }
        // 4. No-op edit (`gb:187-191`).
        if args.old_string == args.new_string {
            return Err(ToolError::Respond(
                "Old string and new string are the same".into(),
            ));
        }

        let display = resolved.display().to_string();

        // Empty old_string ⇒ create — or OVERWRITE an existing file (grok's
        // default: `empty_old_string_does_not_override = false`, `gb:99-102`,
        // `gb:283-293` + test `gb:1264-1282`). The Task 11 "File already
        // exists" error was invented and is removed.
        if args.old_string.is_empty() {
            self.host
                .write_file(&ctx.cwd, path, &args.new_string)
                .await
                .map_err(|e| ToolError::Respond(e.to_string()))?;
            return Ok(SearchReplaceOutput {
                path: display,
                created: true,
                replacements: 0,
                // grok's creation text (`gb:358-361`), model-supplied path.
                message: format!("The file {} has been created successfully.", args.file_path),
            });
        }

        // Edit an existing file. Not-found = current era's base text
        // (`gb:517-529`, hints off; model-supplied path).
        let read = match self.host.read_file(&ctx.cwd, path).await {
            Ok(read) => read,
            Err(FsError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(ToolError::Respond(format!(
                    "Error: {} does not exist.",
                    args.file_path
                )));
            }
            Err(e) => return Err(ToolError::Respond(e.to_string())),
        };
        let content = read.contents;

        // CRLF handling (`gb:553-563, 688-692`): match against an
        // LF-normalized copy; re-expand on write-back.
        let is_crlf = content.contains("\r\n");
        let match_text = if is_crlf {
            content.replace("\r\n", "\n")
        } else {
            content.clone()
        };

        let count = match_text.matches(&args.old_string).count();
        if count == 0 {
            // Current-era no-match assembly (`gb:626-653`): base + user-edit
            // hint (default on) + nearest-match hint + confusable diagnostic.
            let nearest = build_nearest_match_hint(&match_text, &args.old_string);
            let confusable = build_confusable_hint(&match_text, &args.old_string);
            return Err(ToolError::Respond(format!(
                "The string to replace was not found in the file, use the read_file tool to see the correct string. The user may have changed the file since you last read it.{nearest}{confusable}"
            )));
        }
        if count > 1 && !args.replace_all {
            return Err(ToolError::Respond(
                "The string to replace was found multiple times in the file. Use replace_all to replace all occurrences, or include more context to only edit one occurrence.".into(),
            ));
        }

        let new_text = if args.replace_all {
            match_text.replace(&args.old_string, &args.new_string)
        } else {
            match_text.replacen(&args.old_string, &args.new_string, 1)
        };
        let write_back = if is_crlf {
            new_text.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            new_text
        };
        self.host
            .write_file(&ctx.cwd, path, &write_back)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;

        // grok's success texts (`gb:724-741`): model-supplied path, no counts.
        let message = if args.replace_all && count > 1 {
            format!(
                "The file {} has been updated. All occurrences were successfully replaced.",
                args.file_path
            )
        } else {
            format!("The file {} has been updated successfully.", args.file_path)
        };
        Ok(SearchReplaceOutput {
            path: display,
            created: false,
            replacements: count,
            message,
        })
    }
}

/// grok's nearest-match hint (`gb:385-408`): first file line containing the
/// longest token of `old_string`'s first line, as
/// `"\n\nNearest match: line N: <content>"`, ≤ 200 bytes with `…`.
fn build_nearest_match_hint(file: &str, old_string: &str) -> String {
    let keyword = old_string
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .max_by_key(|w| w.len())
        .unwrap_or("");
    if keyword.is_empty() {
        return String::new();
    }
    file.lines()
        .enumerate()
        .find(|(_, l)| l.contains(keyword))
        .map(|(i, l)| {
            let full = format!("\n\nNearest match: line {}: {}", i + 1, l.trim_end());
            truncate_str_with_marker(&full, 200)
        })
        .unwrap_or_default()
}

/// grok's `truncate_str_with_marker` (`truncate.rs:137-154`): byte-budget cut
/// at a char boundary with the `…` marker.
fn truncate_str_with_marker(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes - TRUNCATION_MARKER.len();
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATION_MARKER}", &s[..end])
}

/// Which scripting tools exist on `$PATH` (grok's `QueryTools`,
/// `query_tools.rs:12-66`): `edit_tools` = present ∈ {python, sed}, python3
/// preferred. Probed once per process, like grok.
fn detected_edit_tools() -> &'static [String] {
    use std::sync::OnceLock;
    static DETECTED: OnceLock<Vec<String>> = OnceLock::new();
    DETECTED.get_or_init(|| {
        let present = |name: &str| {
            std::env::var_os("PATH").is_some_and(|paths| {
                std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
            })
        };
        let mut tools = Vec::new();
        if present("python3") || present("python") {
            let py = if present("python3") {
                "python3"
            } else {
                "python"
            };
            tools.push(format!("`{py}`"));
        }
        if present("sed") {
            tools.push("`sed`".to_string());
        }
        tools
    })
}

/// grok's `examples_clause` (`query_tools.rs:71-78`).
fn examples_clause(tools: &[String]) -> String {
    match tools {
        [] => String::new(),
        [a] => format!(" (e.g. {a})"),
        [a, b] => format!(" (e.g. {a} or {b})"),
        [rest @ .., last] => format!(" (e.g. {}, or {last})", rest.join(", ")),
    }
}

/// grok's Unicode-confusable diagnostic (`gb:423-499`), rendered for the
/// `grok_build` toolset (`read_file` / `old_string` / `run_terminal_cmd`).
/// Unconditional in the current era — only the fallback *matching* is gated.
fn build_confusable_hint(file: &str, old_string: &str) -> String {
    if !confusables::has_confusables(file) {
        return String::new();
    }
    let (norm_file, offset_map) = confusables::build_offset_map(file);
    let norm_old = confusables::normalize_confusables(old_string);
    let Some(norm_start) = norm_file.find(&norm_old) else {
        return String::new();
    };
    let orig_start = offset_map[norm_start];
    let orig_end = offset_map[norm_start + norm_old.len()];
    let match_start_line = file[..orig_start].matches('\n').count() + 1;
    let match_end_line = file[..orig_end].matches('\n').count() + 1;
    let hits = confusables::detect_confusables(file);
    let mut affected_lines: Vec<usize> = hits
        .iter()
        .filter(|h| h.line_number >= match_start_line && h.line_number <= match_end_line)
        .map(|h| h.line_number)
        .collect();
    affected_lines.dedup();
    if affected_lines.is_empty() {
        return String::new();
    }
    let line_summary = if affected_lines.len() <= MAX_LISTED_LINES {
        affected_lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        let shown: Vec<String> = affected_lines[..MAX_LISTED_LINES]
            .iter()
            .map(ToString::to_string)
            .collect();
        format!(
            "{} (and {} more)",
            shown.join(", "),
            affected_lines.len() - MAX_LISTED_LINES
        )
    };
    let edit_tools = detected_edit_tools();
    let terminal_fallback = if edit_tools.is_empty() {
        String::new()
    } else {
        format!(
            ", or use run_terminal_cmd with a short script{} to edit the file directly",
            examples_clause(edit_tools)
        )
    };
    format!(
        "\n\nThe nearest matching region contains Unicode typography characters \
         (smart quotes, em-dashes, etc.) on lines {line_summary} that look identical to \
         ASCII in read_file output but differ at the byte level. Re-read the file and \
         use a shorter old_string anchored on nearby ASCII-only context{terminal_fallback}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_match_hint_finds_longest_token_line() {
        let file = "alpha\nthe quick brownfox jumps\nomega\n";
        let hint = build_nearest_match_hint(file, "quick brownfox leaps");
        assert_eq!(hint, "\n\nNearest match: line 2: the quick brownfox jumps");
        assert_eq!(build_nearest_match_hint(file, ""), "");
        assert_eq!(build_nearest_match_hint(file, "zzz"), "");
    }

    #[test]
    fn nearest_match_hint_caps_at_200_bytes_with_marker() {
        let long_line = format!("keyword {}", "x".repeat(400));
        let file = format!("{long_line}\n");
        let hint = build_nearest_match_hint(&file, "keyword");
        assert!(hint.len() <= 200);
        assert!(hint.ends_with(TRUNCATION_MARKER));
    }

    #[test]
    fn confusable_hint_fires_on_smart_quotes() {
        let file = "let s = \u{201c}hello\u{201d};\n";
        let hint = build_confusable_hint(file, "let s = \"hello\";");
        assert!(
            hint.contains("Unicode typography characters"),
            "hint: {hint:?}"
        );
        assert!(hint.contains("on lines 1"), "hint: {hint:?}");
    }

    #[test]
    fn confusable_hint_silent_without_confusables() {
        assert_eq!(build_confusable_hint("plain ascii\n", "missing"), "");
    }

    #[test]
    fn examples_clause_formats_match_grok() {
        assert_eq!(examples_clause(&[]), "");
        assert_eq!(examples_clause(&["`sed`".into()]), " (e.g. `sed`)");
        assert_eq!(
            examples_clause(&["`python3`".into(), "`sed`".into()]),
            " (e.g. `python3` or `sed`)"
        );
    }
}
