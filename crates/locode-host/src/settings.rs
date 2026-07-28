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
use serde_json::{Map, Value};

use crate::root::find_root_from_markers;

/// Keys stripped from the project layers before merging (ADR-0024 §1.3 — a
/// reviewed list: extending it is a normal change, shrinking needs an amendment).
const PROJECT_DENYLIST: &[&str] = &["api_schema"];
/// The user-layer-only pointer key (§1.2 amendment): stripped from project layers.
const EXTENDS_KEY: &str = "extends";

/// The typed view of the merged settings (v1 fields, ADR-0024 §1.4). Unknown keys
/// are tolerated at every layer; absent keys are `None`/empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// Default model (`--model` wins).
    pub model: Option<String>,
    /// Default wire (`--api-schema`/`LOCODE_API_SCHEMA` win). Project-denylisted.
    pub api_schema: Option<String>,
    /// Default harness pack (`--harness` wins).
    pub harness: Option<String>,
    /// Default reasoning effort, as a locode ladder name (`--effort` wins).
    /// Kept as a string here so this crate stays free of provider types; the
    /// caller parses it and warns on an unknown rung rather than failing.
    pub effort: Option<String>,
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
    /// The resolved `extends` dotfolders, in list order (ADR-0024 §1.2 amendment).
    ///
    /// Each also contributes a `skills/` root and an `AGENTS.md` entry, read by their
    /// own loaders. Resolving them once here is what makes the load order an invariant
    /// rather than a convention (ADR-0025 §6.1): a caller cannot discover skills or
    /// instructions without first holding this value.
    pub extends_dirs: Vec<PathBuf>,
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
    // First-run scaffold (user decision 2026-07-24, ADR-0024 §1 amendment): an
    // absent user settings.json is written with the CURRENT defaults, freezing
    // them as explicit config and doubling as a discoverable template.
    if let Some(dir) = &user_dir
        && let Some(notice) = scaffold_user_settings(dir)
    {
        warnings.push(notice);
    }
    let home_for_tilde = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    let mut load = load_settings_from(user_dir.as_deref(), cwd, home_for_tilde.as_deref(), flag);
    warnings.append(&mut load.warnings);
    SettingsLoad {
        settings: load.settings,
        extends_dirs: load.extends_dirs,
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
    // Resolved `extends` dotfolders, in list order — the *other* two things they
    // contribute (skills roots, `AGENTS.md`) are read by their own loaders, which is
    // why the resolved list has to leave this function.
    let mut extends_dirs: Vec<PathBuf> = Vec::new();

    // ---- 1. user layer + 2. its extends dotfolders ----
    merge_user_and_extends_layers(
        user_dir,
        home_for_tilde,
        &mut merged,
        &mut extends_dirs,
        &mut warnings,
    );

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
            effort: raw.effort,
            root_stop_pattern: raw.instructions.root_stop_pattern,
            skills_extra,
        },
        extends_dirs,
        warnings,
    }
}

/// Write `key = value` into the **user** settings file, preserving every other key.
///
/// This is the write half of the settings layer: `/model` persists the model the same
/// way both reference harnesses do — into the user-global file (Claude Code's
/// `updateSettingsForSource('userSettings', { model })`; grok's `[models].default` in
/// `~/.grok/config.toml`). Neither has a project-scoped model, and neither needs one:
/// a running session holds its model in memory, so the file only decides what the
/// **next** session starts with.
///
/// **The write is atomic.** The new contents go to a temp file in the *same directory*
/// — same filesystem, so the rename cannot fail with `EXDEV` — are flushed to disk, and
/// only then renamed over the target. `rename(2)` is atomic, so a crash, a full disk or
/// two processes writing at once can leave the file as the old contents or the new,
/// never a truncated mixture. A half-written `settings.json` would be worse than no
/// write at all: the next run would fall back to defaults with no way to tell why.
///
/// # Errors
/// The path could not be resolved, created, written, or renamed. The existing file is
/// untouched in every failure case.
pub fn update_user_setting(user_dir: &Path, key: &str, value: Value) -> Result<PathBuf, String> {
    let path = user_dir.join("settings.json");
    // Read what is there, so unrelated keys (and unknown ones a newer version wrote)
    // survive. An unreadable or malformed file is replaced rather than merged — there
    // is nothing to preserve in it.
    let mut root = match std::fs::read_to_string(&path) {
        Ok(text) => {
            serde_json::from_str::<Value>(&text).unwrap_or_else(|_| Value::Object(Map::new()))
        }
        Err(_) => Value::Object(Map::new()),
    };
    if !root.is_object() {
        root = Value::Object(Map::new());
    }
    let Some(object) = root.as_object_mut() else {
        unreachable!("just forced to an object");
    };
    match value {
        // `null` removes the key rather than storing a null, so "unset it" round-trips
        // through the same call.
        Value::Null => {
            object.remove(key);
        }
        value => {
            object.insert(key.to_string(), value);
        }
    }
    let text = serde_json::to_string_pretty(&root).map_err(|e| format!("serialize: {e}"))? + "\n";
    crate::trace::create_dir_private(user_dir)?;
    write_atomically(&path, text.as_bytes())?;
    Ok(path)
}

