//! Project-root detection: the cwd→ancestor walk shared by the settings loader and
//! project-instruction discovery (ADR-0023 §2, *Root detection*).
//!
//! It lives here, in the trusted OS seam, because two unrelated consumers need it and
//! neither should own it: `locode-host`'s own settings loader, and the
//! `locode-instructions` crate split out on 2026-07-24 (ADR-0002 amendment). Keeping it
//! host-side is also what keeps that dependency one-way.

use std::path::{Path, PathBuf};

/// Ascend from `start`; the nearest ancestor containing any `markers` entry **or whose
/// absolute path matches `stop_pattern`** is the root (ADR-0023 rules 1+2). No hit up
/// to the filesystem root ⇒ cwd-only (returns `start`). The filesystem root is only a
/// backstop, never itself the project root.
///
/// Shared with the settings loader (which passes `stop_pattern = None` — the settings
/// files' own location is marker-detected only, avoiding a settings→pattern cycle).
#[must_use]
pub fn find_root_from_markers(
    start: &Path,
    markers: &[String],
    stop_pattern: Option<&regex::Regex>,
) -> PathBuf {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if markers.iter().any(|m| d.join(m).exists())
            || stop_pattern.is_some_and(|re| re.is_match(&d.to_string_lossy()))
        {
            return d.to_path_buf();
        }
        dir = d.parent();
    }
    start.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// What the pattern is actually matched against: the directory's path as a
    /// string, **without a trailing separator** (`Path::to_string_lossy` never adds
    /// one). So a suffix pattern must end at the directory name, not at a slash.
    #[test]
    fn stop_pattern_matches_a_path_with_no_trailing_slash() {
        let dir = tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let workspace = root.join("xx");
        let leaf = workspace.join("a").join("b");
        std::fs::create_dir_all(&leaf).unwrap();

        // Trailing-slash form: never matches.
        let re = regex::Regex::new("/xx/$").unwrap();
        assert_eq!(
            find_root_from_markers(&leaf, &[], Some(&re)),
            leaf,
            "`/xx/$` matches nothing, so the walk falls back to cwd-only"
        );

        // Without the trailing slash: matches the `xx` directory itself.
        let re = regex::Regex::new("/xx$").unwrap();
        assert_eq!(find_root_from_markers(&leaf, &[], Some(&re)), workspace);
    }

    /// `is_match` is a *search*, not a full match, so no leading `.*` is needed —
    /// and a literal `...` would impose a three-character minimum instead.
    #[test]
    fn stop_pattern_is_an_unanchored_search() {
        let dir = tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let workspace = root.join("xx");
        std::fs::create_dir_all(&workspace).unwrap();

        for pattern in ["/xx$", ".*/xx$"] {
            let re = regex::Regex::new(pattern).unwrap();
            assert_eq!(
                find_root_from_markers(&workspace, &[], Some(&re)),
                workspace,
                "{pattern} should match"
            );
        }
        // A trailing `/` in the pattern is the common mistake; keep it pinned.
        let re = regex::Regex::new(".*/xx/$").unwrap();
        assert_eq!(
            find_root_from_markers(&workspace, &[], Some(&re)),
            workspace,
            "no match ⇒ cwd-only fallback returns the start dir (which happens to be xx here)"
        );
    }

    /// The start directory itself is tested before ascending.
    #[test]
    fn the_start_dir_is_a_candidate() {
        let dir = tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let re =
            regex::Regex::new(&format!("^{}$", regex::escape(&root.to_string_lossy()))).unwrap();
        assert_eq!(find_root_from_markers(&root, &[], Some(&re)), root);
    }
}
