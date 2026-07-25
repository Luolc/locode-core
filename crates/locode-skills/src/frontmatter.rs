//! The `SKILL.md` YAML frontmatter reader — **scalar keys only, by design**.
//!
//! ADR-0025 §2 recognizes exactly five keys, all scalars (`name`, `description`,
//! `when-to-use`, `disable-model-invocation`, `user-invocable`), and requires unknown
//! keys to be *ignored* rather than rejected. That makes a full YAML parser unnecessary
//! — and a new dependency is an ask-first item under AGENTS.md, so this reads the block
//! directly.
//!
//! Grok's own loader has the same shape as its recovery path: it salvages
//! "listing-relevant scalar fields" and deliberately does not try to interpret list or
//! map values like `allowed-tools` and `paths` (`discovery.rs:406`). We simply never
//! need those, so scalar-only is the whole contract rather than a fallback.
//!
//! What is handled: the `---` fences, `key: value` pairs, quoted values, values that
//! themselves contain `:` (`description: Deploy: push to prod`), comments, and blank
//! lines. What is skipped: any key whose value spans lines (a block scalar, a list, or
//! a nested map) — its continuation lines are consumed and dropped, so a list-valued
//! `allowed-tools:` can never be mistaken for a scalar.

use std::collections::HashMap;

/// Split a `SKILL.md` body into `(frontmatter_pairs, markdown_body)`.
///
/// Returns `None` when the file does not open with a `---` fence — a `SKILL.md` with no
/// frontmatter has no name and no description, so the caller skips it.
pub(crate) fn parse(source: &str) -> Option<(HashMap<String, String>, &str)> {
    // Tolerate a leading BOM and blank lines before the fence.
    let text = source.strip_prefix('\u{feff}').unwrap_or(source);
    let after_open = strip_fence_line(text)?;
    let (block, body) = split_at_closing_fence(after_open)?;
    Some((parse_pairs(block), body))
}

/// Consume an opening `---` line (and any blank lines before it).
fn strip_fence_line(text: &str) -> Option<&str> {
    let mut rest = text;
    loop {
        let (line, tail) = split_line(rest);
        if line.trim().is_empty() {
            rest = tail?;
            continue;
        }
        return if line.trim_end() == "---" { tail } else { None };
    }
}

/// Split at the first closing `---`, returning `(block, body_after)`.
fn split_at_closing_fence(text: &str) -> Option<(&str, &str)> {
    let mut offset = 0usize;
    let mut rest = text;
    loop {
        let (line, tail) = split_line(rest);
        if matches!(line.trim_end(), "---" | "...") {
            let body = tail.unwrap_or("");
            return Some((&text[..offset], body));
        }
        let tail = tail?;
        offset += rest.len() - tail.len();
        rest = tail;
    }
}

/// `(line_without_newline, rest_after_newline_or_None_at_eof)`.
fn split_line(text: &str) -> (&str, Option<&str>) {
    match text.find('\n') {
        Some(i) => (&text[..i], Some(&text[i + 1..])),
        None => (text, None),
    }
}

fn parse_pairs(block: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in block.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Indented lines belong to a previous multi-line value; a top-level key starts
        // at column 0. Anything indented here is a continuation we already decided to
        // drop, so skip it.
        if line.starts_with([' ', '\t']) || trimmed.starts_with('-') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if value.is_empty() {
            // `key:` with the value on following lines — a block scalar, list, or map.
            // Not a scalar, so drop it *and* its continuation, which the indent check
            // above already handles as the iterator advances.
            continue;
        }
        out.insert(key, unquote(value).to_string());
    }
    out
}

/// Strip one layer of matching quotes; leave everything else verbatim (including a
/// trailing `#`, which in YAML would need a space before it to start a comment and is
/// far more likely to be part of a description here).
fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// YAML-ish truthiness for the two boolean keys: `true`/`yes`/`on`/`1`.
pub(crate) fn as_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "yes" | "on" | "1"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(src: &str) -> HashMap<String, String> {
        parse(src).expect("frontmatter").0
    }

    #[test]
    fn reads_scalar_keys_and_the_body() {
        let (fm, body) =
            parse("---\nname: commit\ndescription: Make a commit\n---\n# Body\ntext\n")
                .expect("frontmatter");
        assert_eq!(fm.get("name").map(String::as_str), Some("commit"));
        assert_eq!(
            fm.get("description").map(String::as_str),
            Some("Make a commit")
        );
        assert_eq!(body, "# Body\ntext\n");
    }

    #[test]
    fn a_value_may_contain_colons() {
        let fm = pairs("---\ndescription: Deploy: push to prod\n---\n");
        assert_eq!(
            fm.get("description").map(String::as_str),
            Some("Deploy: push to prod")
        );
    }

    #[test]
    fn quotes_are_stripped_once() {
        let fm = pairs("---\nname: \"commit\"\nwhen-to-use: 'on push'\n---\n");
        assert_eq!(fm.get("name").map(String::as_str), Some("commit"));
        assert_eq!(fm.get("when-to-use").map(String::as_str), Some("on push"));
    }

    /// The load-bearing case: a list-valued key must never be salvaged as a scalar, or
    /// `allowed-tools: [Bash, Edit]` would look like a recognized value.
    #[test]
    fn multi_line_and_list_values_are_dropped_whole() {
        let fm = pairs(
            "---\nname: d\nallowed-tools:\n  - Bash\n  - Edit\npaths:\n  \"*.rs\": true\ndescription: after\n---\n",
        );
        assert_eq!(fm.get("name").map(String::as_str), Some("d"));
        assert_eq!(fm.get("description").map(String::as_str), Some("after"));
        assert!(!fm.contains_key("allowed-tools"), "{fm:?}");
        assert!(!fm.contains_key("paths"), "{fm:?}");
    }

    /// An inline list stays a string — we never look at this key, and mis-parsing it
    /// into something structured would be worse than ignoring it.
    #[test]
    fn inline_list_is_not_interpreted() {
        let fm = pairs("---\nallowed-tools: [Bash, Edit]\nname: x\n---\n");
        assert_eq!(fm.get("name").map(String::as_str), Some("x"));
        assert_eq!(
            fm.get("allowed-tools").map(String::as_str),
            Some("[Bash, Edit]")
        );
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let fm = pairs("---\n# a comment\n\nname: x\n---\n");
        assert_eq!(fm.len(), 1);
        assert_eq!(fm.get("name").map(String::as_str), Some("x"));
    }

    #[test]
    fn keys_are_case_insensitive() {
        let fm = pairs("---\nName: x\nWhen-To-Use: y\n---\n");
        assert_eq!(fm.get("name").map(String::as_str), Some("x"));
        assert_eq!(fm.get("when-to-use").map(String::as_str), Some("y"));
    }

    #[test]
    fn no_frontmatter_is_none() {
        assert!(parse("# Just markdown\n").is_none());
        assert!(parse("---\nname: x\n").is_none(), "unterminated block");
    }

    #[test]
    fn booleans() {
        for t in ["true", "TRUE", "yes", "on", "1"] {
            assert!(as_bool(t), "{t}");
        }
        for f in ["false", "no", "off", "0", "", "maybe"] {
            assert!(!as_bool(f), "{f}");
        }
    }
}