/// Replace `path`'s contents atomically: temp file beside it → flush → rename.
///
/// The temp name carries the pid so two processes writing at once each land in their
/// own file and the rename decides the winner, rather than interleaving into one.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map_or_else(
        || std::ffi::OsString::from("settings.json"),
        std::ffi::OsStr::to_os_string,
    );
    let mut temp_name = name;
    temp_name.push(format!(".tmp.{}", std::process::id()));
    let temp = dir.join(temp_name);

    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        std::io::Write::write_all(&mut file, bytes)?;
        // Flush to the device before the rename: without it a crash right after the
        // rename can leave the new name pointing at an empty file.
        file.sync_all()?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("write {}: {e}", temp.display()));
    }
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("replace {}: {e}", path.display())
    })
}

/// The first-run scaffold: written only when the user `settings.json` is
/// absent. Carries every v1 key with its **current default** — `null` marks
/// "no override" (the factory/built-in default applies) — so the file is both
/// the frozen defaults and a template to edit. `create_new` makes a concurrent
/// first run race-safe (the loser reads the winner's file); any failure is
/// silent (the loader works identically without the file).
fn scaffold_user_settings(user_dir: &Path) -> Option<String> {
    let path = user_dir.join("settings.json");
    if path.exists() {
        return None;
    }
    // Keys in lexicographic order — the emitted file is deterministic
    // regardless of serde_json's map flavor (user decision 2026-07-24).
    let body = serde_json::json!({
        "api_schema": "anthropic",
        "extends": [],
        "harness": "claude",
        "instructions": { "root_stop_pattern": Value::Null },
        "model": "claude-sonnet-5",
        "skills": { "extra": [] },
    });
    let text = serde_json::to_string_pretty(&body).ok()? + "\n";
    crate::trace::create_dir_private(user_dir).ok()?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .ok()?;
    std::io::Write::write_all(&mut file, text.as_bytes()).ok()?;
    Some(format!(
        "settings: created {} with the current defaults",
        path.display()
    ))
}

