//! Skill discovery: the four roots, the five-key contract, precedence, and collisions
//! (ADR-0025 §2).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locode_host::{SkillsExtraEntry, find_root_from_markers, locode_home};

use crate::frontmatter;

/// The canonical per-skill file.
pub const SKILL_FILE: &str = "SKILL.md";
/// The **project** skills directory: `<repo>/.agents/skills`.
///
/// Deliberately not `<repo>/.locode/skills`, unlike every other project-scoped thing
/// we read (ADR-0025 §2 amendment 2026-07-24). `.agents/` is the cross-agent
/// convention: codex scans `<root>/.agents/skills` via its own `AGENTS_DIR_NAME`
/// constant, and grok hard-codes `.agents` alongside `.grok` as an always-scanned
/// config root (`.claude` is merely opt-in compat). A skill is a portable artifact —
/// the format is the one thing all four harnesses converged on — so a repo's skills
/// belong where any agent can find them. Settings stay under `.locode/`, because those
/// are ours alone.
pub const PROJECT_SKILLS_DIR: [&str; 2] = [".agents", "skills"];
/// Slug cap, grok's `MAX_NAME_LEN` (`discovery.rs:16`).
const MAX_NAME_LEN: usize = 64;

/// Where a skill came from — and, on a collision, how it is addressed.
///
/// Three scopes, not four: an `extends` dotfolder's skills are **`User`**, because
/// `extends` is how a user composes their own configuration rather than a separate
/// authority (ADR-0025 §2, user decision). It still ranks *below* `~/.locode/skills`
/// in precedence — same qualifier, different tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillScope {
    /// `<repo>/.agents/skills/` — the cross-agent location (see [`PROJECT_SKILLS_DIR`]).
    Project,
    /// `~/.locode/skills/`, and every `extends` dotfolder's `skills/`.
    User,
    /// A `skills.extra` entry.
    Extra,
}

impl SkillScope {
    /// The qualifier used in `<scope>:<name>`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SkillScope::Project => "project",
            SkillScope::User => "user",
            SkillScope::Extra => "extra",
        }
    }
}

/// One discovered skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// Slug-normalized identity (ADR-0025 §2).
    pub name: String,
    /// Which tier it came from, and its collision qualifier.
    pub scope: SkillScope,
    /// The router text shown in the listing.
    pub description: String,
    /// Optional trigger phrasing, rendered as a separate `Use when:` line.
    pub when_to_use: Option<String>,
    /// Absolute path to the `SKILL.md` — this *is* the invocation mechanism
    /// (ADR-0025 §4: the model reads it).
    pub path: PathBuf,
    /// `true` ⇒ never listed, so the model never learns it exists.
    pub disable_model_invocation: bool,
    /// Parsed and carried; **no observable effect until slash invocation exists**.
    pub user_invocable: bool,
}

impl Skill {
    /// `name`, or `scope:name` when another scope also has this name.
    #[must_use]
    pub fn display_name(&self, ambiguous: bool) -> String {
        if ambiguous {
            format!("{}:{}", self.scope.as_str(), self.name)
        } else {
            self.name.clone()
        }
    }
}

/// Inputs for [`discover`] — all of it derived from resolved settings, which is what
/// makes the load order an invariant (ADR-0025 §6.1).
#[derive(Debug, Clone, Default)]
pub struct SkillsConfig {
    /// Master switch. `false` ⇒ discovery returns nothing and touches no disk.
    pub enabled: bool,
    /// Root markers for locating `<repo>` (default `[".git"]`).
    pub root_markers: Vec<String>,
    /// Resolved `extends` dotfolders (`SettingsLoad::extends_dirs`).
    pub extends_dirs: Vec<PathBuf>,
    /// `skills.extra` entries from settings.
    pub extra: Vec<SkillsExtraEntry>,
}

impl SkillsConfig {
    /// Enabled, with the default `.git` marker.
    #[must_use]
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            root_markers: vec![".git".to_string()],
            extends_dirs: Vec::new(),
            extra: Vec::new(),
        }
    }
}

/// What discovery produced.
#[derive(Debug, Clone, Default)]
pub struct DiscoveredSkills {
    /// Listable skills, highest precedence first, ambiguity already resolved.
    pub skills: Vec<Skill>,
    /// Skipped files and unusable names — for stderr, never for the model.
    pub warnings: Vec<String>,
}

