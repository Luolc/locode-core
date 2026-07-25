//! The `SKILL.md` YAML frontmatter reader.
//!
//! Splitting the `---` fences is ours; parsing what is between them is
//! [`serde_yaml_ng`]'s. Both reference harnesses do exactly this — codex runs
//! `serde_yaml::from_str::<SkillFrontmatter>` (`core-skills/src/loader.rs:747`) and grok
//! coerces from `serde_yaml::Value` (`skills/discovery.rs:150-176`) — and the reason
//! matters: real skills use YAML that a scalar scanner silently loses. A folded
//! `description: >` spanning three lines is the common one, and losing it drops the
//! skill entirely, since a skill with no description cannot be routed to.
//!
//! Only the five recognized keys (ADR-0025 §2) are deserialized. Everything else —
//! `allowed-tools`, `model`, `paths`, … — is *ignored*, which is serde's default for
//! unknown fields and is the behavior the ADR requires.

use serde::Deserialize;

/// The five recognized keys. Absent fields stay `None`/default; unknown keys are
/// ignored rather than rejected, so a skill authored for another harness still loads.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Frontmatter {
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    #[serde(rename = "when-to-use", alias = "when_to_use")]
    pub(crate) when_to_use: Option<String>,
    #[serde(
        rename = "disable-model-invocation",
        alias = "disable_model_invocation"
    )]
    disable_model_invocation: Option<serde_yaml_ng::Value>,
    #[serde(rename = "user-invocable", alias = "user_invocable")]
    user_invocable: Option<serde_yaml_ng::Value>,
}

impl Frontmatter {
    /// `disable-model-invocation`, defaulting to `false`.
    pub(crate) fn disable_model_invocation(&self) -> bool {
        truthy(self.disable_model_invocation.as_ref()).unwrap_or(false)
    }

    /// `user-invocable`, defaulting to `true` (a skill is user-invocable unless it
    /// says otherwise).
    pub(crate) fn user_invocable(&self) -> bool {
        truthy(self.user_invocable.as_ref()).unwrap_or(true)
    }
}

/// YAML 1.2 makes `yes`/`on` plain strings, but skill authors write them as booleans
/// (grok's own test fixtures use `user-invocable: yes`). Coerce both spellings, exactly
/// as grok's `parse_boolean_frontmatter` does.
fn truthy(value: Option<&serde_yaml_ng::Value>) -> Option<bool> {
    match value? {
        serde_yaml_ng::Value::Bool(b) => Some(*b),
        serde_yaml_ng::Value::String(s) => Some(matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "true" | "yes" | "on" | "1"
        )),
        serde_yaml_ng::Value::Number(n) => Some(n.as_f64().is_some_and(|f| f != 0.0)),
        _ => None,
    }
}

/// Split a `SKILL.md` into `(frontmatter, markdown_body)`.
///
/// `None` when the file does not open with a `---` fence, when the block is
/// unterminated, or when the YAML does not parse — in every case the caller skips the
/// skill with a diagnostic rather than guessing at half-read metadata.
pub(crate) fn parse(source: &str) -> Option<(Frontmatter, &str)> {
    let text = source.strip_prefix('\u{feff}').unwrap_or(source);
    let after_open = strip_open_fence(text)?;
    let (block, body) = split_at_closing_fence(after_open)?;
    let fm: Frontmatter = serde_yaml_ng::from_str(block).ok()?;
    Some((fm, body))
}

/// Consume the opening `---` (skipping leading blank lines).
fn strip_open_fence(text: &str) -> Option<&str> {
    let mut rest = text;
    loop {
        let (line, tail) = split_line(rest);
        if line.trim().is_empty() {
            rest = tail?;
            continue;
        }
        return if line.trim_end() == "---" { tail } else { None };
    }
}