/// The serde shape of one merged settings document. Plain `Deserialize` — unknown
/// keys are ignored by default, exactly the tolerance ADR-0024 §1.5 requires.
#[derive(Debug, Default, Deserialize)]
struct RawSettings {
    model: Option<String>,
    api_schema: Option<String>,
    harness: Option<String>,
    effort: Option<String>,
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

/// Merge the user layer and each dotfolder it `extends`, collecting the resolved
/// dotfolders on the way (ADR-0024 §1.2 amendment 2026-07-24).
///
/// Split out of [`load_settings_from`] to keep that function readable; the ordering is
/// the interesting part — the user file merges first, then each extended dotfolder in
/// list order, so a later entry wins within the layer and everything here loses to the
/// project layers.
fn merge_user_and_extends_layers(
    user_dir: Option<&Path>,
    home_for_tilde: Option<&Path>,
    merged: &mut Value,
    extends_dirs: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
) {
    let Some(user_dir) = user_dir else { return };
    let user_file = user_dir.join("settings.json");
    let Some(user_value) = read_layer(&user_file, warnings) else {
        return;
    };
    let extends = extract_extends(&user_value, &user_file, warnings);
    merge_values(merged, strip_key(user_value, EXTENDS_KEY));

    for entry in extends {
        let dir = expand_tilde(&entry, home_for_tilde, user_dir);
        // An entry is a **locode dotfolder**, not a settings file: its `settings.json`
        // merges here, and its `skills/` + `AGENTS.md` are read by their own loaders
        // from `extends_dirs`. A file-valued entry is refused explicitly rather than
        // reinterpreted — §1.5 forbids silently changing what an existing key means,
        // and the file form was valid until this amendment.
        if dir.is_file() {
            warnings.push(format!(
                "settings: `extends` entry {} is a file; it must be a locode directory \
                 (point it at the folder holding settings.json)",
                dir.display()
            ));
            continue;
        }
        // The user explicitly pointed at this directory — absence is loud (unlike the
        // standard layers, whose absence is normal).
        if !dir.is_dir() {
            warnings.push(format!(
                "settings: extends directory {} not found; skipped",
                dir.display()
            ));
            continue;
        }
        extends_dirs.push(dir.clone());

        // A dotfolder that ships only skills or only `AGENTS.md` is normal.
        let path = dir.join("settings.json");
        if !path.is_file() {
            continue;
        }
        if let Some(mut value) = read_layer(&path, warnings) {
            // Non-recursive (§1.2 amendment): a nested `extends` is ignored.
            if value.get(EXTENDS_KEY).is_some() {
                warnings.push(format!(
                    "settings: nested `extends` in {} ignored (extends does not recurse)",
                    path.display()
                ));
                value = strip_key(value, EXTENDS_KEY);
            }
            merge_values(merged, value);
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

    // ── the write half ──────────────────────────────────────────────────────

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Setting one key leaves every other one alone — including keys this version does
    /// not know about, which a newer version may have written.
    #[test]
    fn updating_a_key_preserves_the_rest_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write(
            &path,
            &json!({
                "harness": "claude",
                "model": "claude-sonnet-5",
                "skills": { "extra": ["~/x"] },
                "some_future_key": 42,
            }),
        );

        let written = update_user_setting(dir.path(), "model", json!("claude-opus-5")).unwrap();
        assert_eq!(written, path);
        let got = read_json(&path);
        assert_eq!(got["model"], "claude-opus-5");
        assert_eq!(got["harness"], "claude", "untouched");
        assert_eq!(got["skills"]["extra"][0], "~/x", "nested value untouched");
        assert_eq!(got["some_future_key"], 42, "unknown key survives");
    }

    /// A `null` unsets rather than storing a null, so the same call both sets and clears.
    #[test]
    fn a_null_value_removes_the_key() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("settings.json"),
            &json!({"model": "x", "harness": "claude"}),
        );
        update_user_setting(dir.path(), "model", Value::Null).unwrap();
        let got = read_json(&dir.path().join("settings.json"));
        assert!(got.get("model").is_none(), "{got}");
        assert_eq!(got["harness"], "claude");
    }

    /// No file, or an unreadable one, still yields a valid file with the key set — and
    /// the loader reads it back.
    #[test]
    fn a_missing_or_corrupt_file_is_replaced_not_merged() {
        let dir = tempfile::tempdir().unwrap();
        update_user_setting(dir.path(), "model", json!("m1")).unwrap();
        assert_eq!(read_json(&dir.path().join("settings.json"))["model"], "m1");

        fs::write(dir.path().join("settings.json"), "{ not json").unwrap();
        update_user_setting(dir.path(), "model", json!("m2")).unwrap();
        assert_eq!(read_json(&dir.path().join("settings.json"))["model"], "m2");
    }

    /// What the writer exists to guarantee: the target is **replaced by rename**, never
    /// written through. A reader therefore sees the old contents or the new, never a
    /// truncated mixture — so a crash mid-write cannot lose the user's settings.
    #[test]
    fn the_write_is_atomic_and_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write(&path, &json!({"model": "old"}));

        // A big payload: a write-through implementation would be observably torn here,
        // and would leave the temp file if it aborted.
        let big: Vec<String> = (0..2000).map(|i| format!("entry-{i}")).collect();
        update_user_setting(dir.path(), "skills", json!({ "extra": big })).unwrap();

