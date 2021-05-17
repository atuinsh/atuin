// Stub definitions to make things *compile*.

use std::path::PathBuf;

use BaseDirs;
use UserDirs;
use ProjectDirs;

pub fn base_dirs() -> Option<BaseDirs> { None }
pub fn user_dirs() -> Option<UserDirs> { None }
pub fn project_dirs_from_path(project_path: PathBuf) -> Option<ProjectDirs> { None }
pub fn project_dirs_from(qualifier: &str, organization: &str, application: &str) -> Option<ProjectDirs> { None }
