//! Shared project-instruction (`AGENTS.md`) discovery (ADR-0023, Task 30).
//!
//! One shared loader — not pack-selectable — walks `cwd → project root` collecting
//! `AGENTS.md` files (deepest wins on conflict), plus a global `~/.locode/AGENTS.md`,
//! and returns a neutral [`ProjectInstructions`]. The engine renders that into a single
//! `User`-role `<system-reminder>` message and injects it (ADR-0023 §2).
//!
//! This lives in `locode-host` — the trusted OS seam (ADR-0008) — and **reads the
//! discovered files directly**, deliberately bypassing the tool path-jail: discovery
//! legitimately spans ancestors *above* the cwd-rooted jail, and the jail governs
//! *tools*, not engine machinery (ADR-0023 implementation note, 2026-07-23). Reads are
//! bounded to the `AGENTS.md`/`AGENTS.override.md` names along the bounded ancestor walk.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The canonical project-instruction filename.
const AGENTS_FILE: &str = "AGENTS.md";
/// A per-directory, higher-precedence local override (Codex's `AGENTS.override.md`):
/// same-directory, first-match-wins **replacement** of that directory's `AGENTS.md`
/// (conventionally gitignored). Not additive, and never affects other directories.
const OVERRIDE_FILE: &str = "AGENTS.override.md";

/// Discovered project instructions: ordered entries, lowest-precedence first, so a later
/// (deeper) entry wins on conflict. Neutral — the engine owns rendering/injection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectInstructions {
    /// One entry per discovered file, in precedence order (last wins).
    pub entries: Vec<InstructionEntry>,
}

impl ProjectInstructions {
    /// Whether nothing was discovered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One discovered instruction file: where it came from, and its contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionEntry {
    /// The file the content came from (labeled in the injected message so the model can
    /// attribute conflicting rules).
    pub source_path: PathBuf,
    /// The file's text.
    pub content: String,
}

/// How project-instruction discovery behaves. All fields have defaults; a caller (the CLI
/// today, a settings layer later) overrides them.
#[derive(Debug, Clone)]
pub struct InstructionsConfig {
    /// Master switch (default `true`). `--no-project-instructions` sets it `false`.
    pub enabled: bool,
    /// Byte budget for the *assembled* injected body (applied at render time, ADR-0023).
    /// Default 64 KiB; `0` means unbounded.
    pub byte_budget: usize,
    /// Directory markers whose presence marks a project root (default `[".git"]`).
    pub root_markers: Vec<String>,
    /// **Dormant seam** (ADR-0023 §2 / implementation note): a regex on a directory path
    /// that, when it matches, stops the ascent. Not matched in v1 — real activation waits
    /// for `settings.json` (would add the `regex` dependency). `TODO(settings)`.
    pub root_stop_pattern: Option<String>,
    /// **Seam** (ADR-0023 implementation note): extra roots to also discover from. Honored
    /// by the walk, but no `--add-dir` CLI flag ships until the tool-jail-widening task.
    pub extra_roots: Vec<PathBuf>,
    /// Read the global `~/.locode/AGENTS.md` (lowest precedence). Default `true`.
    pub global_file: bool,
}

impl Default for InstructionsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            byte_budget: 64 * 1024,
            root_markers: vec![".git".to_string()],
            root_stop_pattern: None,
            extra_roots: Vec::new(),
            global_file: true,
        }
    }
}

/// Discover project instructions for `cwd` under `cfg`.
///
/// Order (lowest precedence first, so the deepest project file wins on conflict):
/// global `~/.locode/AGENTS.md`, then the primary chain **root→cwd**, then each
/// `extra_roots` entry's own root→dir chain. Per directory, `AGENTS.override.md` replaces
/// `AGENTS.md` (first-match-wins by existence). Empty/whitespace-only and gitignored files
/// (primary chain only) are dropped; entries are deduped by canonical path.
///
/// Synchronous by design: the walk is bounded (at most root-deep) and the files are tiny,
/// so this runs inline once per turn (ADR-0023 "per-turn rescan").
#[must_use]
pub fn load_project_instructions(cwd: &Path, cfg: &InstructionsConfig) -> ProjectInstructions {
    // Resolve the global file from `HOME` here (the only env read); the assembly in
    // `load_impl` takes it explicitly so tests can inject it without mutating process env.
    let global = (cfg.enabled && cfg.global_file)
        .then(global_instruction_path)
        .flatten();
    load_impl(cwd, cfg, global.as_deref())
}