/// Split at the first closing `---`/`...`, returning `(block, body_after)`.
fn split_at_closing_fence(text: &str) -> Option<(&str, &str)> {
    let mut offset = 0usize;
    let mut rest = text;
    loop {
        let (line, tail) = split_line(rest);
        if matches!(line.trim_end(), "---" | "...") {
            return Some((&text[..offset], tail.unwrap_or("")));
        }
        let tail = tail?;
        offset += rest.len() - tail.len();
        rest = tail;
    }
}

/// `(line_without_newline, rest_after_newline_or_None_at_eof)`.
fn split_line(text: &str) -> (&str, Option<&str>) {
    match text.find('\n') {
        Some(i) => (&text[..i], Some(&text[i + 1..])),
        None => (text, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(src: &str) -> Frontmatter {
        parse(src).expect("frontmatter").0
    }

    #[test]
    fn reads_the_five_keys_and_the_body() {
        let (f, body) = parse(
            "---\nname: commit\ndescription: Make a commit\nwhen-to-use: on push\n\
             disable-model-invocation: true\nuser-invocable: false\n---\n# Body\n",
        )
        .expect("frontmatter");
        assert_eq!(f.name.as_deref(), Some("commit"));
        assert_eq!(f.description.as_deref(), Some("Make a commit"));
        assert_eq!(f.when_to_use.as_deref(), Some("on push"));
        assert!(f.disable_model_invocation());
        assert!(!f.user_invocable());
        assert_eq!(body, "# Body\n");
    }

    #[test]
    fn defaults_when_absent() {
        let f = fm("---\nname: x\n---\n");
        assert!(!f.disable_model_invocation(), "model may invoke by default");
        assert!(f.user_invocable(), "user may invoke by default");
        assert!(f.description.is_none());
    }

    /// The case the hand-rolled scanner silently lost: a folded description spanning
    /// several lines. Losing it drops the skill, since a skill with no description
    /// cannot be routed to.
    #[test]
    fn folded_and_literal_block_scalars_are_read() {
        let f = fm("---\nname: x\ndescription: >\n  first line\n  second line\n---\n");
        assert_eq!(f.description.as_deref(), Some("first line second line\n"));

        let f = fm("---\nname: x\ndescription: |\n  line one\n  line two\n---\n");
        assert_eq!(f.description.as_deref(), Some("line one\nline two\n"));
    }

    /// Unknown keys — including the ones ADR-0025 deliberately does not honor — are
    /// ignored, so a skill authored for another harness still loads.
    #[test]
    fn unknown_keys_are_ignored_not_rejected() {
        let f = fm(
            "---\nname: x\ndescription: D\nallowed-tools:\n  - Bash\n  - Edit\n\
             model: opus\npaths:\n  \"*.rs\": true\n---\n",
        );
        assert_eq!(f.name.as_deref(), Some("x"));
        assert_eq!(f.description.as_deref(), Some("D"));
    }

    #[test]
    fn quoted_values_and_embedded_colons() {
        let f = fm("---\nname: \"commit\"\ndescription: \"Deploy: push to prod\"\n---\n");
        assert_eq!(f.name.as_deref(), Some("commit"));
        assert_eq!(f.description.as_deref(), Some("Deploy: push to prod"));
    }

    /// Authors write `yes`/`no`, which YAML 1.2 calls strings — coerce both spellings.
    #[test]
    fn yes_no_booleans_are_coerced() {
        assert!(!fm("---\nuser-invocable: no\n---\n").user_invocable());
        assert!(fm("---\ndisable-model-invocation: yes\n---\n").disable_model_invocation());
        assert!(!fm("---\ndisable-model-invocation: off\n---\n").disable_model_invocation());
    }

    #[test]
    fn missing_or_broken_frontmatter_is_none() {
        assert!(parse("# just markdown\n").is_none(), "no fence");
        assert!(parse("---\nname: x\n").is_none(), "unterminated");
        assert!(parse("---\n\tname: [\n---\n").is_none(), "invalid yaml");
    }
}
