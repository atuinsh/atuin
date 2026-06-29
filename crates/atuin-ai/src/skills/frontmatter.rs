//! YAML frontmatter parsing for `SKILL.md` files.
//!
//! Extracts the YAML block between `---` delimiters and parses it with
//! `yaml-rust2`. Returns the parsed fields and the byte offset where the
//! body begins (after the closing `---`).

use yaml_rust2::YamlLoader;

/// Parsed frontmatter fields from a `SKILL.md` file.
#[derive(Debug, Default)]
pub(crate) struct Frontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub disable_model_invocation: bool,
}

/// Result of splitting a skill file into frontmatter + body.
#[derive(Debug)]
pub(crate) struct ParsedSkillFile {
    pub frontmatter: Frontmatter,
    /// Everything after the closing `---` delimiter.
    pub body: String,
}

/// Parse a `SKILL.md` file's content into frontmatter and body.
///
/// If no frontmatter delimiters are found, all content is treated as body
/// with default frontmatter.
pub(crate) fn parse(content: &str) -> ParsedSkillFile {
    let Some((yaml_str, body)) = split_frontmatter(content) else {
        return ParsedSkillFile {
            frontmatter: Frontmatter::default(),
            body: content.to_string(),
        };
    };

    let frontmatter = match YamlLoader::load_from_str(yaml_str) {
        Ok(docs) if !docs.is_empty() => extract_fields(&docs[0]),
        Ok(_) => Frontmatter::default(),
        Err(e) => {
            tracing::warn!("Failed to parse skill frontmatter: {e}");
            Frontmatter::default()
        }
    };

    ParsedSkillFile { frontmatter, body }
}

/// Split content on `---` delimiters. Returns `(yaml_str, body)` or `None`
/// if frontmatter is not present.
fn split_frontmatter(content: &str) -> Option<(&str, String)> {
    let trimmed = content.trim_start();

    // Must start with `---`
    if !trimmed.starts_with("---") {
        return None;
    }

    // Find the end of the opening delimiter line
    let after_open = trimmed.get(3..)?.trim_start_matches(|c: char| c != '\n');
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);

    // Find the closing `---`
    let close_pos = after_open
        .lines()
        .enumerate()
        .find(|(_, line)| line.trim() == "---")
        .map(|(i, _)| {
            after_open
                .lines()
                .take(i)
                .map(|l| l.len() + 1) // +1 for newline
                .sum::<usize>()
        })?;

    let yaml_str = &after_open[..close_pos];
    let rest = &after_open[close_pos..];
    // Skip the closing `---` line
    let body = rest
        .strip_prefix("---")
        .unwrap_or(rest)
        .trim_start_matches(|c: char| c != '\n');
    let body = body.strip_prefix('\n').unwrap_or(body);

    Some((yaml_str, body.to_string()))
}

fn extract_fields(doc: &yaml_rust2::Yaml) -> Frontmatter {
    use yaml_rust2::Yaml;

    let name = match &doc["name"] {
        Yaml::String(s) => Some(s.clone()),
        _ => None,
    };

    let description = match &doc["description"] {
        Yaml::String(s) => Some(s.trim().to_string()),
        _ => None,
    };

    let disable_model_invocation = match &doc["disable-model-invocation"] {
        Yaml::Boolean(b) => *b,
        Yaml::String(s) => matches!(s.as_str(), "true" | "yes" | "1"),
        _ => false,
    };

    Frontmatter {
        name,
        description,
        disable_model_invocation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_frontmatter() {
        let content = "\
---
name: my-skill
description: A test skill
disable-model-invocation: true
---

Body content here.
";
        let parsed = parse(content);
        assert_eq!(parsed.frontmatter.name.as_deref(), Some("my-skill"));
        assert_eq!(
            parsed.frontmatter.description.as_deref(),
            Some("A test skill")
        );
        assert!(parsed.frontmatter.disable_model_invocation);
        assert_eq!(parsed.body.trim(), "Body content here.");
    }

    #[test]
    fn multiline_folded_description() {
        let content = "\
---
name: release
description: >
  Orchestrate a multi-step release — version bumping, changelog
  generation, PR creation, tagging, and publishing.
disable-model-invocation: true
---

# Release steps
";
        let parsed = parse(content);
        assert_eq!(parsed.frontmatter.name.as_deref(), Some("release"));
        let desc = parsed.frontmatter.description.unwrap();
        assert!(desc.contains("Orchestrate a multi-step release"));
        assert!(desc.contains("publishing"));
        assert!(parsed.frontmatter.disable_model_invocation);
        assert!(parsed.body.contains("# Release steps"));
    }

    #[test]
    fn no_frontmatter() {
        let content = "Just a body with no frontmatter.";
        let parsed = parse(content);
        assert!(parsed.frontmatter.name.is_none());
        assert!(parsed.frontmatter.description.is_none());
        assert!(!parsed.frontmatter.disable_model_invocation);
        assert_eq!(parsed.body, content);
    }

    #[test]
    fn empty_frontmatter() {
        let content = "\
---
---

Body after empty frontmatter.
";
        let parsed = parse(content);
        assert!(parsed.frontmatter.name.is_none());
        assert!(parsed.frontmatter.description.is_none());
        assert_eq!(parsed.body.trim(), "Body after empty frontmatter.");
    }

    #[test]
    fn missing_fields_use_defaults() {
        let content = "\
---
name: partial
---

Some body.
";
        let parsed = parse(content);
        assert_eq!(parsed.frontmatter.name.as_deref(), Some("partial"));
        assert!(parsed.frontmatter.description.is_none());
        assert!(!parsed.frontmatter.disable_model_invocation);
    }

    #[test]
    fn unknown_fields_ignored() {
        let content = "\
---
name: my-skill
future-field: some value
another: 42
---

Body.
";
        let parsed = parse(content);
        assert_eq!(parsed.frontmatter.name.as_deref(), Some("my-skill"));
    }

    #[test]
    fn body_with_triple_dashes() {
        let content = "\
---
name: test
---

Some body.

---

More body after a horizontal rule.
";
        let parsed = parse(content);
        assert_eq!(parsed.frontmatter.name.as_deref(), Some("test"));
        assert!(parsed.body.contains("Some body."));
        assert!(parsed.body.contains("More body after a horizontal rule."));
    }
}
