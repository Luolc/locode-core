//! Layered `settings.json` loading (ADR-0024 §1).
//!
//! Five layers, lowest → highest precedence:
//! 1. `~/.locode/settings.json` (user)
//! 2. the user layer's `extends` files (list order; ADR-0024 §1.2 amendment)
//! 3. `<project-root>/.locode/settings.json` (committed)
//! 4. `<project-root>/.locode/settings.local.json` (gitignored)
//! 5. `--settings <file-or-inline-json>` (flag)
//!
//! Merge semantics (Claude `settings.ts:529-547`): objects deep-merge, scalars
//! overwrite, arrays **concatenate + dedupe** (permission-style lists accumulate).
//! Merging happens on raw `serde_json::Value`s, so unknown keys survive and are
//! simply not interpreted (never rejected). A malformed/missing layer degrades to
//! skipped-with-warning — never a hard error (Claude's filter-not-reject).
//!
//! Security (§1.3): the two **project** layers are attacker-controlled (a cloned
//! repo ships them), so the denylisted keys (`api_schema`) and the `extends`
//! pointer are stripped from them with a warning. `extends` files merge with
//! *user* trust — the user explicitly pointed at them (§1.2 amendment).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::instructions::find_root_from_markers;

/// Keys stripped from the project layers before merging (ADR-0024 §1.3 — a
/// reviewed list: extending it is a normal change, shrinking needs an amendment).
const PROJECT_DENYLIST: &[&str] = &["api_schema"];
/// The user-layer-only pointer key (§1.2 amendment): stripped from project layers.
const EXTENDS_KEY: &str = "extends";

/// The typed view of the merged settings (v1 fields, ADR-0024 §1.4). Unknown keys
/// are tolerated at every layer; absent keys are `None`/empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// Default model (threaded to the provider factory; no flag yet).
    pub model: Option<String>,
    /// Default wire (`--api-schema`/`LOCODE_API_SCHEMA` win). Project-denylisted.
    pub api_schema: Option<String>,
    /// Default harness pack (`--harness` wins).
    pub harness: Option<String>,
    /// `instructions.root_stop_pattern` — activates ADR-0023's root-detection
    /// regex (matching itself lands in Task 31 S2).
    pub root_stop_pattern: Option<String>,
    /// `skills.extra` — validated manual skill entries (consumed by the skills P0).
    pub skills_extra: Vec<SkillsExtraEntry>,
}

/// One validated `skills.extra` entry (ADR-0024 §1.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsExtraEntry {
    /// The path itself contains `SKILL.md` — a single skill.
    Skill(PathBuf),
    /// A folder of skills (its path ends in `skills`); children holding
    /// `SKILL.md` are skills.
    Folder(PathBuf),
}

/// The loader result: the merged settings plus human-readable warnings the
/// caller surfaces on stderr (this crate never prints).
#[derive(Debug, Clone, Default)]
pub struct SettingsLoad {
    /// The merged, typed settings.
    pub settings: Settings,
    /// Skipped layers, stripped keys, invalid entries — in discovery order.
    pub warnings: Vec<String>,
}

/// Load and merge the five layers for `cwd`. `flag` is the raw `--settings`
/// value (a path, or inline JSON when it starts with `{`).
///
/// Env reads happen only here (`~` expansion + the home resolver); the core is
/// [`load_settings_from`] so tests inject everything.
#[must_use]
pub fn load_settings(cwd: &Path, flag: Option<&str>) -> SettingsLoad {
    let mut warnings = Vec::new();
    let user_dir = match crate::home::locode_home() {
        Ok(dir) => Some(dir),
        Err(e) => {
            warnings.push(format!("settings: {e}; user layer skipped"));
            None
        }
    };
    let home_for_tilde = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    let mut load = load_settings_from(user_dir.as_deref(), cwd, home_for_tilde.as_deref(), flag);
    warnings.append(&mut load.warnings);
    SettingsLoad {
        settings: load.settings,
        warnings,
    }
}