        let got = read_json(&path);
        assert_eq!(got["model"], "old", "the untouched key survived");
        assert_eq!(got["skills"]["extra"].as_array().unwrap().len(), 2000);

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "settings.json")
            .collect();
        assert!(leftovers.is_empty(), "temp files cleaned up: {leftovers:?}");
    }

    /// The written file is what the loader reads — the round trip has to close.
    #[test]
    fn the_written_model_is_what_the_loader_resolves() {
        let f = fixture();
        update_user_setting(&f.user_dir, "model", json!("claude-opus-5")).unwrap();
        let load = load_settings_from(Some(&f.user_dir), &f.repo, Some(&f.home), None);
        assert_eq!(load.settings.model.as_deref(), Some("claude-opus-5"));
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
                    "extends": ["team"]}),
        );
        write(
            &f.user_dir.join("team/settings.json"),
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
            &json!({"extends": ["a", "b"]}),
        );
        write(&f.user_dir.join("a/settings.json"), &json!({"model": "a"}));
        write(
            &f.user_dir.join("b/settings.json"),
            &json!({"model": "b", "extends": ["c"]}),
        );
        write(&f.user_dir.join("c/settings.json"), &json!({"model": "c"}));

        let got = load(&f, None);
        // Later extends entry wins; `c` never loads (no recursion).
        assert_eq!(got.settings.model.as_deref(), Some("b"));
        assert!(
            got.warnings.iter().any(|w| w.contains("nested `extends`")),
            "{:?}",
            got.warnings
        );
        assert_eq!(
            got.extends_dirs,
            vec![f.user_dir.join("a"), f.user_dir.join("b")],
            "resolved dotfolders travel out in list order"
        );
    }

    /// A dotfolder may ship only skills or only `AGENTS.md`; a missing
    /// `settings.json` is normal and must stay silent.
    #[test]
    fn extends_dotfolder_without_settings_json_is_silent() {
        let f = fixture();
        write(
            &f.user_dir.join("settings.json"),
            &json!({"extends": ["team"]}),
        );
        std::fs::create_dir_all(f.user_dir.join("team/skills")).unwrap();

        let got = load(&f, None);
        assert_eq!(got.extends_dirs, vec![f.user_dir.join("team")]);
        assert!(
            got.warnings.is_empty(),
            "a dotfolder with no settings.json is normal: {:?}",
            got.warnings
        );
    }

    /// The old form (an entry pointing at a settings *file*) is refused with a message
    /// naming the fix — never reinterpreted as "a directory with no settings.json",
    /// which would silently drop a layer the user still expects (ADR-0024 §1.5).
    #[test]
    fn extends_entry_pointing_at_a_file_is_refused_explicitly() {
        let f = fixture();
        write(
            &f.user_dir.join("settings.json"),
            &json!({"extends": ["team.json"]}),
        );
        write(&f.user_dir.join("team.json"), &json!({"model": "team"}));

        let got = load(&f, None);
        assert_eq!(got.settings.model, None, "the file must not be merged");
        assert!(got.extends_dirs.is_empty());
        let w = got.warnings.join(" | ");
        assert!(w.contains("is a file"), "{w}");
        assert!(w.contains("must be a locode directory"), "{w}");
    }

    #[test]
    fn extends_missing_directory_warns_loudly() {
        let f = fixture();
        write(
            &f.user_dir.join("settings.json"),
            &json!({"extends": ["nope"]}),
        );
        let got = load(&f, None);
        assert!(got.extends_dirs.is_empty());
        assert!(
            got.warnings.iter().any(|w| w.contains("not found")),
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
    fn scaffold_writes_current_defaults_once() {
        let dir = tempfile::tempdir().unwrap();
        let user_dir = dir.path().join(".locode");
        // Absent file (and absent dir): scaffolded.
        let notice = scaffold_user_settings(&user_dir).expect("scaffolded");
        assert!(notice.contains("settings.json"));
        let path = user_dir.join("settings.json");
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["harness"], "claude");
        assert_eq!(value["api_schema"], "anthropic");
        assert_eq!(value["model"], "claude-sonnet-5");
        assert_eq!(value["skills"]["extra"], serde_json::json!([]));
        // The scaffold round-trips through the loader with the same effective
        // result as no file at all (nulls decode to None).
        let cwd = tempfile::tempdir().unwrap();
        let got = load_settings_from(Some(&user_dir), cwd.path(), None, None);
        assert_eq!(got.settings.harness.as_deref(), Some("claude"));
        assert_eq!(got.settings.model.as_deref(), Some("claude-sonnet-5"));
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
        // Second call: never overwrites.
        fs::write(&path, r#"{"harness":"claude"}"#).unwrap();
        assert!(scaffold_user_settings(&user_dir).is_none());
        let kept: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(kept["harness"], "claude", "existing file untouched");
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
