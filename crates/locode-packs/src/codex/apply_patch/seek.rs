//! `seek_sequence` — the tiered context matcher, a faithful port of codex's
//! `apply-patch/src/seek_sequence.rs` (submodule `f201c30c`).
//!
//! Four decreasingly strict passes locate a block of `pattern` lines within
//! `lines`, at or after `start`: (1) exact, (2) ignoring trailing whitespace,
//! (3) ignoring leading+trailing whitespace, (4) trim + Unicode-punctuation fold
//! (typographic dashes/quotes/spaces → ASCII). When `eof` is set the search
//! anchors at the file end first. Tier 4 is easy to omit and would spuriously
//! fail on files with typographic characters — it is kept verbatim.

/// Find `pattern` within `lines` at or after `start`; `None` if unmatched.
pub(crate) fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eof: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    // A pattern longer than the input can never match (and would panic the slice).
    if pattern.len() > lines.len() {
        return None;
    }
    let search_start = if eof && lines.len() >= pattern.len() {
        lines.len() - pattern.len()
    } else {
        start
    };
    let last = lines.len().saturating_sub(pattern.len());

    // Tier 1 — exact.
    for i in search_start..=last {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }
    // Tier 2 — ignore trailing whitespace.
    for i in search_start..=last {
        if (0..pattern.len()).all(|p| lines[i + p].trim_end() == pattern[p].trim_end()) {
            return Some(i);
        }
    }
    // Tier 3 — ignore leading + trailing whitespace.
    for i in search_start..=last {
        if (0..pattern.len()).all(|p| lines[i + p].trim() == pattern[p].trim()) {
            return Some(i);
        }
    }
    // Tier 4 — trim + fold common Unicode punctuation to ASCII.
    for i in search_start..=last {
        if (0..pattern.len()).all(|p| normalise(&lines[i + p]) == normalise(&pattern[p])) {
            return Some(i);
        }
    }
    None
}

/// Trim, then map typographic dashes / quotes / spaces to their ASCII forms
/// (verbatim from the source's `normalise`).
fn normalise(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}
