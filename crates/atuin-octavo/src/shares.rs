//! Which organizations a thread is shared with by configuration.
//!
//! A directory's are the `orgs` of the nearest config file at or above it (see
//! [`atuin_client::lua_config`]), else of the user's own. A share holds the whole thread, so a
//! thread's are those every directory it ran in has in common.

use std::path::{Path, PathBuf};
use std::time::Duration;

use atuin_client::lua_config::{ConfigFile, LoadError, OrgSlug};
use thiserror::Error;
use tokio::task::JoinError;

/// The config files that say who threads are shared with: each project's, and the user's own.
#[derive(Debug, Clone)]
pub struct Configs {
    /// Where the user's own is, if they have one.
    atuin_home: Option<PathBuf>,
}

impl Configs {
    #[must_use]
    pub const fn new(atuin_home: Option<PathBuf>) -> Self {
        Self { atuin_home }
    }

    /// The organizations `dir`'s config shares threads with.
    pub async fn orgs_of_dir(&self, dir: PathBuf) -> Result<Vec<OrgSlug>, LookupError> {
        self.off_runtime(move |configs| configs.nearest(&dir)).await
    }

    /// The organizations a thread that ran in each of `dirs` is shared with: none for one that
    /// names no directory.
    pub async fn orgs_of_thread(&self, dirs: Vec<PathBuf>) -> Result<Vec<OrgSlug>, LookupError> {
        self.off_runtime(move |configs| configs.common(&dirs)).await
    }

    /// Runs `look_up` on a blocking thread, as a hung network mount hangs it.
    async fn off_runtime(
        &self,
        look_up: impl FnOnce(&Self) -> Result<Vec<OrgSlug>, LoadError> + Send + 'static,
    ) -> Result<Vec<OrgSlug>, LookupError> {
        // A config file runs for half a second at most, so the rest is the disk.
        const TIMEOUT: Duration = Duration::from_secs(5);

        let configs = self.clone();
        let lookup = tokio::task::spawn_blocking(move || look_up(&configs));
        match tokio::time::timeout(TIMEOUT, lookup).await {
            Ok(Ok(orgs)) => Ok(orgs?),
            Ok(Err(err)) => Err(LookupError::Failed(err)),
            Err(_elapsed) => Err(LookupError::TimedOut),
        }
    }

    /// Those of the nearest project config at or above `dir` setting any, else the user's own.
    /// A config file that can't be read or run fails, as it may be what keeps the thread to itself.
    fn nearest(&self, dir: &Path) -> Result<Vec<OrgSlug>, LoadError> {
        let users = self.atuin_home.iter().filter_map(|home| ConfigFile::in_dir(home).transpose());
        for file in ConfigFile::projects(dir).chain(users) {
            if let Some(orgs) = file?.into_config().orgs {
                return Ok(orgs.to_vec());
            }
        }
        Ok(Vec::new())
    }

    /// Those [`Self::nearest`] gives every one of `dirs`.
    fn common(&self, dirs: &[PathBuf]) -> Result<Vec<OrgSlug>, LoadError> {
        let Some((first, rest)) = dirs.split_first() else {
            return Ok(Vec::new());
        };
        let mut orgs = self.nearest(first)?;
        for dir in rest {
            if orgs.is_empty() {
                break;
            }
            let theirs = self.nearest(dir)?;
            orgs.retain(|org| theirs.contains(org));
        }
        Ok(orgs)
    }
}