/// The env-free core of [`load_settings`]: `user_dir` is the resolved `~/.locode`
/// (or `None`), `home_for_tilde` backs `~` expansion.
#[must_use]
pub fn load_settings_from(
    user_dir: Option<&Path>,
    cwd: &Path,
    home_for_tilde: Option<&Path>,
    flag: Option<&str>,
) -> SettingsLoad {
    let mut warnings: Vec<String> = Vec::new();
    let mut merged = Value::Object(serde_json::Map::new());

    // ---- 1. user layer + 2. its extends files ----
    if let Some(user_dir) = user_dir {
        let user_file = user_dir.join("settings.json");
        if let Some(user_value) = read_layer(&user_file, &mut warnings) {
            let extends = extract_extends(&user_value, &user_file, &mut warnings);
            merge_values(&mut merged, strip_key(user_value, EXTENDS_KEY));
            for entry in extends {
                let path = expand_tilde(&entry, home_for_tilde, user_dir);
                // The user explicitly pointed at this file — absence is loud
                // (unlike the standard layers, whose absence is normal).
                if !path.is_file() {
                    warnings.push(format!(
                        "settings: extends file {} not found; skipped",
                        path.display()
                    ));
                    continue;
                }
                if let Some(mut value) = read_layer(&path, &mut warnings) {
                    // Non-recursive (§1.2 amendment): a nested `extends` is ignored.
                    if value.get(EXTENDS_KEY).is_some() {
                        warnings.push(format!(
                            "settings: nested `extends` in {} ignored (extends does not recurse)",
                            path.display()
                        ));
                        value = strip_key(value, EXTENDS_KEY);
                    }
                    merge_values(&mut merged, value);
                }
            }
        }
    }

    // ---- 3. project + 4. project-local layers (denylisted) ----
    let root = find_root_from_markers(cwd, &[".git".to_string()], None);
    for name in ["settings.json", "settings.local.json"] {
        let path = root.join(".locode").join(name);
        if let Some(mut value) = read_layer(&path, &mut warnings) {
            for key in PROJECT_DENYLIST.iter().copied().chain([EXTENDS_KEY]) {
                if value.get(key).is_some() {
                    warnings.push(format!(
                        "settings: `{key}` in {} ignored (project layers may not set it, ADR-0024 §1.3)",
                        path.display()
                    ));
                    value = strip_key(value, key);
                }
            }
            merge_values(&mut merged, value);
        }
    }

    // ---- 5. flag layer ----
    if let Some(flag) = flag {
        // Inline JSON when it *looks* like JSON (object or array — the array case
        // still fails the object check below, with a clearer message than ENOENT).
        let parsed = if matches!(flag.trim_start().chars().next(), Some('{' | '[')) {
            serde_json::from_str::<Value>(flag)
                .map_err(|e| format!("settings: --settings inline JSON: {e}"))
        } else {
            let path = expand_tilde(flag, home_for_tilde, cwd);
            std::fs::read_to_string(&path)
                .map_err(|e| format!("settings: --settings {}: {e}", path.display()))
                .and_then(|text| {
                    serde_json::from_str::<Value>(&text)
                        .map_err(|e| format!("settings: --settings {}: {e}", path.display()))
                })
        };
        match parsed {
            Ok(value) if value.is_object() => merge_values(&mut merged, value),
            Ok(_) => warnings.push("settings: --settings must be a JSON object".to_string()),
            Err(e) => warnings.push(e),
        }
    }

    // ---- decode the typed view + validate skills.extra ----
    let raw: RawSettings = serde_json::from_value(merged).unwrap_or_else(|e| {
        warnings.push(format!("settings: merged settings did not decode: {e}"));
        RawSettings::default()
    });
    if let Some(pattern) = &raw.instructions.root_stop_pattern
        && let Err(e) = regex::Regex::new(pattern)
    {
        warnings.push(format!(
            "settings: instructions.root_stop_pattern is not a valid regex ({e}); \
             root detection will ignore it"
        ));
    }
    let skills_extra = validate_skills_extra(
        &raw.skills.extra,
        home_for_tilde,
        user_dir.unwrap_or(cwd),
        &mut warnings,
    );
    SettingsLoad {
        settings: Settings {
            model: raw.model,
            api_schema: raw.api_schema,
            harness: raw.harness,
            root_stop_pattern: raw.instructions.root_stop_pattern,
            skills_extra,
        },
        warnings,
    }
}

