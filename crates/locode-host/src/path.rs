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
    /// Resolve `candidate` (absolute, `~`-prefixed, or relative to `cwd`) to a concrete
    /// absolute path.
    ///
    /// A leading `~` or `~/` is expanded against `$HOME` **before** anything else, so a
    /// model-supplied `~/.config/x` names the real home path instead of a literal `~`
    /// directory under `cwd`. Only `~` and `~/…` expand; `~user` is left alone, and with
    /// no `$HOME` the path is unchanged. Expansion is not a permission:
    /// the expanded path still faces the jail below, and under [`PathPolicy::Jailed`] a
    /// home path outside the workspace is rejected as an escape — the point is that the
    /// rejection now names the path the caller actually meant.
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
        let candidate = expand_home_prefix(candidate, home_dir().as_deref());
        let absolute = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            cwd.join(&*candidate)
        };
        let normalized = normalize_lexical(&absolute);

        if self.path_policy == PathPolicy::Unrestricted {
            return Ok(normalized);
        }

        // (1) Lexical prefix check — catches `../etc/passwd`, `/etc/passwd`, `a/../../b`.
        //     A path may sit under the workspace root OR any `--add-dir` root; both
        //     lists are already canonical, and both checks below use the same set so a
        //     symlink cannot enter through one root and escape via another.
        if !self.is_under_a_root(&normalized) && !self.is_under_a_root_as_given(&normalized) {
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
        if !self.is_under_a_root(&canonical_ancestor) {
            return Err(PathError::Escape(normalized.display().to_string()));
        }
        Ok(normalized)
    }

    /// Whether `path` sits under the workspace root or any `--add-dir` root.
    ///
    /// Roots are canonical by construction ([`Host::new`]), so this is a pure
    /// prefix test. Extra roots widen the jail additively: they never relax the
    /// checks applied to the primary root, and a path outside every root is still
    /// an escape.
    fn is_under_a_root(&self, path: &Path) -> bool {
        if path.starts_with(&self.workspace_root) {
            return true;
        }
        // The guard is taken and dropped inside this call — never held across
        // the `await` in `resolve_in_jail`.
        let roots = self
            .extra_roots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        roots.iter().any(|root| path.starts_with(root))
    }

    /// The lexical pre-check's second form: roots as the caller wrote them.
    ///
    /// Only ever *widens* step (1), which is a cheap filter — step (2) then
    /// canonicalizes and re-checks against the canonical roots, so a symlink
    /// that actually leaves every root is still rejected.
    fn is_under_a_root_as_given(&self, path: &Path) -> bool {
        let roots = self
            .roots_as_given
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        roots.iter().any(|root| path.starts_with(root))
    }
}

/// The user's home directory (`$HOME`), or `None` when it is unset or empty.
///
/// Deliberately `$HOME` and **not** `$LOCODE_HOME`: `~` is the OS-level home in every
/// shell and every surveyed harness, while `$LOCODE_HOME` (`crate::locode_home`) only
/// relocates *our* dotfolder. Conflating them would make `~/x` mean different files
/// depending on an unrelated override.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Expand a leading `~` / `~/…` against `home`, borrowing when there is nothing to do.
///
/// Only the bare `~` and the `~/` prefix are recognized — `~user` is left untouched, as
/// in both reference harnesses (Claude Code's `expandPath`, `src/utils/path.ts:57-64`,
/// and grok's `shellexpand::tilde`), because resolving another user's home needs a
/// passwd lookup we have no reason to perform. With no `$HOME` the path is returned
/// unchanged rather than guessed at.
fn expand_home_prefix<'a>(path: &'a Path, home: Option<&Path>) -> std::borrow::Cow<'a, Path> {
    use std::borrow::Cow;

    // Operate on the raw bytes/str form: `Path::components` would already have split
    // `~/foo` into `~` + `foo`, and a bare `~` must map to the home dir itself.
    let Some(raw) = path.to_str() else {
        return Cow::Borrowed(path);
    };
    let Some(home) = home else {
        return Cow::Borrowed(path);
    };
    if raw == "~" {
        return Cow::Owned(home.to_path_buf());
    }
    match raw.strip_prefix("~/") {
        // `~/` alone (and `~//x`) would `join("")`/absolutize oddly; trim leading slashes
        // so the result is always `<home>/<rest>`.
        Some(rest) => Cow::Owned(home.join(rest.trim_start_matches('/'))),
        None => Cow::Borrowed(path),
    }
}

