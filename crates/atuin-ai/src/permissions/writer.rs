use std::path::{Path, PathBuf};

use atuin_common::path::PathExt as _;
use eyre::Result;

use crate::permissions::rule::Rule;

/// Whether a rule should be added to the allow or deny list.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum RuleDisposition {
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
pub async fn write_rule(file_path: &Path, rule: &Rule, disposition: RuleDisposition) -> Result<()> {
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

/// Controls the scope of "always allow" for a given directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectScope {
    /// The Git repository the directory is in.
    Workspace,
    /// The directory itself.
    Directory,
    /// The directory is `$HOME`, so project-level scope is unavailable, as it would conflict with
    /// `~/.atuin` being used for global permissions.
    Home,
}

impl ProjectScope {
    /// The scope for the current directory.
    ///
    /// `git_root` is the root of the Git repository the current directory is in, if any.
    pub fn detect(git_root: Option<&Path>) -> Self {
        Self::detect_with(git_root, || std::env::current_dir().ok())
    }

    fn detect_with(git_root: Option<&Path>, get_cwd: impl FnOnce() -> Option<PathBuf>) -> Self {
        let cwd;
        let root = if let Some(root) = git_root {
            Some(root)
        } else {
            cwd = get_cwd();
            cwd.as_deref()
        };

        if let Some(root) = root
            && atuin_common::dirs::try_home_dir().is_ok_and(|home| root.is_same_path(home))
        {
            return Self::Home;
        }

        if git_root.is_some() {
            Self::Workspace
        } else {
            Self::Directory
        }
    }
}

/// Build the path to the project-level permissions file.
/// `project_root` is typically a git root or the current working directory.
pub fn project_permissions_path(project_root: &Path) -> PathBuf {
    PathBuf::from_iter([project_root, ".atuin".as_ref(), "permissions.ai.toml".as_ref()])
}

/// Build the path to the global permissions file (sibling of atuin config).
pub fn global_permissions_path() -> PathBuf {
    atuin_common::dirs::config_path("permissions.ai.toml")
}

#[cfg(test)]
mod tests {
    use atuin_common::env::MockEnv;
    use rstest::*;

    use super::*;

    /// A mock environment, in which every variable (including `HOME`) starts out unset.
    #[fixture]
    fn env() -> MockEnv {
        MockEnv::install()
    }

    #[rstest]
    #[case::git_repository(
        Some("/home/me/src/atuin"),
        "/home/me/src/atuin/crates",
        ProjectScope::Workspace
    )]
    #[case::plain_directory(None, "/home/me/notes", ProjectScope::Directory)]
    #[case::outside_home(None, "/tmp", ProjectScope::Directory)]
    #[case::home(None, "/home/me", ProjectScope::Home)]
    // Dotfiles kept in a git repository rooted at the home directory.
    #[case::home_as_git_repository(Some("/home/me"), "/home/me/.config", ProjectScope::Home)]
    #[case::subdirectory_of_home(None, "/home/me/.config", ProjectScope::Directory)]
    fn project_scope_is_never_the_home_directory(
        env: MockEnv,
        #[case] git_root: Option<&str>,
        #[case] cwd: &str,
        #[case] expected: ProjectScope,
    ) {
        env.set("HOME", "/home/me");
        let scope = ProjectScope::detect_with(git_root.map(Path::new), || {
            assert!(git_root.is_none(), "the working directory isn't needed in a git repository");
            Some(PathBuf::from(cwd))
        });
        assert_eq!(scope, expected);
    }

    /// Without a home directory or a working directory to compare, the project is just a
    /// directory.
    #[rstest]
    fn project_scope_without_a_known_home_or_cwd(env: MockEnv) {
        assert_eq!(
            ProjectScope::detect_with(None, || Some(PathBuf::from("/home/me"))),
            ProjectScope::Directory
        );
        env.set("HOME", "/home/me");
        assert_eq!(ProjectScope::detect_with(None, || None), ProjectScope::Directory);
    }

    /// A home directory reached through a symlink is still the home directory.
    #[cfg(unix)]
    #[rstest]
    fn project_scope_sees_through_symlinks(env: MockEnv) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let link = dir.path().join("link");
        std::fs::create_dir(&home).unwrap();
        std::os::unix::fs::symlink(&home, &link).unwrap();
        env.set("HOME", &home);
        assert_eq!(ProjectScope::detect_with(None, || Some(link)), ProjectScope::Home);
    }

    #[fixture]
    fn perm_file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("permissions.ai.toml");
        (dir, file)
    }

    #[rstest]
    #[case::create_allow(None, "AtuinHistory", RuleDisposition::Allow, &["[permissions]", r#""AtuinHistory""#])]
    #[case::append_existing(
        Some("# My permissions\n[permissions]\nallow = [\"Read\"]\n"),
        "AtuinHistory",
        RuleDisposition::Allow,
        &["# My permissions", r#""Read""#, r#""AtuinHistory""#]
    )]
    #[case::inline_table(
        Some("permissions = { allow = [\"Read\"] }\n"),
        "AtuinHistory",
        RuleDisposition::Allow,
        &[r#""Read""#, r#""AtuinHistory""#]
    )]
    #[case::deny(None, "Shell", RuleDisposition::Deny, &["deny", r#""Shell""#])]
    #[tokio::test]
    async fn writes_expected_content(
        #[from(perm_file)] (_dir, file): (tempfile::TempDir, PathBuf),
        #[case] initial: Option<&str>,
        #[case] tool: &str,
        #[case] disposition: RuleDisposition,
        #[case] expected: &[&str],
    ) {
        if let Some(c) = initial {
            tokio::fs::write(&file, c).await.unwrap();
        }

        let rule = Rule {
            tool: tool.to_string(),
            scope: None,
        };
        write_rule(&file, &rule, disposition).await.unwrap();

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        for s in expected {
            assert!(content.contains(s), "missing {s} in:\n{content}");
        }
    }

    #[rstest]
    #[tokio::test]
    async fn does_not_duplicate_existing_rule(
        #[from(perm_file)] (_dir, file): (tempfile::TempDir, PathBuf),
    ) {
        let existing = r#"[permissions]
allow = ["AtuinHistory"]
"#;
        tokio::fs::write(&file, existing).await.unwrap();

        let rule = Rule {
            tool: "AtuinHistory".to_string(),
            scope: None,
        };
        write_rule(&file, &rule, RuleDisposition::Allow).await.unwrap();

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        // Should appear exactly once
        assert_eq!(content.matches("AtuinHistory").count(), 1);
    }
}