/// Discover every skill visible from `cwd`.
///
/// Precedence, highest first: `<repo>/.agents/skills` → `~/.locode/skills` →
/// `extends` dotfolders (list order) → `extra` entries (list order). Within one
/// **qualifier**, a higher tier shadows a lower one and the
/// loser is dropped; across qualifiers both survive and are rendered qualified.
#[must_use]
pub fn discover(cwd: &Path, cfg: &SkillsConfig) -> DiscoveredSkills {
    // Resolve the user root from the environment here — the only env read — so the
    // assembly below can be driven with injected paths. Same split as
    // `locode_instructions::load_project_instructions`, and for the same reason: a test
    // must never see the developer's real `~/.locode/skills`.
    let user_root = cfg
        .enabled
        .then(|| locode_home().ok().map(|home| home.join("skills")))
        .flatten();
    discover_impl(cwd, cfg, user_root.as_deref())
}

fn discover_impl(cwd: &Path, cfg: &SkillsConfig, user_root: Option<&Path>) -> DiscoveredSkills {
    if !cfg.enabled {
        return DiscoveredSkills::default();
    }
    let mut warnings = Vec::new();
    let mut found: Vec<Skill> = Vec::new();

    let root = find_root_from_markers(cwd, &cfg.root_markers, None);
    collect_root(
        &root.join(PROJECT_SKILLS_DIR[0]).join(PROJECT_SKILLS_DIR[1]),
        SkillScope::Project,
        &mut found,
        &mut warnings,
    );
    if let Some(user_root) = user_root {
        collect_root(user_root, SkillScope::User, &mut found, &mut warnings);
    }
    for dir in &cfg.extends_dirs {
        collect_root(
            &dir.join("skills"),
            SkillScope::User,
            &mut found,
            &mut warnings,
        );
    }
    for entry in &cfg.extra {
        match entry {
            SkillsExtraEntry::Skill(dir) => {
                collect_skill_dir(dir, SkillScope::Extra, &mut found, &mut warnings);
            }
            SkillsExtraEntry::Folder(dir) => {
                collect_root(dir, SkillScope::Extra, &mut found, &mut warnings);
            }
        }
    }

    // Same qualifier + same name ⇒ the first (highest-precedence) one wins.
    let mut seen: Vec<(SkillScope, String)> = Vec::new();
    found.retain(|s| {
        let key = (s.scope, s.name.clone());
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    // A skill the user marked model-invisible never reaches the listing (§2).
    found.retain(|s| !s.disable_model_invocation);

    DiscoveredSkills {
        skills: found,
        warnings,
    }
}

/// Names carried by more than one scope — these render qualified.
#[must_use]
pub fn ambiguous_names(skills: &[Skill]) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for s in skills {
        *counts.entry(s.name.as_str()).or_default() += 1;
    }
    let mut out: Vec<String> = counts
        .into_iter()
        .filter(|&(_, n)| n > 1)
        .map(|(n, _)| n.to_string())
        .collect();
    out.sort();
    out
}

/// Scan one `skills/` root: every child directory holding a `SKILL.md`.
fn collect_root(root: &Path, scope: SkillScope, out: &mut Vec<Skill>, warnings: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(root) else {
        return; // absent roots are normal
    };
    let mut dirs: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort(); // stable listing order regardless of filesystem order
    for dir in dirs {
        collect_skill_dir(&dir, scope, out, warnings);
    }
}

/// Load one skill directory, if it holds a readable `SKILL.md`.
fn collect_skill_dir(
    dir: &Path,
    scope: SkillScope,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<String>,
) {
    let path = dir.join(SKILL_FILE);
    if !path.is_file() {
        return;
    }
    let Ok(source) = std::fs::read_to_string(&path) else {
        warnings.push(format!("skills: cannot read {}; skipped", path.display()));
        return;
    };
    let Some((fm, _body)) = frontmatter::parse(&source) else {
        warnings.push(format!(
            "skills: {} has no `---` frontmatter block; skipped",
            path.display()
        ));
        return;
    };

    let raw_name = fm.get("name").map_or("", String::as_str);
    let name = if raw_name.trim().is_empty() {
        dir.file_name().map(|n| n.to_string_lossy().to_string())
    } else {
        Some(raw_name.to_string())
    }
    .map(|n| normalize_name(&n))
    .filter(|n| !n.is_empty());
    let Some(name) = name else {
        warnings.push(format!(
            "skills: {} has no usable name (frontmatter `name` and directory name both \
             normalize to empty); skipped",
            path.display()
        ));
        return;
    };

    let description = fm
        .get("description")
        .map(|d| d.trim().to_string())
        .unwrap_or_default();
    if description.is_empty() {
        warnings.push(format!(
            "skills: {} has no `description`; skipped (the description is what the \
             model matches a task against)",
            path.display()
        ));
        return;
    }

    out.push(Skill {
        name,
        scope,
        description,
        when_to_use: fm
            .get("when-to-use")
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty()),
        path,
        disable_model_invocation: fm
            .get("disable-model-invocation")
            .is_some_and(|v| frontmatter::as_bool(v)),
        user_invocable: fm
            .get("user-invocable")
            .is_none_or(|v| frontmatter::as_bool(v)),
    });
}

