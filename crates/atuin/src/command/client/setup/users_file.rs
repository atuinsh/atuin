//! The user's own `atuin.lua`, in `$ATUIN_HOME`, which turns Atuin for Teams on: setup writes it
//! to say which threads go up, and leaves one they wrote be.

use std::path::{Path, PathBuf};
use std::{fs, io};

use atuin_client::lua_config::{LoadError, RuleName, Upload};
use thiserror::Error;

use super::plan::ThreadUploads;

/// The user's own config file, as setup reads it.
#[derive(Debug)]
pub(super) struct UsersFile {
    /// Where it is, or where setup would write it.
    path: PathBuf,
    says: Says,
}

/// What a [`UsersFile`] says of thread uploads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Says {
    /// There's no file.
    Nothing,
    /// The file setup wrote for these.
    Setup(ThreadUploads),
    /// One they wrote, which turns uploading on or doesn't.
    Theirs {
        on: bool,
    },
}

impl UsersFile {
    /// The one in `home`. **Blocks** on the disk and runs the file.
    pub(super) fn read(home: &Path) -> Result<Self, LoadError> {
        let Some(upload) = Upload::in_dir(home)? else {
            return Ok(Self {
                path: home.join("atuin.lua"),
                says: Says::Nothing,
            });
        };
        let path = upload.path().to_path_buf();
        let text = fs::read_to_string(&path).map_err(|source| LoadError::Read {
            path: path.clone(),
            source,
        })?;
        let wrote = ThreadUploads::EACH.into_iter().find(|&uploads| source(uploads) == text);
        let says = wrote.map_or_else(
            || Says::Theirs {
                on: upload.switches_on(RuleName::Threads),
            },
            Says::Setup,
        );
        Ok(Self { path, says })
    }

    /// Whether it turns Atuin for Teams on, and for which threads as setup offers them: every
    /// thread for one of theirs that's on, and for one that's off, so turning it on again does.
    pub(super) const fn uploads(&self) -> (ThreadUploads, bool) {
        match self.says {
            Says::Nothing | Says::Setup(ThreadUploads::Off) | Says::Theirs { on: false } => {
                (ThreadUploads::All, false)
            }
            Says::Setup(uploads) => (uploads, true),
            Says::Theirs { on: true } => (ThreadUploads::All, true),
        }
    }

    /// Makes it say `uploads`: a new file, or one over the file setup wrote. One of theirs is left
    /// be, as setup can't edit their code, so it says so already or `Err` says what to change: one
    /// that's on uploads every thread for all setup can tell.
    pub(super) fn write(&self, uploads: ThreadUploads) -> Result<(), WriteError> {
        match (self.says, uploads) {
            (Says::Theirs { on: true }, ThreadUploads::All)
            | (Says::Theirs { on: false } | Says::Nothing, ThreadUploads::Off) => Ok(()),
            (Says::Theirs { .. }, uploads) => Err(WriteError::Theirs {
                path: self.path.clone(),
                rule: rule(uploads),
            }),
            (Says::Nothing | Says::Setup(_), uploads) => self.replace(uploads),
        }
    }

    fn replace(&self, uploads: ThreadUploads) -> Result<(), WriteError> {
        let write = |source| WriteError::Write {
            path: self.path.clone(),
            source,
        };
        if let Some(home) = self.path.parent() {
            fs::create_dir_all(home).map_err(write)?;
        }
        fs::write(&self.path, source(uploads)).map_err(write)
    }
}

#[derive(Debug, Error)]
pub(super) enum WriteError {
    #[error(
        "{} is yours, so setup leaves it be: set `upload = {{ threads = {rule} }}` in its \
         `atuin.config`",
        path.display()
    )]
    Theirs {
        path: PathBuf,
        rule: &'static str,
    },
    #[error("couldn't write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// The rule for `uploads`, as one line of Lua.
const fn rule(uploads: ThreadUploads) -> &'static str {
    match uploads {
        ThreadUploads::All => "true",
        ThreadUploads::Workspaces => "function(thread) return thread.workspace ~= nil end",
        ThreadUploads::Off => "false",
    }
}

/// The file setup writes for `uploads`.
fn source(uploads: ThreadUploads) -> String {
    let rule = match uploads {
        ThreadUploads::All => {
            r"    -- Every agent thread goes up to Atuin for Teams.
    threads = true,
"
        }
        ThreadUploads::Workspaces => {
            r"    -- The agent threads started in a workspace, at or under a project's atuin.lua, go
    -- up to Atuin for Teams.
    threads = function(thread)
      return thread.workspace ~= nil
    end,
"
        }
        ThreadUploads::Off => {
            r"    -- Agent threads stay on this machine.
    threads = false,
"
        }
    };
    format!("-- atuin:version=1\n\natuin.config = {{\n  upload = {{\n{rule}  }},\n}}\n")
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    #[fixture]
    fn home() -> TempDir {
        tempfile::tempdir().expect("a temp dir")
    }

    fn read(home: &TempDir) -> UsersFile {
        UsersFile::read(home.path()).expect("a readable file")
    }

    /// Setup reads back each file it writes as what it wrote, and the config files read it as
    /// turning uploading on for it. One it wrote off offers every thread, for turning it on again.
    #[rstest]
    fn setup_reads_back_what_it_wrote(
        home: TempDir,
        #[values(ThreadUploads::All, ThreadUploads::Workspaces, ThreadUploads::Off)]
        uploads: ThreadUploads,
    ) {
        read(&home).replace(uploads).expect("a write");

        let on = !matches!(uploads, ThreadUploads::Off);
        let offered = if on {
            uploads
        } else {
            ThreadUploads::All
        };
        assert_eq!(read(&home).uploads(), (offered, on));
        let upload = Upload::in_dir(home.path()).expect("a run").expect("a file");
        assert_eq!(upload.switches_on(RuleName::Threads), on);
    }

    /// Setup can't edit their code, so a file of theirs that says otherwise is left as it was.
    #[rstest]
    #[case::turning_on("{ threads = false }", ThreadUploads::All, false)]
    #[case::turning_off("{ threads = true }", ThreadUploads::Off, false)]
    #[case::on_already("{ threads = function() return true end }", ThreadUploads::All, true)]
    #[case::narrowing_to_workspaces("{ threads = true }", ThreadUploads::Workspaces, false)]
    #[case::off_already("{}", ThreadUploads::Off, true)]
    fn a_file_of_theirs_is_left_be(
        home: TempDir,
        #[case] upload: &str,
        #[case] uploads: ThreadUploads,
        #[case] says_so: bool,
    ) {
        let theirs = format!("-- atuin:version=1\natuin.config = {{ upload = {upload} }}\n");
        let path = home.path().join(".atuin.lua");
        fs::write(&path, &theirs).expect("write");

        let wrote = read(&home).write(uploads);

        assert_eq!(wrote.is_ok(), says_so, "{wrote:?}");
        assert_eq!(fs::read_to_string(&path).expect("the file"), theirs);
    }

    #[rstest]
    #[case::on(ThreadUploads::Workspaces, true)]
    #[case::off(ThreadUploads::Off, false)]
    fn without_a_file_one_is_written_only_to_turn_it_on(
        home: TempDir,
        #[case] uploads: ThreadUploads,
        #[case] written: bool,
    ) {
        read(&home).write(uploads).expect("a write");

        assert_eq!(home.path().join("atuin.lua").exists(), written);
    }
}