/// The serde shape of one merged settings document. Plain `Deserialize` — unknown
/// keys are ignored by default, exactly the tolerance ADR-0024 §1.5 requires.
#[derive(Debug, Default, Deserialize)]
struct RawSettings {
    model: Option<String>,
    api_schema: Option<String>,
    harness: Option<String>,
    #[serde(default)]
    instructions: RawInstructions,
    #[serde(default)]
    skills: RawSkills,
}

#[derive(Debug, Default, Deserialize)]
struct RawInstructions {
    root_stop_pattern: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawSkills {
    #[serde(default)]
    extra: Vec<String>,
}

/// Read + parse one layer file. Absent file ⇒ `None` silently; unreadable or
/// non-object JSON ⇒ `None` with a warning naming the file.
fn read_layer(path: &Path, warnings: &mut Vec<String>) -> Option<Value> {
    if !path.is_file() {
        return None;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            warnings.push(format!(
                "settings: {} unreadable ({e}); skipped",
                path.display()
            ));
            return None;
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(value) if value.is_object() => Some(value),
        Ok(_) => {
            warnings.push(format!(
                "settings: {} is not a JSON object; skipped",
                path.display()
            ));
            None
        }
        Err(e) => {
            warnings.push(format!(
                "settings: {} invalid ({e}); skipped",
                path.display()
            ));
            None
        }
    }
}

/// Pull the user layer's `extends` list (strings only; anything else warns).
fn extract_extends(
    user_value: &Value,
    user_file: &Path,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    match user_value.get(EXTENDS_KEY) {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.clone()),
                other => {
                    warnings.push(format!(
                        "settings: non-string `extends` entry {other} in {} ignored",
                        user_file.display()
                    ));
                    None
                }
            })
            .collect(),
        Some(_) => {
            warnings.push(format!(
                "settings: `extends` in {} must be an array of paths; ignored",
                user_file.display()
            ));
            Vec::new()
        }
    }
}

/// Validate `skills.extra` entries (ADR-0024 §1.4): contains `SKILL.md` ⇒ a single
/// skill; else the path must end in `skills` ⇒ a folder; anything else warns + drops.
fn validate_skills_extra(
    entries: &[String],
    home_for_tilde: Option<&Path>,
    base: &Path,
    warnings: &mut Vec<String>,
) -> Vec<SkillsExtraEntry> {
    entries
        .iter()
        .filter_map(|entry| {
            let path = expand_tilde(entry, home_for_tilde, base);
            if path.join("SKILL.md").is_file() {
                return Some(SkillsExtraEntry::Skill(path));
            }
            let trimmed = entry.trim_end_matches('/');
            if trimmed.ends_with("skills") {
                return Some(SkillsExtraEntry::Folder(path));
            }
            warnings.push(format!(
                "settings: skills.extra entry `{entry}` is neither a skill (no SKILL.md) \
                 nor a skills folder (path must end in `skills`); ignored"
            ));
            None
        })
        .collect()
}

/// `~`/`~/…` expansion against `home`, else resolution of relative paths against
/// `base` (the referencing file's directory — ADR-0024 §1.2 amendment).
fn expand_tilde(raw: &str, home: Option<&Path>, base: &Path) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = home
    {
        return home.join(rest);
    }
    if raw == "~"
        && let Some(home) = home
    {
        return home.to_path_buf();
    }
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

/// Remove `key` from an object value (no-op otherwise).
fn strip_key(mut value: Value, key: &str) -> Value {
    if let Value::Object(map) = &mut value {
        map.remove(key);
    }
    value
}

