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

    /// Stat a file (jail-resolved).
    ///
    /// # Errors
    /// [`FsError::Path`] if the path escapes the jail; [`FsError::Io`] if the stat fails.
    pub async fn stat(&self, cwd: &Path, path: &Path) -> Result<FileStat, FsError> {
        let resolved = self.resolve_in_jail(cwd, path).await?;
        self.stat_resolved(&resolved).await
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
}