#[derive(Debug, Error)]
pub enum LookupError {
    #[error(transparent)]
    Config(#[from] LoadError),
    #[error("looking up a config file took too long")]
    TimedOut,
    #[error("looking up a config file failed")]
    Failed(#[source] JoinError),
}

#[cfg(test)]
mod tests {
    use std::fs;

    use atuin_common::path::PathBufExt as _;
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    const ACME: &str = r#"atuin.config = { orgs = { "acme" } }"#;
    const USERS: &str = r#"atuin.config = { orgs = { "user-org" } }"#;

    /// A temp dir holding the user's `$ATUIN_HOME` at `home`, and their projects at `projects`.
    #[fixture]
    fn root() -> TempDir {
        tempfile::tempdir().expect("a temp dir")
    }

    /// Writes a version 1 config file whose body is `body`.
    fn write(root: &Path, file: &str, body: &str) -> PathBuf {
        let path = root.join(file);
        fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        fs::write(&path, format!("-- atuin:version=1\n{body}")).expect("write");
        path
    }

    fn configs(root: &Path) -> Configs {
        Configs::new(Some(root.join("home")))
    }

    fn orgs_in(root: &Path, dir: &str) -> Result<Vec<String>, LoadError> {
        let orgs = configs(root).nearest(&root.join("projects").joined(dir))?;
        Ok(orgs.iter().map(ToString::to_string).collect())
    }

    /// The orgs of a thread that ran in each of `dirs` under the projects in `root`.
    fn shared_by(root: &Path, dirs: &[&str]) -> Result<Vec<String>, LoadError> {
        let dirs: Vec<_> = dirs.iter().map(|dir| root.join("projects").joined(dir)).collect();
        let orgs = configs(root).common(&dirs)?;
        Ok(orgs.iter().map(ToString::to_string).collect())
    }

    #[rstest]
    #[case::the_users_own(&[("home/atuin.lua", USERS)], "work", &["user-org"])]
    #[case::none_without_a_config(&[], "work", &[])]
    #[case::project_replaces_the_users(&[("home/atuin.lua", USERS), ("projects/work/atuin.lua", ACME)], "work", &["acme"])]
    #[case::found_from_a_subdirectory(&[("projects/work/.atuin.lua", ACME)], "work/src/deep", &["acme"])]
    #[case::empty_list_opts_out(
        &[("home/atuin.lua", USERS), ("projects/work/atuin.lua", r"atuin.config = { orgs = {} }")],
        "work",
        &[]
    )]
    #[case::nearest_wins(
        &[("projects/work/atuin.lua", ACME), ("projects/work/sub/atuin.lua", r#"atuin.config = { orgs = { "globex" } }"#)],
        "work/sub",
        &["globex"]
    )]
    #[case::without_orgs_looks_further_up(
        &[("projects/work/atuin.lua", ACME), ("projects/work/sub/atuin.lua", r"atuin.config = {}")],
        "work/sub",
        &["acme"]
    )]
    #[case::a_home_directory_is_a_project_too(&[("home/atuin.lua", USERS), ("projects/atuin.lua", ACME)], "work", &["acme"])]
    fn the_nearest_config_setting_orgs_decides(
        root: TempDir,
        #[case] configs: &[(&str, &str)],
        #[case] started_in: &str,
        #[case] expected: &[&str],
    ) {
        for (file, source) in configs {
            write(root.path(), file, source);
        }

        let orgs = orgs_in(root.path(), started_in).expect("the lookup");

        assert_eq!(orgs, expected);
    }

    /// A broken file could be an opt-out with a typo, so the thread must not go to the user's list.
    #[rstest]
    fn a_broken_project_config_fails_rather_than_falling_through(root: TempDir) {
        write(root.path(), "home/atuin.lua", USERS);
        let broken = write(root.path(), "projects/work/atuin.lua", r"atuin.config = { org = {} }");

        let err = orgs_in(root.path(), "work").expect_err("a broken config");

        assert_eq!(err.path(), broken);
    }

    /// The user's own file is run only when no project's settles it, so a broken one fails only
    /// the threads that fall to it.
    #[rstest]
    fn the_users_config_is_run_only_when_reached(root: TempDir) {
        let broken = write(root.path(), "home/atuin.lua", "this is not lua");
        write(root.path(), "projects/work/atuin.lua", ACME);

        assert_eq!(orgs_in(root.path(), "work").expect("the lookup"), ["acme"]);
        assert_eq!(orgs_in(root.path(), "elsewhere").expect_err("a broken config").path(), broken);
    }

    /// A share holds the whole thread, so each directory it ran in must share it: a fork copies
    /// its parent's rows, directories and all, and a private project's must not reach the
    /// parent's organizations.
    #[rstest]
    #[case::one_directory(&["work"], &["acme"])]
    #[case::one_under_another(&["work", "work/src"], &["acme"])]
    #[case::only_the_orgs_they_have_in_common(&["both", "work"], &["acme"])]
    #[case::a_private_project_keeps_it_unshared(&["work", "private"], &[])]
    #[case::whichever_came_first(&["private", "work"], &[])]
    #[case::one_the_users_config_covers(&["work", "elsewhere"], &[])]
    #[case::a_slug_whatever_its_case(&["work", "shouty"], &["acme"])]
    #[case::no_directory(&[], &[])]
    fn a_thread_is_shared_with_the_orgs_of_every_directory_it_ran_in(
        root: TempDir,
        #[case] dirs: &[&str],
        #[case] expected: &[&str],
    ) {
        write(root.path(), "home/atuin.lua", USERS);
        write(root.path(), "projects/work/atuin.lua", ACME);
        write(
            root.path(),
            "projects/both/atuin.lua",
            r#"atuin.config = { orgs = { "globex", "acme" } }"#,
        );
        write(root.path(), "projects/private/atuin.lua", r"atuin.config = { orgs = {} }");
        write(root.path(), "projects/shouty/atuin.lua", r#"atuin.config = { orgs = { "ACME" } }"#);

        assert_eq!(shared_by(root.path(), dirs).expect("the lookup"), expected);
    }

    /// Once no org is left, the rest can't add one, so their files needn't run.
    #[rstest]
    fn a_broken_config_fails_the_thread_unless_no_org_is_left_by_then(root: TempDir) {
        write(root.path(), "projects/work/atuin.lua", ACME);
        write(root.path(), "projects/private/atuin.lua", r"atuin.config = { orgs = {} }");
        let broken = write(root.path(), "projects/broken/atuin.lua", "this is not lua");

        let err = shared_by(root.path(), &["work", "broken"]).expect_err("a broken config");

        assert_eq!(err.path(), broken);
        assert!(shared_by(root.path(), &["private", "broken"]).expect("the lookup").is_empty());
    }
}
