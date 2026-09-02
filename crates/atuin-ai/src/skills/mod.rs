//! AI skill discovery, metadata, and lazy loading.
//!
//! Skills are markdown files (`SKILL.md`) with YAML frontmatter that define
//! reusable instructions for the LLM. Only skill metadata (name + description)
//! is sent to the server; full content is loaded on demand via `load_skill`.

mod frontmatter;
pub mod walker;

use std::path::Path;

use atuin_common::string::ellipsis::{Indicator, Pos};
use atuin_common::string::{EllipsizeExt as _, Measure};
use eyre::{Result, eyre};

use crate::user_context::interpolate;

/// Per-skill description truncation limit (before budget calculation).
const MAX_DESCRIPTION_LEN: usize = 1024;

/// Default total character budget for skill descriptions sent to the server.
const DEFAULT_DESCRIPTION_BUDGET: usize = 9992;

/// JSON overhead per skill entry: `{"name":"","description":""},` ≈ 30 chars.
const PER_ENTRY_OVERHEAD: usize = 30;

/// Metadata for a discovered skill. Produced at discovery time from
/// frontmatter only — the body is not read until `load()`.
#[derive(Debug, Clone)]
pub struct SkillDescriptor {
    pub name: String,
    pub description: String,
    pub source_path: std::path::PathBuf,
    pub disable_model_invocation: bool,
}

/// A name + description pair ready to serialize into the request payload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
}

/// Holds discovered skills and provides lookup, budget packing, and loading.
#[derive(Debug, Clone)]
pub struct SkillRegistry {
    skills: Vec<SkillDescriptor>,
}

impl SkillRegistry {
    /// Discover skills from project and global directories.
    pub async fn discover(project_root: Option<&Path>) -> Self {
        let global_dir = walker::global_skills_dir();
        let project_dir = project_root.map(walker::project_skills_dir);

        Self::discover_from_dirs(project_dir.as_deref(), &global_dir).await
    }

    /// Discover skills from explicit directory paths. Useful for testing.
    pub async fn discover_from_dirs(
        project_skills_dir: Option<&Path>,
        global_skills_dir: &Path,
    ) -> Self {
        let raw_files = walker::discover(project_skills_dir, global_skills_dir).await;

        let mut skills = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for raw in raw_files {
            let parsed = frontmatter::parse(&raw.content);
            let fm = parsed.frontmatter;

            let name = fm.name.unwrap_or_else(|| sanitize_name(&raw.dir_name));

            // Deduplicate: first seen wins (project before global)
            if !seen_names.insert(name.clone()) {
                continue;
            }

            let description =
                fm.description.or_else(|| first_paragraph(&parsed.body)).unwrap_or_default();

            skills.push(SkillDescriptor {
                name,
                description,
                source_path: raw.path,
                disable_model_invocation: fm.disable_model_invocation,
            });
        }

        Self { skills }
    }

