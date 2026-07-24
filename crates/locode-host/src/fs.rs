//! Jailed filesystem helpers: read / write / stat, all resolving through the jail first.

use std::path::Path;
use std::time::SystemTime;

use crate::Host;
use crate::path::PathError;

/// A file's contents plus its stat (the stat is the freshness token edits compare).
#[derive(Debug, Clone)]
pub struct FileRead {
    /// The file contents, lossy UTF-8 (binary files degrade rather than error).
    pub contents: String,
    /// The file's size + mtime at read time.
    pub stat: FileStat,
}

/// A file's size and last-modified time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    /// Length in bytes.
    pub len: u64,
    /// Last-modified time, if the platform reports it (the edit-freshness token).
    pub modified: Option<SystemTime>,
}

/// One entry in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// The entry's file name (not a full path).
    pub name: String,
    /// Whether the entry is a directory.
    pub is_dir: bool,
}

/// A filesystem operation failure.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    /// The path was rejected by the jail.
    #[error(transparent)]
    Path(#[from] PathError),
    /// An IO error performing `op`.
    #[error("{op} failed for {path}: {source}")]
    Io {
        /// The operation that failed (`read`/`write`/`stat`).
        op: &'static str,
        /// The path involved.
        path: String,
        /// The underlying IO error.
        source: std::io::Error,
    },
}

impl Host {
    /// Read a file (jail-resolved), returning its lossy-UTF-8 contents + stat.
    ///
    /// # Errors
    /// [`FsError::Path`] if the path escapes the jail; [`FsError::Io`] if the read fails.
    pub async fn read_file(&self, cwd: &Path, path: &Path) -> Result<FileRead, FsError> {
        let resolved = self.resolve_in_jail(cwd, path).await?;
        let bytes = tokio::fs::read(&resolved)
            .await
            .map_err(|source| FsError::Io {
                op: "read",
                path: resolved.display().to_string(),
                source,
            })?;
        let contents = String::from_utf8_lossy(&bytes).into_owned();
        let stat = self.stat_resolved(&resolved).await?;
        Ok(FileRead { contents, stat })
    }

    /// Create-or-overwrite a file (jail-resolved), returning its post-write stat.
    ///
    /// Does **not** auto-create parent directories (a mistyped nested path silently
    /// creating dirs is a footgun); a missing parent surfaces as an IO error.
    ///
    /// # Errors
    /// [`FsError::Path`] if the path escapes the jail; [`FsError::Io`] if the write fails.
    pub async fn write_file(
        &self,
        cwd: &Path,
        path: &Path,
        contents: &str,
    ) -> Result<FileStat, FsError> {
        let resolved = self.resolve_in_jail(cwd, path).await?;
        tokio::fs::write(&resolved, contents)
            .await
            .map_err(|source| FsError::Io {
                op: "write",
                path: resolved.display().to_string(),
                source,
            })?;
        self.stat_resolved(&resolved).await
    }