/// The env-free core of [`load_project_instructions`]: `global` is the already-resolved
/// global file path (or `None`).
fn load_impl(cwd: &Path, cfg: &InstructionsConfig, global: Option<&Path>) -> ProjectInstructions {
    if !cfg.enabled {
        return ProjectInstructions::default();
    }

    let mut entries: Vec<InstructionEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // Global file — lowest precedence, first. Outside any repo, so not gitignore-filtered.
    if let Some(global) = global {
        push_dir_entry(&global_dir_of(global), &mut entries, &mut seen, None);
    }

    // Primary chain: root→cwd, deepest last (wins). Gitignore matcher rooted at the
    // discovered project root (only when it is a real git root).
    let root = find_root(cwd, &cfg.root_markers);
    let ignore = build_gitignore(&root);
    for dir in dirs_root_to_leaf(cwd, &root) {
        push_dir_entry(&dir, &mut entries, &mut seen, ignore.as_ref());
    }

    // Extra roots (seam): each discovered the same way, appended after the primary chain.
    for extra in &cfg.extra_roots {
        let extra_root = find_root(extra, &cfg.root_markers);
        for dir in dirs_root_to_leaf(extra, &extra_root) {
            push_dir_entry(&dir, &mut entries, &mut seen, None);
        }
    }

    ProjectInstructions { entries }
}

/// The directory that holds the global file (so [`push_dir_entry`] can pick the override
/// there too, mirroring project directories).
fn global_dir_of(global_file: &Path) -> PathBuf {
    global_file
        .parent()
        .map_or_else(|| global_file.to_path_buf(), Path::to_path_buf)
}

/// `~/.locode/AGENTS.md`, or `None` when `HOME` is unset/empty (dependency-free — the
/// shipped targets are macOS/Linux).
fn global_instruction_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".locode").join(AGENTS_FILE))
}

/// Ascend from `start`; the nearest ancestor containing any `markers` entry is the root.
/// No marker up to the filesystem root ⇒ cwd-only (returns `start`). The filesystem root
/// is only a backstop, never itself the project root.
///
/// `root_stop_pattern` (a regex on the path) is a dormant seam and is **not** consulted
/// here — see [`InstructionsConfig::root_stop_pattern`]. `TODO(settings)`.
fn find_root(start: &Path, markers: &[String]) -> PathBuf {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if markers.iter().any(|m| d.join(m).exists()) {
            return d.to_path_buf();
        }
        dir = d.parent();
    }
    start.to_path_buf()
}

/// The directories from `root` down to `leaf`, inclusive, in root→leaf order (deepest
/// last). `root` is always `leaf` or an ancestor of it, so the ascent always reaches it.
fn dirs_root_to_leaf(leaf: &Path, root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut cur = Some(leaf);
    while let Some(d) = cur {
        dirs.push(d.to_path_buf());
        if d == root {
            break;
        }
        cur = d.parent();
    }
    dirs.reverse();
    dirs
}

/// Pick a directory's instruction file (`AGENTS.override.md` before `AGENTS.md`,
/// first-match-wins by existence), read it directly, and push a deduped, non-empty,
/// non-gitignored entry.
fn push_dir_entry(
    dir: &Path,
    entries: &mut Vec<InstructionEntry>,
    seen: &mut HashSet<String>,
    ignore: Option<&ignore::gitignore::Gitignore>,
) {
    for name in [OVERRIDE_FILE, AGENTS_FILE] {
        let path = dir.join(name);
        if !path.is_file() {
            continue;
        }
        // The first existing candidate wins the directory (even if empty — an empty
        // override still replaces the sibling AGENTS.md; it just yields no entry).
        if let Some(gi) = ignore
            && is_gitignored(gi, &path)
        {
            return;
        }
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        if content.trim().is_empty() {
            return;
        }
        let key = canonical_key(&path);
        if seen.insert(key) {
            entries.push(InstructionEntry {
                source_path: path,
                content,
            });
        }
        return;
    }
}