    /// Create an empty registry.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self { skills: Vec::new() }
    }

    /// Look up a skill by name.
    pub fn get(&self, name: &str) -> Option<&SkillDescriptor> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// All discovered skills.
    pub fn all(&self) -> &[SkillDescriptor] {
        &self.skills
    }

    /// Whether any non-disabled skills exist (determines capability advertisement).
    #[cfg(test)]
    pub fn has_server_visible_skills(&self) -> bool {
        self.skills.iter().any(|s| !s.disable_model_invocation)
    }

    /// Pack skill descriptions into the server payload under a character budget.
    ///
    /// Returns the summaries that fit plus an optional overflow message.
    pub fn server_skills(&self) -> (Vec<SkillSummary>, Option<String>) {
        self.server_skills_with_budget(DEFAULT_DESCRIPTION_BUDGET)
    }

    pub fn server_skills_with_budget(&self, budget: usize) -> (Vec<SkillSummary>, Option<String>) {
        let eligible: Vec<&SkillDescriptor> =
            self.skills.iter().filter(|s| !s.disable_model_invocation).collect();

        let mut summaries = Vec::new();
        let mut used = 0;
        let mut overflow_names = Vec::new();

        for skill in &eligible {
            // Truncate to a byte budget (not display columns): descriptions are
            // packed into a byte-bounded LLM prompt budget.
            let truncated_desc = skill
                .description
                .ellipsize(Measure::Bytes(MAX_DESCRIPTION_LEN), Pos::End, Indicator::ASCII)
                .to_string();
            let entry_size = skill.name.len() + truncated_desc.len() + PER_ENTRY_OVERHEAD;

            if used + entry_size > budget && !summaries.is_empty() {
                overflow_names.push(skill.name.as_str());
                continue;
            }

            used += entry_size;
            summaries.push(SkillSummary {
                name: skill.name.clone(),
                description: truncated_desc,
            });
        }

        let overflow = if overflow_names.is_empty() {
            None
        } else {
            Some(format!(
                "{} additional skill(s) not listed due to size limits: {}",
                overflow_names.len(),
                overflow_names.join(", ")
            ))
        };

        (summaries, overflow)
    }

    /// Load a skill's full body content, with argument substitution and
    /// `!`` interpolation applied.
    ///
    /// `$ARGUMENTS` in the body is replaced with the provided arguments before
    /// shell interpolation runs. If `$ARGUMENTS` does not appear in the body
    /// and arguments were provided, they are appended as `ARGUMENTS: <value>`.
    pub async fn load(&self, name: &str, shell: &str, arguments: Option<&str>) -> Result<String> {
        let skill = self.get(name).ok_or_else(|| eyre!("Unknown skill: {name}"))?;

        let content = tokio::fs::read_to_string(&skill.source_path).await?;
        let parsed = frontmatter::parse(&content);
        let body = parsed.body;

        if body.trim().is_empty() {
            return Ok(format!("(Skill '{name}' has no body content)"));
        }

        let body = substitute_arguments(&body, arguments);

        Ok(interpolate::interpolate(&body, shell).await)
    }
}

/// Replace `$ARGUMENTS` placeholders in skill body text.
///
/// If `$ARGUMENTS` appears in the body, all occurrences are replaced with the
/// argument string (or empty string if none). If `$ARGUMENTS` does not appear
/// and arguments were provided, they are appended on a new line.
fn substitute_arguments(body: &str, arguments: Option<&str>) -> String {
    let args = arguments.unwrap_or("");

    if body.contains("$ARGUMENTS") {
        return body.replace("$ARGUMENTS", args);
    }

    if !args.is_empty() {
        return format!("{body}\n\nARGUMENTS: {args}");
    }

    body.to_string()
}

/// Sanitize a directory name into a valid skill name.
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .to_lowercase()
}

