//! Assembling what a *user-invoked* skill sends to the model (ADR-0026 §8).
//!
//! The body goes in **verbatim**; arguments are appended as a plain-text block. There is
//! no template syntax — no `$ARGUMENTS`, no `${SKILL_DIR}` — and the reason is
//! consistency rather than simplicity: the model's own path opens `SKILL.md` with its
//! read tool (ADR-0025 §4), where nothing can substitute. Any template scheme would
//! therefore apply on the slash path only, and one skill would behave two ways
//! depending on who invoked it.
//!
//! Grok has exactly this append as the *fallback* under its substitution machinery
//! (`apply_substitutions`, the `**ARGUMENTS:**` suffix). We take the fallback as the
//! whole contract.

use std::path::Path;

use crate::frontmatter;

/// The marker introducing the argument block. A skill refers to arguments in ordinary
/// prose ("the request is in ARGUMENTS below"), so this string is the entire contract.
pub const ARGUMENTS_MARKER: &str = "**ARGUMENTS:**";

/// Read a `SKILL.md` and return its markdown body, frontmatter stripped.
///
/// Read at invocation rather than cached at discovery, so editing a skill takes effect
/// on the next use without waiting for a rescan.
///
/// # Errors
/// The underlying IO error when the file cannot be read.
pub fn read_body(path: &Path) -> std::io::Result<String> {
    let source = std::fs::read_to_string(path)?;
    Ok(frontmatter::parse(&source).map_or(source.clone(), |(_, body)| body.to_string()))
}

/// The text a user-invoked skill sends: the body, then the arguments block when there
/// are any.
///
/// With no arguments nothing is appended — a bare `**ARGUMENTS:**` with nothing after it
/// reads as "the user gave arguments and they were empty", which is a different claim
/// from "the user gave none".
#[must_use]
pub fn invocation_text(body: &str, args: &str) -> String {
    let body = body.trim_end();
    let args = args.trim();
    if args.is_empty() {
        return body.to_string();
    }
    format!("{body}\n\n{ARGUMENTS_MARKER} {args}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn body_is_verbatim_with_the_arguments_block_appended() {
        let out = invocation_text("# Commit\nStage, then commit.\n", "fix the typo");
        assert_eq!(
            out,
            "# Commit\nStage, then commit.\n\n**ARGUMENTS:** fix the typo"
        );
    }

    /// No arguments ⇒ no marker. An empty block would claim the user passed something.
    #[test]
    fn no_arguments_appends_nothing() {
        assert_eq!(invocation_text("# Commit\n", ""), "# Commit");
        assert_eq!(invocation_text("# Commit\n", "   "), "# Commit");
    }

    /// The body is never templated — a `$ARGUMENTS` an author wrote (or imported from
    /// another harness) reaches the model as written, which is the documented cost of
    /// ADR-0026 §8.
    #[test]
    fn template_tokens_in_the_body_are_left_alone() {
        let out = invocation_text("Use $ARGUMENTS and ${SKILL_DIR}/x.sh", "hello");
        assert!(out.contains("$ARGUMENTS and ${SKILL_DIR}/x.sh"), "{out}");
        assert!(out.ends_with("**ARGUMENTS:** hello"), "{out}");
    }

    #[test]
    fn read_body_strips_frontmatter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("SKILL.md");
        fs::write(&path, "---\nname: x\ndescription: d\n---\n# Body\ntext\n").unwrap();
        assert_eq!(read_body(&path).unwrap(), "# Body\ntext\n");
    }

    /// A file with no frontmatter is still usable as a body — discovery would have
    /// rejected it, but `read_body` is not the place to re-litigate that.
    #[test]
    fn read_body_without_frontmatter_returns_the_whole_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("SKILL.md");
        fs::write(&path, "# Just markdown\n").unwrap();
        assert_eq!(read_body(&path).unwrap(), "# Just markdown\n");
    }
}
