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
