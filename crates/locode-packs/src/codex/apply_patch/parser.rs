//! The V4A patch parser — a faithful port of codex's `apply-patch` crate
//! (`streaming_parser.rs` + `parser.rs`, submodule `f201c30c`).
//!
//! Ported line-for-line: the markers, the streaming state machine, the `Hunk` /
//! `UpdateFileChunk` AST, and the lenient boundary check (incl. the GPT-4.1
//! heredoc-unwrap workaround). **Deviation:** the single-environment grammar we
//! ship has no `*** Environment ID:` production (`include_environment_id=false`,
//! `spec_plan.rs:784`), so environment-id handling is dropped (unreachable).

use std::path::PathBuf;

// ---- markers (`parser.rs:37-45`) ----
const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
const END_PATCH_MARKER: &str = "*** End Patch";
const ADD_FILE_MARKER: &str = "*** Add File: ";
const DELETE_FILE_MARKER: &str = "*** Delete File: ";
const UPDATE_FILE_MARKER: &str = "*** Update File: ";
const MOVE_TO_MARKER: &str = "*** Move to: ";
const EOF_MARKER: &str = "*** End of File";
const CHANGE_CONTEXT_MARKER: &str = "@@ ";
const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";

/// One file operation in a patch (`parser.rs:64-82`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // faithful to codex's `AddFile`/`DeleteFile`/`UpdateFile`
pub(crate) enum Hunk {
    /// Create a file whose contents are the concatenated `+` lines.
    AddFile {
        /// Target path, as written in the patch.
        path: PathBuf,
        /// The new file's contents (each `+` line + `\n`).
        contents: String,
    },
    /// Delete an existing file.
    DeleteFile {
        /// Target path, as written in the patch.
        path: PathBuf,
    },
    /// Edit (and optionally move) an existing file.
    UpdateFile {
        /// Source path, as written in the patch.
        path: PathBuf,
        /// Destination path when the hunk carries a `*** Move to:` line.
        move_path: Option<PathBuf>,
        /// The ordered edit chunks.
        chunks: Vec<UpdateFileChunk>,
    },
}

/// One `@@`-delimited edit region within an `UpdateFile` (`parser.rs:114-128`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateFileChunk {
    /// The text after `@@ ` used to locate the region (`None` for a bare `@@`).
    pub(crate) change_context: Option<String>,
    /// Context + removed lines (what must be found in the file).
    pub(crate) old_lines: Vec<String>,
    /// Context + added lines (what replaces the found region).
    pub(crate) new_lines: Vec<String>,
    /// Set by a trailing `*** End of File` — anchors the match at file end.
    pub(crate) is_end_of_file: bool,
}

/// A patch that could not be parsed (`parser.rs:55-61`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseError {
    /// A structural problem with the patch envelope.
    InvalidPatchError(String),
    /// A problem within a hunk, tagged with the 1-based line number.
    InvalidHunkError {
        /// The human-readable reason.
        message: String,
        /// The 1-based line number the error was found on.
        line_number: usize,
    },
}

use Hunk::{AddFile, DeleteFile, UpdateFile};
use ParseError::{InvalidHunkError, InvalidPatchError};

/// Parse a full patch string into its hunks (`parser.rs:130-196`, lenient mode).
pub(crate) fn parse(patch: &str) -> Result<Vec<Hunk>, ParseError> {
    let lines: Vec<&str> = patch.trim().lines().collect();
    let patch_lines = check_patch_boundaries_lenient(&lines)?;
    let patch = patch_lines.join("\n");
    let mut parser = StreamingPatchParser::default();
    parser.push_delta(&patch)?;
    parser.finish()
}

