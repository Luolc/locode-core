//! Session-directory naming: the **bijective** cwd → dirname encoding (ADR-0024 §2.1).
//!
//! The trace store groups sessions **per cwd** — `~/.locode/sessions/<encoded-cwd>/…`
//! (Claude Code's structure, making `--continue` an O(1) one-directory listing).
//! Claude's own encoding (`[^a-zA-Z0-9]` → `-`) is lossy — `foo-bar`/`foo_bar`/
//! `foo.bar`/`foo/bar` all collide, and every non-ASCII char collapses to `-` — so
//! ours is a bijection instead:
//!
//! - `/` → `+` (the readable separator swap);
//! - literal `+` → `%2B` and literal `%` → `%25` (the self-escape that upgrades
//!   "less likely to collide" into "cannot collide");
//! - every other character (including non-ASCII) is preserved verbatim.
//!
//! Decoding inverts it exactly, so a resume picker can show real cwds with zero file
//! reads. Over the 255-byte filename limit a stable `<fnv1a64-hex16>-<tail>` fallback
//! is emitted — never decodable (starts with a hex digit, never `+`); the store writes
//! a `.cwd` sidecar inside the directory to recover the original path (grok's scheme).
//!
//! Callers pass a **canonicalized** cwd; this module never touches the filesystem.

use std::path::{Path, PathBuf};

/// The filesystem's single-component limit (bytes) — beyond it the hash fallback kicks in.
const MAX_DIRNAME_BYTES: usize = 255;
/// Bytes of the encoded tail kept in the fallback name (the leaf dirs are the
/// distinctive part).
const FALLBACK_TAIL_BYTES: usize = 64;

/// Encode a (canonicalized, absolute) `cwd` into its session-directory name.
///
/// Bijective for every path whose encoding fits the filename limit; longer paths get
/// the stable `<hash16>-<tail>` fallback (decode returns `None`; the store's `.cwd`
/// sidecar recovers the path).
#[must_use]
pub fn encode_cwd_dirname(cwd: &Path) -> String {
    let raw = cwd.to_string_lossy();
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '/' => out.push('+'),
            '+' => out.push_str("%2B"),
            '%' => out.push_str("%25"),
            other => out.push(other),
        }
    }
    if out.len() > MAX_DIRNAME_BYTES {
        return fallback_dirname(&raw, &out);
    }
    out
}

/// Decode a session-directory name back to the cwd it encodes, or `None` for a
/// hash-fallback name (recover via the `.cwd` sidecar) or a malformed escape.
///
/// Only names starting with `+` decode — an encoded absolute path always starts with
/// `+` (the leading `/`), and a fallback name starts with a hex digit, so the first
/// byte disambiguates the two forms.
#[must_use]
pub fn decode_cwd_dirname(name: &str) -> Option<PathBuf> {
    if !name.starts_with('+') {
        return None;
    }
    let mut out = String::with_capacity(name.len());
    let mut chars = name.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '+' => out.push('/'),
            '%' => {
                let hi = chars.next()?.to_digit(16)?;
                let lo = chars.next()?.to_digit(16)?;
                #[allow(clippy::cast_possible_truncation)] // two hex digits ≤ 0xFF
                let byte = (hi * 16 + lo) as u8;
                // The encoder only escapes ASCII '+' and '%'; anything else is
                // malformed.
                if !byte.is_ascii() {
                    return None;
                }
                out.push(char::from(byte));
            }
            other => out.push(other),
        }
    }
    Some(PathBuf::from(out))
}

/// The over-limit fallback: `<fnv1a64 of the ORIGINAL path, 16 hex>-<encoded tail>`,
/// truncated to fit. Deterministic and collision-resistant via the full-path hash;
/// starts with a hex digit so it is never mistaken for a decodable `+…` name.
fn fallback_dirname(raw: &str, encoded: &str) -> String {
    let hash = fnv1a64(raw.as_bytes());
    // Keep the tail of the encoded form — the deepest (most distinctive) directories.
    let mut start = encoded.len().saturating_sub(FALLBACK_TAIL_BYTES);
    while start < encoded.len() && !encoded.is_char_boundary(start) {
        start += 1;
    }
    format!("{hash:016x}-{}", &encoded[start..])
}