/// ADR-0024 §1.2 merge: objects deep-merge, arrays concat+dedupe, scalars (and
/// type mismatches) overwrite.
fn merge_values(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                match base_map.get_mut(&key) {
                    Some(base_value) => merge_values(base_value, overlay_value),
                    None => {
                        base_map.insert(key, overlay_value);
                    }
                }
            }
        }
        (Value::Array(base_items), Value::Array(overlay_items)) => {
            for item in overlay_items {
                if !base_items.contains(&item) {
                    base_items.push(item);
                }
            }
        }
        (base_slot, overlay_value) => *base_slot = overlay_value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    /// A canonicalized tempdir tree with a `.git` root and a `~/.locode` home.
    struct Fixture {
        _guards: Vec<tempfile::TempDir>,
        home: PathBuf,     // fake $HOME
        user_dir: PathBuf, // fake ~/.locode
        repo: PathBuf,     // project root (.git)
    }

    fn fixture() -> Fixture {
        let home_guard = tempfile::tempdir().unwrap();
        let repo_guard = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home_guard.path()).unwrap();
        let repo = fs::canonicalize(repo_guard.path()).unwrap();
        let user_dir = home.join(".locode");
        fs::create_dir_all(&user_dir).unwrap();
        fs::create_dir(repo.join(".git")).unwrap();
        Fixture {
            _guards: vec![home_guard, repo_guard],
            home,
            user_dir,
            repo,
        }
    }

    fn write(path: &Path, value: &Value) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    fn load(f: &Fixture, flag: Option<&str>) -> SettingsLoad {
        load_settings_from(Some(&f.user_dir), &f.repo, Some(&f.home), flag)
    }

    #[test]
    fn precedence_user_lt_extends_lt_project_lt_local_lt_flag() {
        let f = fixture();
        write(
            &f.user_dir.join("settings.json"),
            &json!({"model": "user", "harness": "user", "api_schema": "user",
                    "extends": ["team.json"]}),
        );
        write(
            &f.user_dir.join("team.json"),
            &json!({"model": "team", "harness": "team"}),
        );
        write(
            &f.repo.join(".locode/settings.json"),
            &json!({"model": "project"}),
        );
        write(
            &f.repo.join(".locode/settings.local.json"),
            &json!({"model": "local"}),
        );

        // No flag: local wins model; team beat user for harness; api_schema
        // survives from user (projects can't set it).
        let got = load(&f, None);
        assert_eq!(got.settings.model.as_deref(), Some("local"));
        assert_eq!(got.settings.harness.as_deref(), Some("team"));
        assert_eq!(got.settings.api_schema.as_deref(), Some("user"));

        // Flag beats everything.
        let got = load(&f, Some(r#"{"model": "flag"}"#));
        assert_eq!(got.settings.model.as_deref(), Some("flag"));
    }

    #[test]
    fn extends_is_ordered_and_non_recursive() {
        let f = fixture();
        write(
            &f.user_dir.join("settings.json"),
            &json!({"extends": ["a.json", "b.json"]}),
        );
        write(&f.user_dir.join("a.json"), &json!({"model": "a"}));
        write(
            &f.user_dir.join("b.json"),
            &json!({"model": "b", "extends": ["c.json"]}),
        );
        write(&f.user_dir.join("c.json"), &json!({"model": "c"}));

        let got = load(&f, None);
        // Later extends entry wins; c.json never loads (no recursion).
        assert_eq!(got.settings.model.as_deref(), Some("b"));
        assert!(
            got.warnings.iter().any(|w| w.contains("nested `extends`")),
            "{:?}",
            got.warnings
        );
    }

    #[test]
    fn project_layers_cannot_set_denylisted_keys_or_extends() {
        let f = fixture();
        write(
            &f.user_dir.join("settings.json"),
            &json!({"api_schema": "user"}),
        );
        write(
            &f.repo.join(".locode/settings.json"),
            &json!({"api_schema": "evil", "extends": ["/tmp/evil.json"], "model": "ok"}),
        );
        let got = load(&f, None);
        assert_eq!(
            got.settings.api_schema.as_deref(),
            Some("user"),
            "denylisted"
        );
        assert_eq!(got.settings.model.as_deref(), Some("ok"), "other keys pass");
        assert_eq!(
            got.warnings
                .iter()
                .filter(|w| w.contains("project layers may not set"))
                .count(),
            2,
            "{:?}",
            got.warnings
        );
    }

    #[test]
    fn arrays_union_and_objects_deep_merge() {
        let f = fixture();
        write(
            &f.user_dir.join("settings.json"),
            &json!({"skills": {"extra": ["~/a-skills"]}, "instructions": {"root_stop_pattern": "u"}}),
        );
        write(
            &f.repo.join(".locode/settings.json"),
            &json!({"skills": {"extra": ["~/b-skills", "~/a-skills"]}}),
        );
        let got = load(&f, None);
        // Deep merge kept instructions from user; arrays unioned without dupes.
        assert_eq!(got.settings.root_stop_pattern.as_deref(), Some("u"));
        let folders: Vec<_> = got
            .settings
            .skills_extra
            .iter()
            .map(|e| match e {
                SkillsExtraEntry::Folder(p) | SkillsExtraEntry::Skill(p) => p.clone(),
            })
            .collect();
        assert_eq!(
            folders,
            vec![f.home.join("a-skills"), f.home.join("b-skills")],
            "union, first occurrence order, no duplicate"
        );
    }

    #[test]
    fn skills_extra_classifies_and_validates() {
        let f = fixture();
        let single = f.home.join("one-off");
        fs::create_dir_all(&single).unwrap();
        fs::write(single.join("SKILL.md"), "x").unwrap();
        write(
            &f.user_dir.join("settings.json"),
            &json!({"skills": {"extra": ["~/one-off", "~/team-skills/", "~/random-dir"]}}),
        );
        let got = load(&f, None);
        assert_eq!(
            got.settings.skills_extra,
            vec![
                SkillsExtraEntry::Skill(single),
                SkillsExtraEntry::Folder(f.home.join("team-skills/")),
            ]
        );
        assert!(
            got.warnings.iter().any(|w| w.contains("random-dir")),
            "{:?}",
            got.warnings
        );
    }

    #[test]
    fn malformed_layers_degrade_with_warnings() {
        let f = fixture();
        fs::write(f.user_dir.join("settings.json"), "{not json").unwrap();
        write(
            &f.repo.join(".locode/settings.json"),
            &json!({"model": "p"}),
        );
        let got = load(&f, None);
        assert_eq!(
            got.settings.model.as_deref(),
            Some("p"),
            "good layers still load"
        );
        assert!(got.warnings.iter().any(|w| w.contains("invalid")));

        // Missing extends file warns loudly (the user pointed at it) but the load
        // survives — the project layer (model "p") still wins as usual.
        write(
            &f.user_dir.join("settings.json"),
            &json!({"extends": ["missing.json"], "model": "u"}),
        );
        let got = load(&f, None);
        assert_eq!(got.settings.model.as_deref(), Some("p"));
        assert!(
            got.warnings
                .iter()
                .any(|w| w.contains("missing.json") && w.contains("not found")),
            "{:?}",
            got.warnings
        );
    }

    #[test]
    fn unknown_keys_are_tolerated() {
        let f = fixture();
        write(
            &f.user_dir.join("settings.json"),
            &json!({"model": "m", "future_feature": {"x": 1}, "another": [1, 2]}),
        );
        let got = load(&f, None);
        assert_eq!(got.settings.model.as_deref(), Some("m"));
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }

    #[test]
    fn no_git_root_uses_cwd_dot_locode() {
        // Without .git the "project root" is the cwd itself.
        let dir = tempfile::tempdir().unwrap();
        let cwd = fs::canonicalize(dir.path()).unwrap();
        write(
            &cwd.join(".locode/settings.json"),
            &json!({"model": "here"}),
        );
        let got = load_settings_from(None, &cwd, None, None);
        assert_eq!(got.settings.model.as_deref(), Some("here"));
    }

    #[test]
    fn invalid_root_stop_pattern_warns_but_survives() {
        let f = fixture();
        write(
            &f.user_dir.join("settings.json"),
            &json!({"instructions": {"root_stop_pattern": "[bad"}, "model": "m"}),
        );
        let got = load(&f, None);
        assert_eq!(got.settings.model.as_deref(), Some("m"));
        assert_eq!(got.settings.root_stop_pattern.as_deref(), Some("[bad"));
        assert!(
            got.warnings.iter().any(|w| w.contains("root_stop_pattern")),
            "{:?}",
            got.warnings
        );
    }

    #[test]
    fn inline_flag_json_and_non_object_rejection() {
        let f = fixture();
        let got = load(&f, Some(r#"{"harness": "codex"}"#));
        assert_eq!(got.settings.harness.as_deref(), Some("codex"));
        let got = load(&f, Some("[1,2]"));
        assert!(got.warnings.iter().any(|w| w.contains("JSON object")));
    }
}