/// Crate-internal alias so `Host::new` can normalize roots before canonicalizing.
pub(crate) fn normalize_lexical_pub(path: &Path) -> PathBuf {
    normalize_lexical(path)
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

    /// `--add-dir` widens the jail additively: a path under an extra root
    /// resolves, while anything outside *every* root is still an escape.
    #[tokio::test]
    async fn extra_roots_widen_the_jail_without_opening_it() {
        let main = tempdir().expect("main root");
        let extra = tempdir().expect("extra root");
        let outside = tempdir().expect("unrelated dir");

        let mut config = crate::HostConfig::new(main.path());
        config.extra_roots = vec![extra.path().to_path_buf()];
        let host = crate::Host::new(config).expect("host with an extra root");
        let cwd = host.workspace_root().to_path_buf();

        // The primary root still works.
        host.resolve_in_jail(&cwd, Path::new("in_main.txt"))
            .await
            .expect("primary root still resolves");

        // The added root is reachable by absolute path, including a leaf that
        // does not exist yet (create-new-file must keep working there).
        let added_file = extra.path().join("subdir/new.txt");
        host.resolve_in_jail(&cwd, &added_file)
            .await
            .expect("a path under an --add-dir root resolves");

        // Everything else is still an escape — widening is not disabling.
        let err = host
            .resolve_in_jail(&cwd, &outside.path().join("secret.txt"))
            .await
            .expect_err("an unrelated directory is still out of bounds");
        assert!(matches!(err, PathError::Escape(_)), "{err:?}");

        let err = host
            .resolve_in_jail(&cwd, Path::new("/etc/passwd"))
            .await
            .expect_err("absolute escapes are unaffected by extra roots");
        assert!(matches!(err, PathError::Escape(_)), "{err:?}");
    }

    /// `/add-dir` widens a **running** host: a path rejected a moment ago
    /// resolves once its root is added, without rebuilding anything.
    #[tokio::test]
    async fn add_root_widens_a_live_host() {
        let main = tempdir().expect("main root");
        let later = tempdir().expect("root added later");
        let host = test_host(main.path(), PathPolicy::Jailed, false);
        let cwd = host.workspace_root().to_path_buf();
        let target = later.path().join("file.txt");

        let err = host
            .resolve_in_jail(&cwd, &target)
            .await
            .expect_err("out of bounds before the root is added");
        assert!(matches!(err, PathError::Escape(_)), "{err:?}");

        host.add_root(later.path()).expect("root added");
        host.resolve_in_jail(&cwd, &target)
            .await
            .expect("reachable once added");

        // Idempotent: repeating the command must not error or duplicate.
        host.add_root(later.path())
            .expect("adding twice is a no-op");
        host.resolve_in_jail(&cwd, &target)
            .await
            .expect("still fine");

        // A clone shares the list — tools already hold their own `Arc<Host>`.
        let clone = host.clone();
        clone
            .resolve_in_jail(&cwd, &target)
            .await
            .expect("clones see the widened jail");

        // Widening one root does not open others.
        let elsewhere = tempdir().expect("unrelated");
        let err = host
            .resolve_in_jail(&cwd, &elsewhere.path().join("x"))
            .await
            .expect_err("unrelated dirs stay out");
        assert!(matches!(err, PathError::Escape(_)), "{err:?}");
    }

    /// A bad path leaves the jail untouched.
    #[tokio::test]
    async fn add_root_rejects_a_missing_directory() {
        let main = tempdir().expect("main root");
        let host = test_host(main.path(), PathPolicy::Jailed, false);
        let err = host
            .add_root(&main.path().join("nope"))
            .expect_err("missing directory");
        assert!(matches!(err, PathError::InvalidRoot(msg) if msg.contains("nope")));
    }

    /// A non-existent `--add-dir` fails at construction, naming the directory —
    /// never a silently narrower jail.
    #[test]
    fn a_missing_extra_root_is_a_construction_error() {
        let main = tempdir().expect("main root");
        let mut config = crate::HostConfig::new(main.path());
        config.extra_roots = vec![main.path().join("does-not-exist")];
        let err = crate::Host::new(config).expect_err("must not silently drop the root");
        assert!(matches!(err, PathError::InvalidRoot(msg) if msg.contains("does-not-exist")));
    }

    #[test]
    fn expands_bare_tilde_and_tilde_slash() {
        let home = Path::new("/home/u");
        let cases = [
            ("~", "/home/u"),
            ("~/", "/home/u"),
            ("~/.locode/settings.json", "/home/u/.locode/settings.json"),
            ("~//nested", "/home/u/nested"),
        ];
        for (raw, want) in cases {
            let got = expand_home_prefix(Path::new(raw), Some(home));
            assert_eq!(normalize_lexical(&got), Path::new(want), "{raw}");
        }
    }

    #[test]
    fn leaves_non_home_prefixes_alone() {
        let home = Path::new("/home/u");
        // `~user` needs a passwd lookup we do not do; `~` mid-path is an ordinary name;
        // and a file literally called `~x` must not become a home path.
        for raw in [
            "~root/x",
            "~x",
            "a/~/b",
            "./~",
            "relative/path",
            "/abs/path",
        ] {
            let got = expand_home_prefix(Path::new(raw), Some(home));
            assert_eq!(got.as_ref(), Path::new(raw), "{raw} must be untouched");
        }
    }

    #[test]
    fn without_home_the_path_is_unchanged() {
        let got = expand_home_prefix(Path::new("~/x"), None);
        assert_eq!(got.as_ref(), Path::new("~/x"));
    }

    #[tokio::test]
    async fn tilde_resolves_to_the_real_home_not_a_cwd_subdir() {
        // The reported bug: `~/…` took the relative branch and became `<cwd>/~/…`, a
        // path that sits *inside* the jail, so both checks passed and the read failed
        // with a bogus "not found" for a directory literally named `~`.
        let Some(home) = home_dir() else {
            return; // no $HOME in this environment; nothing to assert
        };
        let dir = tempdir().unwrap();
        let host = test_host(dir.path(), PathPolicy::Unrestricted, false);
        let cwd = host.workspace_root().to_path_buf();

        let resolved = host
            .resolve_in_jail(&cwd, Path::new("~/.locode/settings.json"))
            .await
            .expect("unrestricted resolve");
        assert_eq!(resolved, home.join(".locode/settings.json"));
        assert!(!resolved.starts_with(&cwd), "must not land under cwd");
    }

    #[tokio::test]
    async fn jailed_tilde_escape_names_the_expanded_path() {
        // Expansion is not a permission: outside the workspace, `~/…` is still an
        // escape — but the error now names the home path the caller meant.
        let Some(home) = home_dir() else {
            return;
        };
        let dir = tempdir().unwrap();
        let host = test_host(dir.path(), PathPolicy::Jailed, false);
        let cwd = host.workspace_root().to_path_buf();

        let err = host
            .resolve_in_jail(&cwd, Path::new("~/.locode/settings.json"))
            .await
            .expect_err("home is outside the workspace root");
        match err {
            PathError::Escape(shown) => {
                assert!(shown.starts_with(&home.display().to_string()), "{shown}");
                assert!(
                    !shown.contains('~'),
                    "the literal tilde must be gone: {shown}"
                );
            }
            other => panic!("expected Escape, got {other:?}"),
        }
    }

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
