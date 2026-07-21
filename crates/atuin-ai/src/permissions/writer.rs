use std::path::Path;

use eyre::Result;

use crate::permissions::rule::Rule;

/// Whether a rule should be added to the allow or deny list.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum RuleDisposition {
    Allow,
    Deny,
}

/// Write a permission rule to a `permissions.ai.toml` file.
///
/// If the file doesn't exist it is created (along with parent directories).
/// If it does exist, `toml_edit` is used to append the rule while preserving
/// existing formatting and comments.
///
/// **Not concurrent-safe.** The read-modify-write cycle is not atomic. In the
/// current UI this is fine — the Select widget serializes permission decisions —
/// but callers should not invoke this concurrently for the same file.
pub(crate) async fn write_rule(
    file_path: &Path,
    rule: &Rule,
    disposition: RuleDisposition,
) -> Result<()> {
    let content = if tokio::fs::try_exists(file_path).await.unwrap_or(false) {
        tokio::fs::read_to_string(file_path).await?
    } else {
        String::new()
    };

    let mut doc: toml_edit::DocumentMut = content.parse()?;

    // Ensure [permissions] table exists
    if !doc.contains_key("permissions") {
        doc["permissions"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let key = match disposition {
        RuleDisposition::Allow => "allow",
        RuleDisposition::Deny => "deny",
    };

    // Use as_table_like_mut so both standard and inline tables work.
    let permissions = doc["permissions"]
        .as_table_like_mut()
        .ok_or_else(|| eyre::eyre!("[permissions] is not a table"))?;

    // Get or create the array
    if !permissions.contains_key(key) {
        permissions.insert(key, toml_edit::Item::Value(toml_edit::Array::new().into()));
    }

    let array = permissions
        .get_mut(key)
        .and_then(|item| item.as_value_mut())
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| eyre::eyre!("permissions.{key} is not an array"))?;

    // Don't add duplicates
    let rule_str = rule.to_string();
    let already_present = array.iter().any(|v| v.as_str() == Some(&rule_str));
    if !already_present {
        array.push(rule_str);
    }

    // Write back, creating parent directories as needed
    if let Some(parent) = file_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(file_path, doc.to_string()).await?;

    Ok(())
}

/// Build the path to the project-level permissions file.
/// `project_root` is typically a git root or the current working directory.
pub(crate) fn project_permissions_path(project_root: &Path) -> std::path::PathBuf {
    project_root.join(".atuin").join("permissions.ai.toml")
}

/// Build the path to the global permissions file (sibling of atuin config).
pub(crate) fn global_permissions_path() -> std::path::PathBuf {
    atuin_common::utils::config_dir().join("permissions.ai.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_new_file_with_allow_rule() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("permissions.ai.toml");
        let rule = Rule {
            tool: "AtuinHistory".to_string(),
            scope: None,
        };

        write_rule(&file, &rule, RuleDisposition::Allow)
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert!(content.contains("[permissions]"));
        assert!(content.contains(r#""AtuinHistory""#));
    }

    #[tokio::test]
    async fn appends_to_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("permissions.ai.toml");
        let existing = r#"# My permissions
[permissions]
allow = ["Read"]
"#;
        tokio::fs::write(&file, existing).await.unwrap();

        let rule = Rule {
            tool: "AtuinHistory".to_string(),
            scope: None,
        };
        write_rule(&file, &rule, RuleDisposition::Allow)
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        // Comment preserved
        assert!(content.contains("# My permissions"));
        // Both rules present
        assert!(content.contains(r#""Read""#));
        assert!(content.contains(r#""AtuinHistory""#));
    }

    #[tokio::test]
    async fn does_not_duplicate_existing_rule() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("permissions.ai.toml");
        let existing = r#"[permissions]
allow = ["AtuinHistory"]
"#;
        tokio::fs::write(&file, existing).await.unwrap();

        let rule = Rule {
            tool: "AtuinHistory".to_string(),
            scope: None,
        };
        write_rule(&file, &rule, RuleDisposition::Allow)
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        // Should appear exactly once
        assert_eq!(content.matches("AtuinHistory").count(), 1);
    }

    #[tokio::test]
    async fn handles_inline_table_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("permissions.ai.toml");
        // Inline table style — as_table_mut() would return None for this
        let existing = r#"permissions = { allow = ["Read"] }
"#;
        tokio::fs::write(&file, existing).await.unwrap();

        let rule = Rule {
            tool: "AtuinHistory".to_string(),
            scope: None,
        };
        write_rule(&file, &rule, RuleDisposition::Allow)
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert!(content.contains(r#""Read""#));
        assert!(content.contains(r#""AtuinHistory""#));
    }

    #[tokio::test]
    async fn writes_deny_rule() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("permissions.ai.toml");
        let rule = Rule {
            tool: "Shell".to_string(),
            scope: None,
        };

        write_rule(&file, &rule, RuleDisposition::Deny)
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert!(content.contains("deny"));
        assert!(content.contains(r#""Shell""#));
    }
}