/// FNV-1a 64-bit — stable and dependency-free (`DefaultHasher` is not stable across
/// Rust releases, and a hashing dependency is an ask-first item).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_typical_path() {
        let cwd = Path::new("/Users/me/dev/locode-core");
        let name = encode_cwd_dirname(cwd);
        assert_eq!(name, "+Users+me+dev+locode-core");
        assert_eq!(decode_cwd_dirname(&name), Some(cwd.to_path_buf()));
    }

    #[test]
    fn claudes_collision_quartet_stays_distinct() {
        // Under Claude's `[^a-zA-Z0-9]` → `-` these four all collide; here they must
        // encode to four distinct names, each round-tripping.
        let paths = ["/d/foo-bar", "/d/foo_bar", "/d/foo.bar", "/d/foo/bar"];
        let names: Vec<String> = paths
            .iter()
            .map(|p| encode_cwd_dirname(Path::new(p)))
            .collect();
        for (i, a) in names.iter().enumerate() {
            for b in &names[i + 1..] {
                assert_ne!(a, b);
            }
        }
        for (path, name) in paths.iter().zip(&names) {
            assert_eq!(
                decode_cwd_dirname(name),
                Some(PathBuf::from(path)),
                "{name}"
            );
        }
    }

    #[test]
    fn self_escapes_literal_plus_and_percent() {
        let cwd = Path::new("/a+b/c%d");
        let name = encode_cwd_dirname(cwd);
        assert_eq!(name, "+a%2Bb+c%25d");
        assert_eq!(decode_cwd_dirname(&name), Some(cwd.to_path_buf()));
        // The dedicated ambiguity case: a dir literally named `a+b` vs nested `a/b`.
        assert_ne!(
            encode_cwd_dirname(Path::new("/a+b")),
            encode_cwd_dirname(Path::new("/a/b"))
        );
    }

    #[test]
    fn non_ascii_components_are_preserved() {
        let cwd = Path::new("/Users/me/dev/项目");
        let name = encode_cwd_dirname(cwd);
        assert_eq!(name, "+Users+me+dev+项目");
        assert_eq!(decode_cwd_dirname(&name), Some(cwd.to_path_buf()));
        // Same-length CJK names — Claude's scheme collides these; ours must not.
        assert_ne!(
            encode_cwd_dirname(Path::new("/dev/项目")),
            encode_cwd_dirname(Path::new("/dev/工作"))
        );
    }

    #[test]
    fn over_limit_falls_back_to_stable_hash_name() {
        let deep = format!("/{}", "very-long-directory-name/".repeat(20));
        let cwd = Path::new(&deep);
        let name = encode_cwd_dirname(cwd);
        assert!(name.len() <= MAX_DIRNAME_BYTES, "fits the component limit");
        assert!(
            name.as_bytes()[0].is_ascii_hexdigit() && !name.starts_with('+'),
            "fallback never looks decodable: {name}"
        );
        // Deterministic, and undecodable (the `.cwd` sidecar recovers it).
        assert_eq!(name, encode_cwd_dirname(cwd));
        assert_eq!(decode_cwd_dirname(&name), None);
        // A different long path gets a different name (full-path hash).
        let other = format!("/{}x", "very-long-directory-name/".repeat(20));
        assert_ne!(name, encode_cwd_dirname(Path::new(&other)));
    }

    #[test]
    fn malformed_names_do_not_decode() {
        assert_eq!(decode_cwd_dirname("no-leading-plus"), None);
        assert_eq!(decode_cwd_dirname("+bad%zz"), None, "non-hex escape");
        assert_eq!(decode_cwd_dirname("+truncated%2"), None, "short escape");
        assert_eq!(
            decode_cwd_dirname("+high%FF"),
            None,
            "non-ASCII escape byte"
        );
    }
}