/// Extract the first non-empty paragraph from markdown body text.
fn first_paragraph(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }

    let para: String =
        trimmed.lines().take_while(|line| !line.trim().is_empty()).collect::<Vec<_>>().join(" ");

    let para = para.trim().to_string();
    if para.is_empty() {
        None
    } else {
        Some(para)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// Build a `SkillDescriptor` from its distinguishing fields, deriving the
    /// source path from the name.
    fn descriptor(name: &str, description: &str, disabled: bool) -> SkillDescriptor {
        SkillDescriptor {
            name: name.to_string(),
            description: description.to_string(),
            source_path: format!("{name}/SKILL.md").into(),
            disable_model_invocation: disabled,
        }
    }

    #[rstest]
    #[case("My Skill", "my-skill")]
    #[case("deploy_prod", "deploy-prod")]
    #[case("code-review", "code-review")]
    fn sanitize_name_cases(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(sanitize_name(input), expected);
    }

    #[rstest]
    #[case("Hello world\nSecond line\n\nNew paragraph", Some("Hello world Second line"))]
    #[case("", None)]
    #[case("\n\n", None)]
    #[case("Single line", Some("Single line"))]
    fn first_paragraph_cases(#[case] input: &str, #[case] expected: Option<&str>) {
        assert_eq!(first_paragraph(input), expected.map(String::from));
    }

    #[rstest]
    #[case::replaces_placeholder(
        "Deploy $ARGUMENTS to production.",
        Some("patch"),
        "Deploy patch to production."
    )]
    #[case::multiple_occurrences(
        "Run $ARGUMENTS then verify $ARGUMENTS worked.",
        Some("migrate"),
        "Run migrate then verify migrate worked."
    )]
    #[case::appends_when_no_placeholder(
        "Do the thing.",
        Some("extra context"),
        "Do the thing.\n\nARGUMENTS: extra context"
    )]
    #[case::no_args_no_placeholder("Just a body.", None, "Just a body.")]
    #[case::no_args_clears_placeholder(
        "Deploy $ARGUMENTS to production.",
        None,
        "Deploy  to production."
    )]
    fn substitute_arguments_cases(
        #[case] body: &str,
        #[case] args: Option<&str>,
        #[case] expected: &str,
    ) {
        assert_eq!(substitute_arguments(body, args), expected);
    }

    #[test]
    fn budget_packing() {
        let registry = SkillRegistry {
            skills: vec![
                descriptor("a", "Short desc", false),
                descriptor("b", "Another desc", false),
            ],
        };

        let (summaries, overflow) = registry.server_skills_with_budget(4096);
        assert_eq!(summaries.len(), 2);
        assert!(overflow.is_none());
    }

    #[test]
    fn budget_overflow() {
        let registry = SkillRegistry {
            skills: vec![
                descriptor("first", &"x".repeat(200), false),
                descriptor("second", &"y".repeat(200), false),
            ],
        };

        // Budget only fits one
        let (summaries, overflow) = registry.server_skills_with_budget(260);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].name, "first");
        let overflow = overflow.unwrap();
        assert!(overflow.contains("second"));
        assert!(overflow.contains("1 additional"));
    }

    #[test]
    fn disabled_skills_excluded_from_server() {
        let registry = SkillRegistry {
            skills: vec![
                descriptor("visible", "I show up", false),
                descriptor("hidden", "I don't", true),
            ],
        };

        let (summaries, _) = registry.server_skills();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].name, "visible");

        // But all() includes both
        assert_eq!(registry.all().len(), 2);
    }

    #[test]
    fn has_server_visible_skills() {
        let empty = SkillRegistry::empty();
        assert!(!empty.has_server_visible_skills());

        let all_disabled = SkillRegistry {
            skills: vec![descriptor("hidden", "", true)],
        };
        assert!(!all_disabled.has_server_visible_skills());

        let some_visible = SkillRegistry {
            skills: vec![descriptor("visible", "", false)],
        };
        assert!(some_visible.has_server_visible_skills());
    }

    #[tokio::test]
    async fn end_to_end_discover() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");

        // Create a skill with frontmatter
        let skill_dir = skills_dir.join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: A test skill\n---\n\nBody here.\n",
        )
        .unwrap();

        // Create a skill with multiline description
        let skill_dir2 = skills_dir.join("release");
        std::fs::create_dir_all(&skill_dir2).unwrap();
        std::fs::write(
            skill_dir2.join("SKILL.md"),
            "---\nname: release\ndescription: >\n  Multi-line\n  description \
             here.\n---\n\nRelease steps.\n",
        )
        .unwrap();

        let registry = SkillRegistry::discover_from_dirs(
            Some(&skills_dir),
            &std::path::PathBuf::from("/nonexistent"),
        )
        .await;
        assert_eq!(registry.all().len(), 2);

        let my_skill = registry.get("my-skill").unwrap();
        assert_eq!(my_skill.description, "A test skill");

        let release = registry.get("release").unwrap();
        assert!(release.description.contains("Multi-line"));
    }
}