/// A gitignore matcher for `root`, or `None` when `root` is not a git root (allow-all,
/// grok's rule — `walk.rs`). Built from `root/.gitignore` + `root/.git/info/exclude`.
fn build_gitignore(root: &Path) -> Option<ignore::gitignore::Gitignore> {
    if !root.join(".git").exists() {
        return None;
    }
    let base = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut builder = ignore::gitignore::GitignoreBuilder::new(&base);
    let _ = builder.add(base.join(".gitignore"));
    let _ = builder.add(base.join(".git").join("info").join("exclude"));
    builder.build().ok()
}

/// Whether `path` is gitignored under `gi` (canonicalized so the matcher's root prefix
/// lines up).
fn is_gitignored(gi: &ignore::gitignore::Gitignore, path: &Path) -> bool {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    gi.matched_path_or_any_parents(&canon, /*is_dir*/ false)
        .is_ignore()
}

/// A canonical, case-insensitive dedup key (symlink-resolved when the path exists).
fn canonical_key(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// A canonicalized tempdir (prod always passes a canonical cwd — macOS `/var` →
    /// `/private/var`, so canonicalize for parity).
    fn tmp() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    /// No `HOME` (or global disabled) unless a test opts in, so the real dev home is never
    /// read. Returns a config with `global_file` off by default.
    fn cfg_no_global() -> InstructionsConfig {
        InstructionsConfig {
            global_file: false,
            ..Default::default()
        }
    }

    #[test]
    fn walks_root_to_cwd_deepest_last() {
        let (_g, root) = tmp();
        fs::create_dir(root.join(".git")).unwrap();
        write(&root.join(AGENTS_FILE), "root rules");
        let sub = root.join("a/b");
        write(&sub.join(AGENTS_FILE), "leaf rules");

        let got = load_project_instructions(&sub, &cfg_no_global());
        let paths: Vec<_> = got.entries.iter().map(|e| e.source_path.clone()).collect();
        assert_eq!(
            paths,
            vec![root.join(AGENTS_FILE), sub.join(AGENTS_FILE)],
            "root→cwd order, deepest last"
        );
        assert_eq!(got.entries[0].content, "root rules");
        assert_eq!(got.entries[1].content, "leaf rules");
    }

    #[test]
    fn no_git_falls_back_to_cwd_only() {
        let (_g, root) = tmp();
        // No `.git` anywhere.
        write(&root.join(AGENTS_FILE), "parent rules");
        let sub = root.join("child");
        write(&sub.join(AGENTS_FILE), "cwd rules");

        let got = load_project_instructions(&sub, &cfg_no_global());
        assert_eq!(got.entries.len(), 1, "cwd-only: parent not walked");
        assert_eq!(got.entries[0].source_path, sub.join(AGENTS_FILE));
    }

    #[test]
    fn override_replaces_same_dir_agents_md_only() {
        let (_g, root) = tmp();
        fs::create_dir(root.join(".git")).unwrap();
        write(&root.join(AGENTS_FILE), "root plain");
        let sub = root.join("s");
        write(&sub.join(AGENTS_FILE), "sub plain");
        write(&sub.join(OVERRIDE_FILE), "sub override");

        let got = load_project_instructions(&sub, &cfg_no_global());
        // Root's plain AGENTS.md still loads; sub's override replaces sub's AGENTS.md.
        assert_eq!(got.entries.len(), 2);
        assert_eq!(got.entries[1].source_path, sub.join(OVERRIDE_FILE));
        assert_eq!(got.entries[1].content, "sub override");
        assert!(
            got.entries
                .iter()
                .all(|e| e.source_path != sub.join(AGENTS_FILE)),
            "sub's plain AGENTS.md is suppressed"
        );
    }

    #[test]
    fn empty_override_suppresses_agents_md_and_yields_nothing() {
        let (_g, root) = tmp();
        write(&root.join(AGENTS_FILE), "would-be content");
        write(&root.join(OVERRIDE_FILE), "   \n  ");

        let got = load_project_instructions(&root, &cfg_no_global());
        assert!(
            got.is_empty(),
            "empty override wins the dir, yields no entry"
        );
    }

    #[test]
    fn empty_and_whitespace_files_dropped() {
        let (_g, root) = tmp();
        write(&root.join(AGENTS_FILE), "\n\t  \n");
        let got = load_project_instructions(&root, &cfg_no_global());
        assert!(got.is_empty());
    }

    #[test]
    fn gitignored_agents_md_skipped() {
        let (_g, root) = tmp();
        fs::create_dir(root.join(".git")).unwrap();
        write(&root.join(".gitignore"), "AGENTS.md\n");
        write(&root.join(AGENTS_FILE), "secret");
        let got = load_project_instructions(&root, &cfg_no_global());
        assert!(got.is_empty(), "gitignored AGENTS.md is filtered");
    }

    #[test]
    fn dedup_by_canonical_path() {
        let (_g, root) = tmp();
        fs::create_dir(root.join(".git")).unwrap();
        write(&root.join(AGENTS_FILE), "rules");
        // A symlinked view of the same dir would double-count without canonical dedup;
        // here we assert a single tree yields no duplicates and the key is stable.
        let got = load_project_instructions(&root, &cfg_no_global());
        assert_eq!(got.entries.len(), 1);
        // Same key for the same file.
        assert_eq!(
            canonical_key(&root.join(AGENTS_FILE)),
            canonical_key(&root.join(AGENTS_FILE))
        );
    }

    #[test]
    fn extra_roots_appended_after_primary() {
        let (_g, root) = tmp();
        fs::create_dir(root.join(".git")).unwrap();
        write(&root.join(AGENTS_FILE), "primary");
        let (_g2, other) = tmp();
        write(&other.join(AGENTS_FILE), "extra");

        let cfg = InstructionsConfig {
            extra_roots: vec![other.clone()],
            ..cfg_no_global()
        };
        let got = load_project_instructions(&root, &cfg);
        let paths: Vec<_> = got.entries.iter().map(|e| e.source_path.clone()).collect();
        assert_eq!(paths, vec![root.join(AGENTS_FILE), other.join(AGENTS_FILE)]);
    }

    #[test]
    fn disabled_returns_empty() {
        let (_g, root) = tmp();
        write(&root.join(AGENTS_FILE), "rules");
        let cfg = InstructionsConfig {
            enabled: false,
            ..cfg_no_global()
        };
        assert!(load_project_instructions(&root, &cfg).is_empty());
    }

    #[test]
    fn root_stop_pattern_is_a_dormant_noop() {
        // Providing a pattern must not change behavior in v1 (guards the dormant seam).
        let (_g, root) = tmp();
        fs::create_dir(root.join(".git")).unwrap();
        write(&root.join(AGENTS_FILE), "rules");
        let sub = root.join("x/y");
        write(&sub.join(AGENTS_FILE), "leaf");

        let mut cfg = cfg_no_global();
        cfg.root_stop_pattern = Some(".*/x$".to_string()); // would "stop at x" once wired
        let got = load_project_instructions(&sub, &cfg);
        // Still the full .git-root chain (pattern ignored): root + leaf.
        assert_eq!(got.entries.len(), 2);
        assert_eq!(got.entries[0].source_path, root.join(AGENTS_FILE));
    }

    #[test]
    fn global_file_is_lowest_precedence() {
        // Inject the global path directly (no env mutation — the crate forbids `unsafe`,
        // and `HOME` is process-global). Exercises the same assembly the public fn uses.
        let (_home_g, home) = tmp();
        let global = home.join(".locode").join(AGENTS_FILE);
        write(&global, "global");
        let (_g, root) = tmp();
        fs::create_dir(root.join(".git")).unwrap();
        write(&root.join(AGENTS_FILE), "project");

        let cfg = InstructionsConfig {
            global_file: true,
            ..Default::default()
        };
        let got = load_impl(&root, &cfg, Some(&global));
        let paths: Vec<_> = got.entries.iter().map(|e| e.source_path.clone()).collect();
        assert_eq!(
            paths,
            vec![global.clone(), root.join(AGENTS_FILE)],
            "global first (lowest precedence), project after"
        );
    }

    #[test]
    fn global_file_disabled_passes_none() {
        // With `global_file=false` the public fn resolves `global = None`, so the real
        // `HOME` is never read and only the project chain loads.
        let (_g, root) = tmp();
        write(&root.join(AGENTS_FILE), "project");
        let got = load_project_instructions(&root, &cfg_no_global());
        assert_eq!(got.entries.len(), 1);
        assert_eq!(got.entries[0].source_path, root.join(AGENTS_FILE));
    }

    #[test]
    fn global_instruction_path_shape() {
        // Read-only: whatever HOME is, the global path is `<home>/.locode/AGENTS.md`.
        if let Some(p) = global_instruction_path() {
            assert!(p.ends_with("AGENTS.md"));
            assert!(p.components().any(|c| c.as_os_str() == ".locode"));
        }
    }
}
