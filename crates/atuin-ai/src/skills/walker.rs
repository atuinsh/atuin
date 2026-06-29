//! Filesystem discovery for `SKILL.md` files.
//!
//! Recursively scans `.atuin/skills/` directories at the project and global
//! levels. Supports nested directories for organization (e.g.
//! `.atuin/skills/ops/deploy/SKILL.md`).

use std::path::{Path, PathBuf};

const SKILL_FILENAME: &str = "SKILL.md";

/// A skill file found on disk, before body interpolation.
#[derive(Debug)]
pub(crate) struct RawSkillFile {
    /// Full path to the SKILL.md file.
    pub path: PathBuf,
    /// The parent directory name, used as fallback skill name.
    pub dir_name: String,
    /// Whether this is a project-level skill (vs global).
    #[allow(dead_code)]
    pub is_project: bool,
    /// Raw file content.
    pub content: String,
}

/// Discover all `SKILL.md` files across project and global skill directories.
///
/// Project skills come first in the returned list (higher priority for
/// deduplication).
pub(crate) async fn discover(
    project_skills_dir: Option<&Path>,
    global_skills_dir: &Path,
) -> Vec<RawSkillFile> {
    let mut files = Vec::new();

    // Project skills first (higher priority)
    if let Some(dir) = project_skills_dir.filter(|d| d.is_dir()) {
        scan_dir(dir, true, &mut files).await;
    }

    // Global skills second
    if global_skills_dir.is_dir() {
        scan_dir(global_skills_dir, false, &mut files).await;
    }

    files
}

/// The default global skills directory (`~/.config/atuin/skills/`).
pub(crate) fn global_skills_dir() -> PathBuf {
    atuin_common::utils::config_dir().join("skills")
}

/// Given a project working directory, return the project skills directory.
pub(crate) fn project_skills_dir(project_root: &Path) -> PathBuf {
    project_root.join(".atuin").join("skills")
}

/// Recursively scan a directory for `SKILL.md` files.
async fn scan_dir(dir: &Path, is_project: bool, out: &mut Vec<RawSkillFile>) {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::debug!("Could not read skills directory {}: {e}", dir.display());
            return;
        }
    };

    let mut subdirs = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();

        if path.is_dir() {
            // Check for SKILL.md directly in this directory
            let skill_path = path.join(SKILL_FILENAME);
            if skill_path.is_file() {
                let dir_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                match tokio::fs::read_to_string(&skill_path).await {
                    Ok(content) => {
                        out.push(RawSkillFile {
                            path: skill_path,
                            dir_name,
                            is_project,
                            content,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Failed to read skill file {}: {e}", skill_path.display());
                    }
                }
            }

            // Collect subdirectories for recursive scanning
            subdirs.push(path);
        }
    }

    for subdir in subdirs {
        Box::pin(scan_dir(&subdir, is_project, out)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_skill(dir: &Path, rel_path: &str, content: &str) {
        let skill_dir = dir.join(rel_path);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join(SKILL_FILENAME), content).unwrap();
    }

    #[tokio::test]
    async fn discovers_project_skills() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        setup_skill(&skills_dir, "deploy", "---\nname: deploy\n---\nDeploy.");

        let files = discover(Some(&skills_dir), Path::new("/nonexistent")).await;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].dir_name, "deploy");
        assert!(files[0].is_project);
    }

    #[tokio::test]
    async fn discovers_global_skills() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        setup_skill(&skills_dir, "review", "---\nname: review\n---\nReview.");

        let files = discover(None, &skills_dir).await;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].dir_name, "review");
        assert!(!files[0].is_project);
    }

    #[tokio::test]
    async fn discovers_nested_skills() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        setup_skill(&skills_dir, "ops/deploy", "---\nname: deploy\n---\n");
        setup_skill(&skills_dir, "ops/rollback", "---\nname: rollback\n---\n");

        let files = discover(Some(&skills_dir), Path::new("/nonexistent")).await;
        assert_eq!(files.len(), 2);
    }

    #[tokio::test]
    async fn project_comes_before_global() {
        let project = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();
        let project_skills = project.path().join("skills");
        let global_skills = global.path().join("skills");

        setup_skill(&project_skills, "a-skill", "project");
        setup_skill(&global_skills, "b-skill", "global");

        let files = discover(Some(&project_skills), &global_skills).await;
        assert_eq!(files.len(), 2);
        assert!(files[0].is_project);
        assert!(!files[1].is_project);
    }

    #[tokio::test]
    async fn missing_directories_handled() {
        let files = discover(
            Some(Path::new("/does/not/exist")),
            Path::new("/also/missing"),
        )
        .await;
        assert!(files.is_empty());
    }
}