    /// Create a directory (jail-resolved) — an explicit, opt-in `mkdir`, separate
    /// from [`Host::write_file`], which deliberately never auto-creates dirs. A
    /// pack calls this when it wants a harness's real "create the parent dir on
    /// write" behavior (e.g. Claude Code's `Edit`/`Write`), without changing the
    /// footgun-avoidance default for everyone else.
    ///
    /// - `parents` — create missing intermediate dirs (`mkdir -p`); when `false`,
    ///   a missing parent is an error.
    /// - `exist_ok` — succeed if the target dir already exists; when `false`, an
    ///   existing target is an error (`mkdir` without `-p`'s tolerance).
    ///
    /// # Errors
    /// [`FsError::Path`] if the path escapes the jail; [`FsError::Io`] on a missing
    /// parent (when `!parents`), an existing target (when `!exist_ok`), or any other
    /// IO failure.
    pub async fn create_dir(
        &self,
        cwd: &Path,
        path: &Path,
        parents: bool,
        exist_ok: bool,
    ) -> Result<(), FsError> {
        let resolved = self.resolve_in_jail(cwd, path).await?;
        let already_exists = tokio::fs::metadata(&resolved)
            .await
            .is_ok_and(|m| m.is_dir());
        if already_exists {
            if exist_ok {
                return Ok(());
            }
            return Err(FsError::Io {
                op: "create_dir",
                path: resolved.display().to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "directory already exists",
                ),
            });
        }
        let result = if parents {
            tokio::fs::create_dir_all(&resolved).await
        } else {
            tokio::fs::create_dir(&resolved).await
        };
        result.map_err(|source| FsError::Io {
            op: "create_dir",
            path: resolved.display().to_string(),
            source,
        })
    }

    /// Remove a single file (jail-resolved). Never recursive — a directory target
    /// surfaces as an IO error (codex's `apply_patch` refuses to delete a directory).
    /// The explicit delete seam for a pack that ports a real "delete file" behavior
    /// (codex `apply_patch`'s `*** Delete File:` and the source side of `*** Move to:`).
    ///
    /// # Errors
    /// [`FsError::Path`] if the path escapes the jail; [`FsError::Io`] if the removal
    /// fails (missing file, a directory target, or any other IO failure).
    pub async fn remove_file(&self, cwd: &Path, path: &Path) -> Result<(), FsError> {
        let resolved = self.resolve_in_jail(cwd, path).await?;
        tokio::fs::remove_file(&resolved)
            .await
            .map_err(|source| FsError::Io {
                op: "remove",
                path: resolved.display().to_string(),
                source,
            })
    }

    /// Stat a file (jail-resolved).
    ///
    /// # Errors
    /// [`FsError::Path`] if the path escapes the jail; [`FsError::Io`] if the stat fails.
    pub async fn stat(&self, cwd: &Path, path: &Path) -> Result<FileStat, FsError> {
        let resolved = self.resolve_in_jail(cwd, path).await?;
        self.stat_resolved(&resolved).await
    }

    /// List a directory's immediate entries (jail-resolved), unsorted.
    ///
    /// # Errors
    /// [`FsError::Path`] if the path escapes the jail; [`FsError::Io`] if it is not a
    /// readable directory.
    pub async fn read_dir(&self, cwd: &Path, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let resolved = self.resolve_in_jail(cwd, path).await?;
        let mut reader = tokio::fs::read_dir(&resolved)
            .await
            .map_err(|source| FsError::Io {
                op: "read_dir",
                path: resolved.display().to_string(),
                source,
            })?;
        let mut entries = Vec::new();
        while let Some(entry) = reader.next_entry().await.map_err(|source| FsError::Io {
            op: "read_dir",
            path: resolved.display().to_string(),
            source,
        })? {
            let is_dir = entry.file_type().await.is_ok_and(|t| t.is_dir());
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir,
            });
        }
        Ok(entries)
    }

    async fn stat_resolved(&self, resolved: &Path) -> Result<FileStat, FsError> {
        let metadata = tokio::fs::metadata(resolved)
            .await
            .map_err(|source| FsError::Io {
                op: "stat",
                path: resolved.display().to_string(),
                source,
            })?;
        Ok(FileStat {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PathPolicy, test_host};
    use tempfile::tempdir;

    #[tokio::test]
    async fn write_then_read_roundtrip_in_jail() {
        let dir = tempdir().unwrap();
        let host = test_host(dir.path(), PathPolicy::Jailed, false);
        let root = host.workspace_root().to_path_buf();

        let stat = host
            .write_file(&root, Path::new("a.txt"), "hello")
            .await
            .unwrap();
        assert_eq!(stat.len, 5);

        let read = host.read_file(&root, Path::new("a.txt")).await.unwrap();
        assert_eq!(read.contents, "hello");
        assert!(read.stat.modified.is_some());
    }

    #[tokio::test]
    async fn read_outside_jail_is_a_path_error() {
        let dir = tempdir().unwrap();
        let host = test_host(dir.path(), PathPolicy::Jailed, false);
        let root = host.workspace_root().to_path_buf();

        let err = host
            .read_file(&root, Path::new("/etc/passwd"))
            .await
            .expect_err("jail rejection");
        assert!(matches!(err, FsError::Path(_)));
    }

    #[tokio::test]
    async fn create_dir_makes_parents_and_is_exist_ok() {
        let dir = tempdir().unwrap();
        let host = test_host(dir.path(), PathPolicy::Jailed, false);
        let root = host.workspace_root().to_path_buf();

        // mkdir -p: intermediate dirs created.
        host.create_dir(&root, Path::new("a/b/c"), true, true)
            .await
            .unwrap();
        assert!(root.join("a/b/c").is_dir());
        // exist_ok = true: creating it again succeeds.
        host.create_dir(&root, Path::new("a/b/c"), true, true)
            .await
            .unwrap();
        // exist_ok = false on an existing dir errors.
        let err = host
            .create_dir(&root, Path::new("a/b/c"), true, false)
            .await
            .expect_err("existing dir with exist_ok=false");
        assert!(matches!(err, FsError::Io { .. }));
        // parents = false with a missing parent errors.
        let err = host
            .create_dir(&root, Path::new("x/y/z"), false, true)
            .await
            .expect_err("missing parent with parents=false");
        assert!(matches!(err, FsError::Io { .. }));
        // A new dir directly under root, no parents needed.
        host.create_dir(&root, Path::new("solo"), false, true)
            .await
            .unwrap();
        assert!(root.join("solo").is_dir());
    }

    #[tokio::test]
    async fn create_dir_outside_jail_is_a_path_error() {
        let dir = tempdir().unwrap();
        let host = test_host(dir.path(), PathPolicy::Jailed, false);
        let root = host.workspace_root().to_path_buf();
        let err = host
            .create_dir(&root, Path::new("/tmp/escape"), true, true)
            .await
            .expect_err("jail rejection");
        assert!(matches!(err, FsError::Path(_)));
    }
}
