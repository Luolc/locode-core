//! Skill-backed commands (ADR-0026 §4) — the half of skills ADR-0025 left inert.
//!
//! ADR-0025 parses `user-invocable` and does nothing with it, because the model's
//! listing is the only channel a skill has. Registering each such skill as a command is
//! that missing channel, and it is why this task is sequenced before background work.

use std::sync::Arc;

use locode_skills::{Skill, invocation_text, read_body};

use super::command::{CommandCtx, CommandResult, SlashCommand};
use super::registry::{CommandRegistry, CommandSource};

/// A command that splices a skill's body into the turn.
#[derive(Debug, Clone)]
pub struct SkillCommand {
    name: String,
    description: String,
    usage: String,
    skill: Skill,
}

impl SkillCommand {
    /// Wrap a discovered skill, under `name` (its own, or a qualified one on collision).
    #[must_use]
    pub fn new(skill: Skill, name: String) -> Self {
        Self {
            usage: format!("/{name} [args]"),
            description: skill.description.clone(),
            name,
            skill,
        }
    }
}

#[async_trait::async_trait]
impl SlashCommand for SkillCommand {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn usage(&self) -> &str {
        &self.usage
    }

    /// Arguments are accepted but never required — a skill is a set of instructions,
    /// and running it with none is a normal thing to want.
    fn takes_args(&self) -> bool {
        true
    }

    async fn execute(&self, _ctx: &CommandCtx<'_>, args: &str) -> CommandResult {
        // Read at invocation, not at discovery: editing a skill takes effect on the
        // next use rather than the next rescan.
        match read_body(&self.skill.path) {
            Ok(body) => CommandResult::InjectSkill {
                display_text: if args.trim().is_empty() {
                    format!("/{}", self.name)
                } else {
                    format!("/{} {}", self.name, args.trim())
                },
                prompt_text: invocation_text(&body, args),
            },
            Err(e) => CommandResult::Error(format!(
                "skill `{}`: cannot read {}: {e}",
                self.name,
                self.skill.path.display()
            )),
        }
    }
}

/// Register every user-invocable skill as a command.
///
/// Call **after** the builtins: registration is first-wins, so that ordering is what
/// gives ADR-0026 §4's builtin-beats-skill precedence. A skill whose name is already
/// taken is registered under its qualified `<scope>:<name>` instead of being dropped,
/// reusing ADR-0025 §2's qualifier scheme rather than inventing a second one.
pub fn register_skills(registry: &mut CommandRegistry, skills: &[Skill]) {
    for skill in skills {
        if !skill.user_invocable {
            continue;
        }
        let taken = registry
            .triggers()
            .iter()
            .any(|t| t.match_text == skill.name);
        let name = if taken {
            format!("{}:{}", skill.scope.as_str(), skill.name)
        } else {
            skill.name.clone()
        };
        registry.register(
            Arc::new(SkillCommand::new(skill.clone(), name)),
            CommandSource::Skill,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locode_skills::SkillScope;
    use std::path::PathBuf;

    fn skill(name: &str, path: PathBuf, user_invocable: bool) -> Skill {
        Skill {
            name: name.to_string(),
            scope: SkillScope::User,
            description: format!("does {name}"),
            when_to_use: None,
            path,
            disable_model_invocation: false,
            user_invocable,
        }
    }

    fn write_skill(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let d = dir.join(name);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("SKILL.md");
        std::fs::write(
            &p,
            format!("---\nname: {name}\ndescription: d\n---\n{body}"),
        )
        .unwrap();
        p
    }

    #[tokio::test]
    async fn invoking_a_skill_yields_its_body_plus_arguments() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_skill(dir.path(), "commit", "# Commit\nStage, then commit.\n");
        let cmd = SkillCommand::new(skill("commit", path, true), "commit".into());

        let ctx = CommandCtx::default();
        match cmd.execute(&ctx, "fix the typo").await {
            CommandResult::InjectSkill {
                display_text,
                prompt_text,
            } => {
                assert_eq!(display_text, "/commit fix the typo");
                assert!(prompt_text.starts_with("# Commit"), "{prompt_text}");
                assert!(
                    prompt_text.ends_with("**ARGUMENTS:** fix the typo"),
                    "{prompt_text}"
                );
            }
            other => panic!("expected InjectSkill, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_deleted_skill_reports_the_path_instead_of_panicking() {
        let cmd = SkillCommand::new(
            skill("gone", PathBuf::from("/nope/SKILL.md"), true),
            "gone".into(),
        );
        let ctx = CommandCtx::default();
        assert!(matches!(
            cmd.execute(&ctx, "").await,
            CommandResult::Error(msg) if msg.contains("/nope/SKILL.md")
        ));
    }

    #[test]
    fn only_user_invocable_skills_register() {
        let dir = tempfile::TempDir::new().unwrap();
        let a = write_skill(dir.path(), "yes", "x");
        let b = write_skill(dir.path(), "no", "x");
        let mut r = CommandRegistry::new();
        register_skills(&mut r, &[skill("yes", a, true), skill("no", b, false)]);
        let names: Vec<&str> = r.triggers().iter().map(|t| t.match_text.as_str()).collect();
        assert_eq!(names, vec!["yes"]);
    }

    /// A skill colliding with a builtin keeps its qualified name rather than vanishing.
    #[test]
    fn a_colliding_skill_registers_qualified() {
        struct Builtin;
        #[async_trait::async_trait]
        impl SlashCommand for Builtin {
            fn name(&self) -> &'static str {
                "commit"
            }
            fn description(&self) -> &'static str {
                "builtin"
            }
            fn usage(&self) -> &'static str {
                "/commit"
            }
            async fn execute(&self, _c: &CommandCtx<'_>, _a: &str) -> CommandResult {
                CommandResult::Handled
            }
        }

        let dir = tempfile::TempDir::new().unwrap();
        let p = write_skill(dir.path(), "commit", "x");
        let mut r = CommandRegistry::new();
        r.register(Arc::new(Builtin), CommandSource::Builtin);
        register_skills(&mut r, &[skill("commit", p, true)]);

        let names: Vec<&str> = r.triggers().iter().map(|t| t.match_text.as_str()).collect();
        assert_eq!(names, vec!["commit", "user:commit"]);
        assert_eq!(
            r.resolve("/commit").expect("resolves").0.description(),
            "builtin",
            "the builtin keeps the bare name"
        );
    }
}
