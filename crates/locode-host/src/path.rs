//! The path jail: resolve a model-supplied path under the workspace root, or reject it.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use crate::{Host, PathPolicy};

/// A path-jail failure.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    /// The path resolves outside the workspace root.
    #[error("path escapes the workspace root: {0}")]
    Escape(String),
    /// The workspace root itself is invalid (does not exist / cannot canonicalize).
    #[error("workspace root is invalid: {0}")]
    InvalidRoot(String),
    /// An IO error while resolving.
    #[error("io error resolving {path}: {source}")]
    Io {
        /// The path being resolved.
        path: String,
        /// The underlying IO error.
        source: std::io::Error,
    },
}

impl Host {
    /// Resolve `candidate` (absolute, or relative to `cwd`) to a concrete absolute path.
    ///
    /// Under [`PathPolicy::Jailed`] the result is guaranteed to live under the workspace
    /// root — `..`, absolute, and **symlink** escapes are rejected — while paths whose
    /// leaf does not yet exist (create-new-file) are still allowed. Under
    /// [`PathPolicy::Unrestricted`] the path is only made absolute; nothing is rejected.
    ///
    /// # Errors
    /// [`PathError::Escape`] if a jailed path escapes the root; [`PathError::Io`] if an
    /// ancestor cannot be canonicalized.
    pub async fn resolve_in_jail(
        &self,
        cwd: &Path,
        candidate: &Path,
    ) -> Result<PathBuf, PathError> {
        let absolute = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            cwd.join(candidate)
        };
        let normalized = normalize_lexical(&absolute);

        if self.path_policy == PathPolicy::Unrestricted {
            return Ok(normalized);
        }

        // (1) Lexical prefix check — catches `../etc/passwd`, `/etc/passwd`, `a/../../b`.
        if !normalized.starts_with(&self.workspace_root) {
            return Err(PathError::Escape(normalized.display().to_string()));
        }
        // (2) Symlink check — canonicalize the deepest *existing* ancestor and confirm it
        // is still under the (already-canonical) root. Catches an in-jail symlink that
        // points out, while still allowing a not-yet-existing leaf.
        let canonical_ancestor =
            canonicalize_existing_ancestor(&normalized)
                .await
                .map_err(|source| PathError::Io {
                    path: normalized.display().to_string(),
                    source,
                })?;
        if !canonical_ancestor.starts_with(&self.workspace_root) {
            return Err(PathError::Escape(normalized.display().to_string()));
        }
        Ok(normalized)
    }
}

/// Syntactically resolve `.`/`..` without touching the filesystem.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalize the deepest ancestor of `path` that actually exists (symlink-resolving).
async fn canonicalize_existing_ancestor(path: &Path) -> std::io::Result<PathBuf> {
    let mut current = path;
    loop {
        match tokio::fs::canonicalize(current).await {
            Ok(canonical) => return Ok(canonical),
            Err(e) if e.kind() == ErrorKind::NotFound => match current.parent() {
                Some(parent) => current = parent,
                None => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PathPolicy, test_host};
    use tempfile::tempdir;

    #[tokio::test]
    async fn rejects_parent_and_absolute_escapes() {
        let dir = tempdir().unwrap();
        let host = test_host(dir.path(), PathPolicy::Jailed, false);
        let root = host.workspace_root().to_path_buf();

        for bad in ["../etc/passwd", "/etc/passwd", "a/../../b"] {
            let err = host
                .resolve_in_jail(&root, Path::new(bad))
                .await
                .expect_err(bad);
            assert!(matches!(err, PathError::Escape(_)), "{bad} should escape");
        }
    }

    #[tokio::test]
    async fn allows_paths_in_jail_including_nonexistent_leaf() {
        let dir = tempdir().unwrap();
        let host = test_host(dir.path(), PathPolicy::Jailed, false);
        let root = host.workspace_root().to_path_buf();

        let resolved = host
            .resolve_in_jail(&root, Path::new("newdir/new.txt"))
            .await
            .expect("nonexistent leaf under root is allowed");
        assert!(resolved.starts_with(&root));

        // Relative resolves against cwd.
        let sub = root.join("sub");
        let resolved = host
            .resolve_in_jail(&sub, Path::new("f.txt"))
            .await
            .expect("relative under cwd");
        assert_eq!(resolved, sub.join("f.txt"));
    }

    #[cfg(unix)]
    // `std::os::unix::fs::symlink` — does not even compile off unix.
    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlink_escape() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let host = test_host(dir.path(), PathPolicy::Jailed, false);
        let root = host.workspace_root().to_path_buf();

        // A symlink inside the jail pointing outside it.
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
        let err = host
            .resolve_in_jail(&root, Path::new("link/secret.txt"))
            .await
            .expect_err("symlink escape");
        assert!(matches!(err, PathError::Escape(_)));
    }

    // `/etc/hostname` and rootless absolute-path semantics are unix-only.
    #[cfg(unix)]
    #[tokio::test]
    async fn unrestricted_allows_escapes() {
        let dir = tempdir().unwrap();
        let host = test_host(dir.path(), PathPolicy::Unrestricted, false);
        let root = host.workspace_root().to_path_buf();

        let resolved = host
            .resolve_in_jail(&root, Path::new("/etc/hostname"))
            .await
            .expect("unrestricted allows absolute out-of-root");
        assert_eq!(resolved, Path::new("/etc/hostname"));
    }
}