/// Slug: lowercase, every non-`[a-z0-9]` → `-`, collapse runs, trim, cap at 64.
///
/// grok's `normalize_skill_name` verbatim (`discovery.rs:333-348`) — it keeps a
/// human-written `name: Review PR` usable as `review-pr` instead of dropping the skill.
#[must_use]
pub fn normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars() {
        let c = c.to_ascii_lowercase();
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            c
        } else {
            '-'
        };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    let out = out.trim_matches('-');
    out.chars().take(MAX_NAME_LEN).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> (TempDir, PathBuf) {
        let d = TempDir::new().unwrap();
        let p = std::fs::canonicalize(d.path()).unwrap();
        (d, p)
    }

    /// Write `<root>/<name>/SKILL.md` with the given frontmatter body.
    fn skill(root: &Path, name: &str, frontmatter: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(SKILL_FILE),
            format!("---\n{frontmatter}\n---\n# {name}\n"),
        )
        .unwrap();
    }

    fn cfg() -> SkillsConfig {
        SkillsConfig::enabled()
    }

    /// Discover with **no** user root, so the dev machine's real `~/.locode/skills`
    /// can never leak into a test.
    fn discover_no_home(cwd: &Path, cfg: &SkillsConfig) -> DiscoveredSkills {
        discover_impl(cwd, cfg, None)
    }

    fn names(d: &DiscoveredSkills) -> Vec<&str> {
        d.skills.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn finds_project_skills_sorted_and_reads_the_five_keys() {
        let (_g, repo) = tmp();
        fs::create_dir(repo.join(".git")).unwrap();
        let root = repo.join(".agents/skills");
        skill(&root, "zebra", "name: zebra\ndescription: Z");
        skill(
            &root,
            "commit",
            "name: commit\ndescription: Make a commit\nwhen-to-use: on push\nuser-invocable: false",
        );

        let got = discover_no_home(&repo, &cfg());
        assert_eq!(names(&got), vec!["commit", "zebra"], "sorted, stable");
        let c = &got.skills[0];
        assert_eq!(c.scope, SkillScope::Project);
        assert_eq!(c.description, "Make a commit");
        assert_eq!(c.when_to_use.as_deref(), Some("on push"));
        assert!(!c.user_invocable, "parsed, even though it is inert in v1");
        assert!(c.path.ends_with("commit/SKILL.md"));
    }

    #[test]
    fn name_falls_back_to_the_directory_and_is_slugified() {
        let (_g, repo) = tmp();
        fs::create_dir(repo.join(".git")).unwrap();
        let root = repo.join(".agents/skills");
        skill(&root, "My_Skill", "description: D"); // no frontmatter name
        skill(&root, "other", "name: Review PR\ndescription: D");

        let got = discover_no_home(&repo, &cfg());
        let mut n = names(&got);
        n.sort_unstable();
        assert_eq!(n, vec!["my-skill", "review-pr"]);
    }

    #[test]
    fn disable_model_invocation_hides_the_skill_entirely() {
        let (_g, repo) = tmp();
        fs::create_dir(repo.join(".git")).unwrap();
        let root = repo.join(".agents/skills");
        skill(&root, "shown", "description: D");
        skill(
            &root,
            "hidden",
            "description: D\ndisable-model-invocation: true",
        );

        let got = discover_no_home(&repo, &cfg());
        assert_eq!(names(&got), vec!["shown"]);
    }

    #[test]
    fn a_broken_skill_is_skipped_with_a_warning_and_does_not_stop_the_others() {
        let (_g, repo) = tmp();
        fs::create_dir(repo.join(".git")).unwrap();
        let root = repo.join(".agents/skills");
        skill(&root, "good", "description: D");
        // No frontmatter at all.
        let bad = root.join("bad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join(SKILL_FILE), "# just markdown\n").unwrap();
        // Frontmatter, but no description — nothing for the model to match on.
        skill(&root, "nodesc", "name: nodesc");

        let got = discover_no_home(&repo, &cfg());
        assert_eq!(names(&got), vec!["good"]);
        let w = got.warnings.join(" | ");
        assert!(w.contains("frontmatter"), "{w}");
        assert!(w.contains("description"), "{w}");
    }

    /// `extends` skills are `user`-scoped but rank *below* `~/.locode/skills`; an
    /// `extra` folder is its own scope and survives a same-name collision.
    #[test]
    fn precedence_within_a_scope_drops_the_loser_across_scopes_keeps_both() {
        let (_g, repo) = tmp();
        fs::create_dir(repo.join(".git")).unwrap();
        skill(
            &repo.join(".agents/skills"),
            "commit",
            "description: project",
        );
        let (_e, team) = tmp();
        skill(&team.join("skills"), "commit", "description: team");
        let (_x, extra) = tmp();
        skill(&extra, "commit", "description: extra");

        let mut c = cfg();
        c.extends_dirs = vec![team];
        c.extra = vec![SkillsExtraEntry::Folder(extra)];
        let got = discover_no_home(&repo, &c);

        let by_scope: Vec<_> = got
            .skills
            .iter()
            .map(|s| (s.scope, &*s.description))
            .collect();
        assert_eq!(
            by_scope,
            vec![
                (SkillScope::Project, "project"),
                (SkillScope::User, "team"),
                (SkillScope::Extra, "extra"),
            ],
            "one per scope survives"
        );
        assert_eq!(ambiguous_names(&got.skills), vec!["commit"]);
        assert_eq!(got.skills[1].display_name(true), "user:commit");
    }

    #[test]
    fn a_single_skill_extra_entry_points_straight_at_the_skill_dir() {
        let (_g, repo) = tmp();
        fs::create_dir(repo.join(".git")).unwrap();
        let (_x, dir) = tmp();
        let one = dir.join("oneoff");
        fs::create_dir_all(&one).unwrap();
        fs::write(one.join(SKILL_FILE), "---\ndescription: D\n---\n").unwrap();

        let mut c = cfg();
        c.extra = vec![SkillsExtraEntry::Skill(one)];
        let got = discover_no_home(&repo, &c);
        assert_eq!(names(&got), vec!["oneoff"]);
        assert_eq!(got.skills[0].scope, SkillScope::Extra);
    }

    #[test]
    fn disabled_config_touches_nothing() {
        let (_g, repo) = tmp();
        fs::create_dir(repo.join(".git")).unwrap();
        skill(&repo.join(".agents/skills"), "commit", "description: D");
        let got = discover_no_home(&repo, &SkillsConfig::default());
        assert!(got.skills.is_empty() && got.warnings.is_empty());
    }

    /// The user root is injected, so a real `~/.locode/skills` cannot affect a run —
    /// and an `extends` dotfolder's skills land in the same `user` scope but below it.
    #[test]
    fn user_root_and_extends_share_the_user_scope_with_the_home_root_winning() {
        let (_g, repo) = tmp();
        fs::create_dir(repo.join(".git")).unwrap();
        let (_h, home) = tmp();
        skill(&home.join("skills"), "commit", "description: home");
        let (_e, team) = tmp();
        skill(&team.join("skills"), "commit", "description: team");

        let mut c = cfg();
        c.extends_dirs = vec![team];
        let got = discover_impl(&repo, &c, Some(&home.join("skills")));
        assert_eq!(
            got.skills
                .iter()
                .map(|s| &*s.description)
                .collect::<Vec<_>>(),
            vec!["home"],
            "same scope ⇒ the home root shadows the extended dotfolder"
        );
    }

    /// Project skills live in the **cross-agent** `.agents/skills`, not `.locode/skills`
    /// — a repo's skills should be findable by any agent, while settings stay ours.
    #[test]
    fn project_skills_come_from_dot_agents_not_dot_locode() {
        let (_g, repo) = tmp();
        fs::create_dir(repo.join(".git")).unwrap();
        skill(&repo.join(".agents/skills"), "shared", "description: D");
        skill(&repo.join(".locode/skills"), "ours", "description: D");

        let got = discover_no_home(&repo, &cfg());
        assert_eq!(
            names(&got),
            vec!["shared"],
            "`.locode/skills` is not a project skills root"
        );
    }

    #[test]
    fn slug_rules() {
        assert_eq!(normalize_name("Review PR"), "review-pr");
        assert_eq!(normalize_name("tool-v1.2"), "tool-v1-2");
        assert_eq!(normalize_name("__weird__"), "weird");
        assert_eq!(normalize_name("---"), "");
        assert_eq!(normalize_name(&"a".repeat(100)).len(), 64);
    }
}
