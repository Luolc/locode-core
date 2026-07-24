//! `~/.locode` home-directory resolution (ADR-0024).
//!
//! One resolver for every `~/.locode` consumer (settings, traces, skills, the global
//! `AGENTS.md`): `$LOCODE_HOME` when set — which **must exist and canonicalize** (the
//! Codex contract, so a typo'd override fails loudly instead of silently reading an
//! empty tree) — else `$HOME/.locode`, deliberately *unverified* (the default may not
//! exist yet; readers tolerate absence, writers create it).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The resolved home, memoized for the process (env is read once).
///
/// # Errors
/// A human-readable message when an **explicitly set** `$LOCODE_HOME` does not exist
/// or cannot be canonicalized, or when neither variable resolves. Callers surface it
/// as a warning (degrading to "no home") or a pre-run error as appropriate.
pub fn locode_home() -> Result<PathBuf, String> {
    static HOME: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    HOME.get_or_init(|| {
        resolve_home_from(std::env::var_os("LOCODE_HOME"), std::env::var_os("HOME"))
    })
    .clone()
}

/// The *default* home (`$HOME/.locode`), ignoring any `$LOCODE_HOME` override — so a
/// caller can detect "am I on the default" (grok's `default_grok_home` split).
#[must_use]
pub fn default_locode_home() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".locode"))
}

/// The env-free core of [`locode_home`] (tests inject values — env is process-global
/// and this crate forbids `unsafe` env mutation).
pub(crate) fn resolve_home_from(
    locode_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, String> {
    if let Some(dir) = locode_home.filter(|d| !d.is_empty()) {
        let dir = PathBuf::from(dir);
        // Explicit override: must exist + canonicalize (catch typos loudly).
        return std::fs::canonicalize(&dir)
            .map_err(|e| format!("LOCODE_HOME `{}`: {e}", dir.display()))
            .and_then(|canon| {
                if canon.is_dir() {
                    Ok(canon)
                } else {
                    Err(format!(
                        "LOCODE_HOME `{}` is not a directory",
                        dir.display()
                    ))
                }
            });
    }
    let home = home
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "neither LOCODE_HOME nor HOME is set".to_string())?;
    Ok(PathBuf::from(home).join(".locode"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_falls_back_to_home_dot_locode() {
        let got = resolve_home_from(None, Some(OsString::from("/home/u"))).unwrap();
        assert_eq!(got, PathBuf::from("/home/u/.locode"));
        // Empty LOCODE_HOME is treated as unset.
        let got =
            resolve_home_from(Some(OsString::new()), Some(OsString::from("/home/u"))).unwrap();
        assert_eq!(got, PathBuf::from("/home/u/.locode"));
    }

    #[test]
    fn explicit_override_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(dir.path()).unwrap();
        let got = resolve_home_from(Some(dir.path().as_os_str().to_owned()), None).unwrap();
        assert_eq!(got, canon, "existing override canonicalizes");

        let err = resolve_home_from(Some(OsString::from("/definitely/not/here")), None)
            .expect_err("missing override errors");
        assert!(err.contains("LOCODE_HOME"), "{err}");
    }

    #[test]
    fn nothing_set_is_an_error() {
        assert!(resolve_home_from(None, None).is_err());
        assert!(resolve_home_from(None, Some(OsString::new())).is_err());
    }
}
