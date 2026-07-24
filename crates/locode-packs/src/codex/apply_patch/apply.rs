//! Turn an `UpdateFile`'s chunks into new file contents — a faithful port of
//! codex's `derive_new_contents_from_chunks` / `compute_replacements` /
//! `apply_replacements` (`apply-patch/src/lib.rs:680-829`, submodule `f201c30c`).

use super::parser::UpdateFileChunk;
use super::seek::seek_sequence;

/// A single scheduled edit: replace `old_len` lines at `start` with `new_lines`.
type Replacement = (usize, usize, Vec<String>);

/// Compute the new contents of a file from its `original` text and the update
/// `chunks`. `path` names the file for error messages.
///
/// # Errors
/// Returns the verbatim codex message when a context or an old-lines block cannot
/// be located.
pub(crate) fn derive_new_contents(
    original: &str,
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<String, String> {
    let mut original_lines: Vec<String> = original.split('\n').map(String::from).collect();
    // Drop the trailing empty element from the final newline so line counts match `diff`.
    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }
    let replacements = compute_replacements(&original_lines, path, chunks)?;
    let mut new_lines = apply_replacements(original_lines, &replacements);
    // Guarantee exactly one trailing newline.
    if !new_lines.last().is_some_and(String::is_empty) {
        new_lines.push(String::new());
    }
    Ok(new_lines.join("\n"))
}

fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<Replacement>, String> {
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(format!("Failed to find context '{ctx_line}' in {path}"));
            }
        }

        if chunk.old_lines.is_empty() {
            // Pure addition — insert at EOF, or just before a trailing empty line.
            let insertion_idx = if original_lines.last().is_some_and(String::is_empty) {
                original_lines.len() - 1
            } else {
                original_lines.len()
            };
            replacements.push((insertion_idx, 0, chunk.new_lines.clone()));
            continue;
        }

        let mut pattern: &[String] = &chunk.old_lines;
        let mut new_slice: &[String] = &chunk.new_lines;
        let mut found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

        // The last element of `old_lines` is often an empty string standing for the
        // file's final newline, which `split('\n')`-then-pop removed from
        // `original_lines`. Retry without it (and its `new_lines` twin) on a miss.
        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        }

        if let Some(start_idx) = found {
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            line_index = start_idx + pattern.len();
        } else {
            return Err(format!(
                "Failed to find expected lines in {}:\n{}",
                path,
                chunk.old_lines.join("\n"),
            ));
        }
    }

    replacements.sort_by_key(|(index, _, _)| *index);
    Ok(replacements)
}

fn apply_replacements(mut lines: Vec<String>, replacements: &[Replacement]) -> Vec<String> {
    // Descending order so earlier edits don't shift later indices.
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        let start_idx = *start_idx;
        for _ in 0..*old_len {
            if start_idx < lines.len() {
                lines.remove(start_idx);
            }
        }
        for (offset, new_line) in new_segment.iter().enumerate() {
            lines.insert(start_idx + offset, new_line.clone());
        }
    }
    lines
}