/// Strict boundary check: first/last (trimmed) lines must be the envelope markers
/// (`parser.rs:241-259`).
fn check_start_and_end_lines_strict(
    first_line: Option<&&str>,
    last_line: Option<&&str>,
) -> Result<(), ParseError> {
    if first_line.map(|l| l.trim()) != Some(BEGIN_PATCH_MARKER) {
        return Err(InvalidPatchError(
            "The first line of the patch must be '*** Begin Patch'".to_string(),
        ));
    }
    if last_line.map(|l| l.trim()) != Some(END_PATCH_MARKER) {
        return Err(InvalidPatchError(
            "The last line of the patch must be '*** End Patch'".to_string(),
        ));
    }
    Ok(())
}

/// Lenient boundary check: unwrap a `<<EOF … EOF` heredoc the GPT-4.1 `local_shell`
/// path leaves around the patch, then apply the strict check (`parser.rs:217-259`).
fn check_patch_boundaries_lenient<'a>(lines: &'a [&'a str]) -> Result<&'a [&'a str], ParseError> {
    let (first, last) = match lines {
        [] => (None, None),
        [only] => (Some(only), Some(only)),
        [first, .., last] => (Some(first), Some(last)),
    };
    if check_start_and_end_lines_strict(first, last).is_ok() {
        return Ok(lines);
    }
    if let (Some(first), Some(last)) = (first, last) {
        let is_heredoc_open = matches!(first.trim(), "<<EOF" | "<<'EOF'" | "<<\"EOF\"");
        if is_heredoc_open && last.trim().ends_with("EOF") && lines.len() >= 4 {
            let inner = &lines[1..lines.len() - 1];
            let (inner_first, inner_last) = (inner.first(), inner.last());
            check_start_and_end_lines_strict(inner_first, inner_last)?;
            return Ok(inner);
        }
    }
    // Report the strict error for a clear message.
    check_start_and_end_lines_strict(first, last)?;
    Ok(lines)
}

// ---- the streaming state machine (`streaming_parser.rs`) ----

#[derive(Debug, Default)]
struct StreamingPatchParser {
    line_buffer: String,
    mode: Mode,
    hunks: Vec<Hunk>,
    line_number: usize,
}

#[derive(Debug, Default, Clone, Copy)]
enum Mode {
    #[default]
    NotStarted,
    StartedPatch,
    AddFile,
    DeleteFile,
    UpdateFile {
        hunk_line_number: usize,
    },
    EndedPatch,
}

impl StreamingPatchParser {
    fn push_delta(&mut self, delta: &str) -> Result<(), ParseError> {
        for ch in delta.chars() {
            if ch == '\n' {
                let mut line = std::mem::take(&mut self.line_buffer);
                line.truncate(line.strip_suffix('\r').map_or(line.len(), str::len));
                self.line_number += 1;
                self.process_line(&line)?;
            } else {
                self.line_buffer.push(ch);
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<Vec<Hunk>, ParseError> {
        if !self.line_buffer.is_empty() {
            let line = std::mem::take(&mut self.line_buffer);
            self.line_number += 1;
            if line.trim() == END_PATCH_MARKER {
                self.ensure_update_hunk_is_not_empty(line.trim())?;
                self.mode = Mode::EndedPatch;
            } else {
                self.process_line(&line)?;
            }
        }
        if !matches!(self.mode, Mode::EndedPatch) {
            return Err(InvalidPatchError(
                "The last line of the patch must be '*** End Patch'".to_string(),
            ));
        }
        Ok(std::mem::take(&mut self.hunks))
    }

    fn ensure_update_hunk_is_not_empty(&self, line: &str) -> Result<(), ParseError> {
        if let Some(UpdateFile { path, chunks, .. }) = self.hunks.last() {
            if chunks.is_empty()
                && let Mode::UpdateFile { hunk_line_number } = self.mode
            {
                return Err(InvalidHunkError {
                    message: format!("Update file hunk for path '{}' is empty", path.display()),
                    line_number: hunk_line_number,
                });
            }
            if chunks
                .last()
                .is_some_and(|c| c.old_lines.is_empty() && c.new_lines.is_empty())
            {
                if line == END_PATCH_MARKER {
                    return Err(InvalidHunkError {
                        message: "Update hunk does not contain any lines".to_string(),
                        line_number: self.line_number,
                    });
                }
                return Err(InvalidHunkError {
                    message: format!(
                        "Unexpected line found in update hunk: '{line}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                    ),
                    line_number: self.line_number,
                });
            }
        }
        Ok(())
    }

    fn handle_hunk_headers_and_end_patch(&mut self, trimmed: &str) -> Result<bool, ParseError> {
        if trimmed == END_PATCH_MARKER {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.mode = Mode::EndedPatch;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(ADD_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.hunks.push(AddFile {
                path: PathBuf::from(path),
                contents: String::new(),
            });
            self.mode = Mode::AddFile;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(DELETE_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.hunks.push(DeleteFile {
                path: PathBuf::from(path),
            });
            self.mode = Mode::DeleteFile;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(UPDATE_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.hunks.push(UpdateFile {
                path: PathBuf::from(path),
                move_path: None,
                chunks: Vec::new(),
            });
            self.mode = Mode::UpdateFile {
                hunk_line_number: self.line_number,
            };
            return Ok(true);
        }
        Ok(false)
    }

    #[allow(clippy::too_many_lines)] // a faithful 1:1 port of the source state machine
    #[allow(clippy::match_same_arms)] // the per-mode "not a valid hunk header" arms stay explicit
    fn process_line(&mut self, line: &str) -> Result<(), ParseError> {
        let trimmed = line.trim();
        match self.mode {
            Mode::NotStarted => {
                if trimmed == BEGIN_PATCH_MARKER {
                    self.mode = Mode::StartedPatch;
                    return Ok(());
                }
                Err(InvalidPatchError(
                    "The first line of the patch must be '*** Begin Patch'".to_string(),
                ))
            }
            Mode::StartedPatch => {
                if self.handle_hunk_headers_and_end_patch(trimmed)? {
                    return Ok(());
                }
                Err(InvalidHunkError {
                    message: format!(
                        "'{trimmed}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
                    ),
                    line_number: self.line_number,
                })
            }
            Mode::AddFile => {
                if self.handle_hunk_headers_and_end_patch(trimmed)? {
                    return Ok(());
                }
                if let Some(to_add) = line.strip_prefix('+')
                    && let Some(AddFile { contents, .. }) = self.hunks.last_mut()
                {
                    contents.push_str(to_add);
                    contents.push('\n');
                    return Ok(());
                }
                Err(InvalidHunkError {
                    message: format!(
                        "'{trimmed}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
                    ),
                    line_number: self.line_number,
                })
            }
            Mode::DeleteFile => {
                if self.handle_hunk_headers_and_end_patch(trimmed)? {
                    return Ok(());
                }
                Err(InvalidHunkError {
                    message: format!(
                        "'{trimmed}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
                    ),
                    line_number: self.line_number,
                })
            }
            Mode::UpdateFile { hunk_line_number } => {
                let update_line = line.trim_end();
                if self.handle_hunk_headers_and_end_patch(update_line)? {
                    return Ok(());
                }
                if let Some(UpdateFile {
                    move_path, chunks, ..
                }) = self.hunks.last_mut()
                {
                    if chunks.last().is_some_and(|c| c.is_end_of_file) {
                        if update_line.is_empty() {
                            return Ok(());
                        }
                        if update_line != EMPTY_CHANGE_CONTEXT_MARKER
                            && !update_line.starts_with(CHANGE_CONTEXT_MARKER)
                        {
                            return Err(InvalidHunkError {
                                message: format!(
                                    "Expected update hunk to start with a @@ context marker, got: '{line}'"
                                ),
                                line_number: self.line_number,
                            });
                        }
                    }

                    if chunks.is_empty()
                        && move_path.is_none()
                        && let Some(dest) = update_line.strip_prefix(MOVE_TO_MARKER)
                    {
                        *move_path = Some(PathBuf::from(dest));
                        self.mode = Mode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }

                    if (update_line == EMPTY_CHANGE_CONTEXT_MARKER
                        || update_line.starts_with(CHANGE_CONTEXT_MARKER))
                        && chunks
                            .last()
                            .is_some_and(|c| c.old_lines.is_empty() && c.new_lines.is_empty())
                    {
                        return Err(InvalidHunkError {
                            message: format!(
                                "Unexpected line found in update hunk: '{line}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                            ),
                            line_number: self.line_number,
                        });
                    }

                    if update_line == EMPTY_CHANGE_CONTEXT_MARKER {
                        chunks.push(new_chunk(None));
                        self.mode = Mode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }
                    if let Some(ctx) = update_line.strip_prefix(CHANGE_CONTEXT_MARKER) {
                        chunks.push(new_chunk(Some(ctx.to_string())));
                        self.mode = Mode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }
                    if update_line == EOF_MARKER {
                        if chunks
                            .last()
                            .is_some_and(|c| c.old_lines.is_empty() && c.new_lines.is_empty())
                        {
                            return Err(InvalidHunkError {
                                message: "Update hunk does not contain any lines".to_string(),
                                line_number: self.line_number,
                            });
                        }
                        if let Some(chunk) = chunks.last_mut() {
                            chunk.is_end_of_file = true;
                        }
                        self.mode = Mode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }

                    if line.is_empty() {
                        ensure_chunk(chunks);
                        if let Some(chunk) = chunks.last_mut() {
                            chunk.old_lines.push(String::new());
                            chunk.new_lines.push(String::new());
                        }
                        self.mode = Mode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }
                    if let Some(ctx) = line.strip_prefix(' ') {
                        ensure_chunk(chunks);
                        if let Some(chunk) = chunks.last_mut() {
                            chunk.old_lines.push(ctx.to_string());
                            chunk.new_lines.push(ctx.to_string());
                        }
                        self.mode = Mode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }
                    if let Some(added) = line.strip_prefix('+') {
                        ensure_chunk(chunks);
                        if let Some(chunk) = chunks.last_mut() {
                            chunk.new_lines.push(added.to_string());
                        }
                        self.mode = Mode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }
                    if let Some(removed) = line.strip_prefix('-') {
                        ensure_chunk(chunks);
                        if let Some(chunk) = chunks.last_mut() {
                            chunk.old_lines.push(removed.to_string());
                        }
                        self.mode = Mode::UpdateFile { hunk_line_number };
                        return Ok(());
                    }

                    if chunks
                        .last()
                        .is_some_and(|c| !c.old_lines.is_empty() || !c.new_lines.is_empty())
                    {
                        return Err(InvalidHunkError {
                            message: format!(
                                "Expected update hunk to start with a @@ context marker, got: '{line}'"
                            ),
                            line_number: self.line_number,
                        });
                    }
                }
                Err(InvalidHunkError {
                    message: format!(
                        "Unexpected line found in update hunk: '{line}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                    ),
                    line_number: self.line_number,
                })
            }
            Mode::EndedPatch => {
                if trimmed.is_empty() {
                    Ok(())
                } else {
                    Err(InvalidPatchError(
                        "The last line of the patch must be '*** End Patch'".to_string(),
                    ))
                }
            }
        }
    }
}

fn new_chunk(change_context: Option<String>) -> UpdateFileChunk {
    UpdateFileChunk {
        change_context,
        old_lines: Vec::new(),
        new_lines: Vec::new(),
        is_end_of_file: false,
    }
}

/// Auto-open a context-less chunk when a `+`/`-`/` `/empty line appears with no
/// preceding `@@` (`streaming_parser.rs` `if chunks.is_empty()` guards).
fn ensure_chunk(chunks: &mut Vec<UpdateFileChunk>) {
    if chunks.is_empty() {
        chunks.push(new_chunk(None));
    }
}
