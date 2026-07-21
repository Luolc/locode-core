//! Gitignore-aware traversal + single-path ignore checks (Task 26 Slice 0).
//!
//! Grok Build is the model: its `list_dir` walks with
//! `WalkBuilder::standard_filters(true).git_ignore(f).git_global(f).git_exclude(f)`
//! (`gb/list_dir/mod.rs:282-291`), and its per-path guard is a session-cached
//! `ignore::gitignore::Gitignore` that allows everything when the workspace is
//! not a git repo (`gb/types/resources.rs:596-627`). Both live HERE — the host
//! seam — so pack tools stay free of direct filesystem access (ADR-0008;
//! plan-finalization Q1, 2026-07-21).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::Host;
use crate::fs::FsError;

/// Options for [`Host::walk`].
#[derive(Debug, Clone, Copy, Default)]
pub struct WalkOptions {
    /// Honor `.gitignore` / global gitignore / `.git/info/exclude`
    /// (grok's three `git_*` builder flags move together).
    pub respect_gitignore: bool,
    /// Maximum depth below the root (`None` = unbounded; `Some(1)` = direct
    /// children — grok's depth-1 seed walk).
    pub max_depth: Option<usize>,
    /// Stop after collecting this many entries (`None` = unbounded). The walk
    /// is depth-first in the `ignore` crate's default order; callers impose
    /// their own ordering/budgeting on the returned entries.
    pub max_entries: Option<usize>,
}

/// One traversal entry (the root itself is not reported).
#[derive(Debug, Clone)]
pub struct WalkEntry {
    /// Absolute path.
    pub path: PathBuf,
    /// Depth below the walk root (direct children = 1).
    pub depth: usize,
    /// Whether the entry is a directory.
    pub is_dir: bool,
}

/// The lazily-built workspace gitignore matcher: `None` when the workspace is
/// not inside a git repo (allow-all, grok's rule).
pub(crate) struct GitignoreCache(OnceLock<Option<(ignore::gitignore::Gitignore, PathBuf)>>);

impl GitignoreCache {
    pub(crate) const fn new() -> Self {
        Self(OnceLock::new())
    }
}

impl Clone for GitignoreCache {
    fn clone(&self) -> Self {
        let cell = OnceLock::new();
        if let Some(v) = self.0.get() {
            let _ = cell.set(v.clone());
        }
        Self(cell)
    }
}

impl std::fmt::Debug for GitignoreCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GitignoreCache")
    }
}

/// Find the enclosing git root (a dir containing `.git`) at or above `start`.
fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

impl Host {
    /// Walk `path` (jail-resolved against `cwd`) with grok-style standard
    /// filters. Hidden files and `.git` are always skipped
    /// (`standard_filters(true)`); the three gitignore sources are toggled
    /// together by [`WalkOptions::respect_gitignore`].
    ///
    /// # Errors
    /// [`FsError::Path`] when the jail rejects `path`; [`FsError::Io`] when
    /// the root does not exist or is unreadable.
    pub async fn walk(
        &self,
        cwd: &Path,
        path: &Path,
        opts: WalkOptions,
    ) -> Result<Vec<WalkEntry>, FsError> {
        let root = self.resolve_in_jail(cwd, path).await?;
        // Surface a missing/unreadable root as the usual Io error (the walker
        // itself yields per-entry errors we skip).
        tokio::fs::metadata(&root).await.map_err(|e| FsError::Io {
            op: "walk",
            path: root.display().to_string(),
            source: e,
        })?;
        let entries = tokio::task::spawn_blocking(move || {
            let mut builder = ignore::WalkBuilder::new(&root);
            builder
                .standard_filters(true)
                .git_ignore(opts.respect_gitignore)
                .git_global(opts.respect_gitignore)
                .git_exclude(opts.respect_gitignore);
            if let Some(depth) = opts.max_depth {
                builder.max_depth(Some(depth));
            }
            let mut out = Vec::new();
            for entry in builder.build() {
                let Ok(entry) = entry else { continue };
                if entry.depth() == 0 {
                    continue; // the root itself
                }
                let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                let depth = entry.depth();
                out.push(WalkEntry {
                    path: entry.into_path(),
                    depth,
                    is_dir,
                });
                if let Some(cap) = opts.max_entries
                    && out.len() >= cap
                {
                    break;
                }
            }
            out
        })
        .await
        .map_err(|e| FsError::Io {
            op: "walk",
            path: String::new(),
            source: std::io::Error::other(e),
        })?;
        Ok(entries)
    }

    /// Whether `path` (jail-resolved against `cwd`) is gitignored relative to
    /// the workspace's git root. `false` when the workspace is not a git repo
    /// (grok's allow-all rule). Non-existent paths fall back to their parent
    /// for resolution (grok's new-file-creation case).
    ///
    /// The matcher is built once per host from the git root's `.gitignore` and
    /// `.git/info/exclude`. Known limitation vs grok (documented): nested
    /// `.gitignore` files in subdirectories are not consulted here — the
    /// [`Host::walk`] path handles them fully via the walker.
    ///
    /// # Errors
    /// [`FsError::Path`] when the jail rejects `path`.
    pub async fn is_path_ignored(&self, cwd: &Path, path: &Path) -> Result<bool, FsError> {
        let resolved = self.resolve_in_jail(cwd, path).await?;
        let cached = self.gitignore.0.get_or_init(|| {
            let git_root = find_git_root(&self.workspace_root)?;
            let mut builder = ignore::gitignore::GitignoreBuilder::new(&git_root);
            let _ = builder.add(git_root.join(".gitignore"));
            let _ = builder.add(git_root.join(".git").join("info").join("exclude"));
            let gitignore = builder.build().ok()?;
            Some((gitignore, git_root))
        });
        let Some((gitignore, _git_root)) = cached else {
            return Ok(false);
        };
        // grok's normalization: canonicalize, falling back to parent + file
        // name for paths that don't exist yet (`resources.rs:615-624`).
        let normalized = std::fs::canonicalize(&resolved).unwrap_or_else(|_| {
            resolved
                .parent()
                .and_then(|parent| {
                    std::fs::canonicalize(parent)
                        .ok()
                        .map(|p| p.join(resolved.file_name().unwrap_or_default()))
                })
                .unwrap_or_else(|| resolved.clone())
        });
        let is_dir = normalized.is_dir();
        Ok(gitignore
            .matched_path_or_any_parents(&normalized, is_dir)
            .is_ignore())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HostConfig;

    fn host_over(dir: &Path) -> Host {
        Host::new(HostConfig::new(dir)).unwrap()
    }

    fn git_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n*.log\n").unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/out.o"), "o").unwrap();
        std::fs::write(dir.path().join("app.log"), "log").unwrap();
        dir
    }

    #[tokio::test]
    async fn walk_respects_gitignore_when_asked() {
        let dir = git_workspace();
        let host = host_over(dir.path());
        let root = host.workspace_root().to_path_buf();
        let opts = WalkOptions {
            respect_gitignore: true,
            ..WalkOptions::default()
        };
        let entries = host.walk(&root, Path::new("."), opts).await.unwrap();
        let names: Vec<String> = entries
            .iter()
            .map(|e| e.path.strip_prefix(&root).unwrap().display().to_string())
            .collect();
        assert!(names.contains(&"src/main.rs".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.starts_with("target")), "{names:?}");
        assert!(!names.contains(&"app.log".to_string()), "{names:?}");
    }

    #[tokio::test]
    async fn walk_includes_ignored_when_not_asked_and_honors_depth() {
        let dir = git_workspace();
        let host = host_over(dir.path());
        let root = host.workspace_root().to_path_buf();
        let opts = WalkOptions {
            respect_gitignore: false,
            max_depth: Some(1),
            max_entries: None,
        };
        let entries = host.walk(&root, Path::new("."), opts).await.unwrap();
        assert!(entries.iter().all(|e| e.depth == 1), "depth-1 seed only");
        let names: Vec<String> = entries
            .iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"app.log".to_string()), "{names:?}");
        assert!(names.contains(&"target".to_string()), "{names:?}");
        assert!(
            !names.contains(&".git".to_string()),
            "standard filters skip .git"
        );
    }

    #[tokio::test]
    async fn is_path_ignored_matches_and_allows() {
        let dir = git_workspace();
        let host = host_over(dir.path());
        let root = host.workspace_root().to_path_buf();
        assert!(
            host.is_path_ignored(&root, Path::new("app.log"))
                .await
                .unwrap()
        );
        assert!(
            host.is_path_ignored(&root, Path::new("target/out.o"))
                .await
                .unwrap()
        );
        assert!(
            !host
                .is_path_ignored(&root, Path::new("src/main.rs"))
                .await
                .unwrap()
        );
        // Non-existent (new-file) path under an ignored pattern.
        assert!(
            host.is_path_ignored(&root, Path::new("new.log"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn non_git_workspace_allows_everything() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.log"), "x").unwrap();
        let host = host_over(dir.path());
        let root = host.workspace_root().to_path_buf();
        assert!(
            !host
                .is_path_ignored(&root, Path::new("app.log"))
                .await
                .unwrap()
        );
    }
}
