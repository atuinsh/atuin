//! Config files written in Luau: `atuin.lua` or `.atuin.lua` in a project's directory, and in
//! `$ATUIN_HOME` for the user's own.
//!
//! A file starts with the version of the config it's written for, as `-- atuin:version=1`.
//!
//! A file sets its config, as `atuin.config = { orgs = { "acme" } }`, which [`Config`] reads and
//! [`TYPES`] declares for an editor. A key it doesn't know fails the file, as a typo may be what
//! keeps a project's threads to itself.
//!
//! A project's file comes with whatever repository the user cloned, and runs with no prompt, so
//! each file runs in a Luau VM of its own that can build a table and nothing else: no files, no
//! clock, no `require`, a few megabytes and half a second.

mod header;
mod orgs;
mod vm;

use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use self::header::Header;
pub use self::header::HeaderError;
pub use self::orgs::{OrgSlug, OrgSlugError, Orgs, TooManyOrgs};
use self::vm::Vm;

/// The names a directory's config file may have, of which it may hold one.
const FILE_NAMES: [&str; 2] = ["atuin.lua", ".atuin.lua"];

/// The longest file read: a config is a few tables, and the file is read whole before it runs.
const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

/// The Luau types of a config file, for an editor's language server: `atuin config types` prints
/// them.
pub const TYPES: &str = include_str!("atuin.d.luau");

/// The configuration a user can set.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The organizations that own the project, with whose members the threads started at or under
    /// the file's directory are shared.
    ///
    /// If a user is not a member of an org, atuin will skip uploading anything.
    pub orgs: Option<Orgs>,
}

impl Config {
    /// The source of a config file that sets this config.
    fn to_source(&self) -> String {
        let Self { orgs } = self;
        let orgs = orgs.as_ref().map_or_else(String::new, |orgs| {
            // A slug holds only letters, digits, `-` and `_`, so it needs no escaping in a string.
            let slugs: Vec<String> = orgs.iter().map(|org| format!("\"{org}\"")).collect();
            let list = if slugs.is_empty() {
                "{}".to_owned()
            } else {
                format!("{{ {} }}", slugs.join(", "))
            };
            format!("  -- Organizations that own this project.\n  orgs = {list},\n")
        });
        format!("-- atuin:version=1\n\natuin.config = {{\n{orgs}}}\n")
    }
}

/// A config file and the config it sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFile {
    path: PathBuf,
    config: Config,
}

impl ConfigFile {
    /// Writes `config` to a new `.atuin.lua` in `dir`, which mustn't have a config file yet.
    ///
    /// **Blocks** on the disk, as [`ConfigFile::in_dir`] does.
    pub fn create(dir: &Path, config: Config) -> Result<Self, CreateError> {
        if let Some(existing) = Source::in_dir(dir)? {
            return Err(CreateError::Exists {
                path: existing.path,
            });
        }

        let [_, dotted] = FILE_NAMES;
        let path = dir.join(dotted);
        let opened = OpenOptions::new().write(true).create_new(true).open(&path);
        let mut file = match opened {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                return Err(CreateError::Exists { path });
            }
            Err(source) => return Err(CreateError::Write { path, source }),
        };
        if let Err(source) = file.write_all(config.to_source().as_bytes()) {
            return Err(CreateError::Write { path, source });
        }
        Ok(Self { path, config })
    }

    /// The config file in `dir`, if it has one: the user's own when `dir` is `$ATUIN_HOME`.
    ///
    /// **Blocks** on the disk, for as long as a network mount under `dir` hangs, and runs the file,
    /// so call it off the async runtime.
    pub fn in_dir(dir: &Path) -> Result<Option<Self>, LoadError> {
        let Some(source) = Source::in_dir(dir)? else {
            return Ok(None);
        };
        Vm::load(source.into_script()?)?.eval().map(Some)
    }

    /// The config files of the projects at and above `dir`, the nearest first, each run as it's
    /// reached. Blocks as [`ConfigFile::in_dir`] does.
    pub fn projects(dir: &Path) -> impl Iterator<Item = Result<Self, LoadError>> {
        dir.ancestors().filter_map(|ancestor| Self::in_dir(ancestor).transpose())
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    #[must_use]
    pub fn into_config(self) -> Config {
        self.config
    }
}

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("{} has both atuin.lua and .atuin.lua; remove one", dir.display())]
    BothFiles {
        dir: PathBuf,
    },
    #[error("failed to read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} is over {MAX_SOURCE_BYTES} bytes", path.display())]
    TooLarge {
        path: PathBuf,
    },
    #[error("{} has an invalid header; it needs `-- atuin:version=1`", path.display())]
    Header {
        path: PathBuf,
        #[source]
        source: HeaderError,
    },
    #[error("failed to run {}", path.display())]
    Run {
        path: PathBuf,
        #[source]
        source: mlua::Error,
    },
    #[error("{} doesn't set a valid `atuin.config`", path.display())]
    Invalid {
        path: PathBuf,
        #[source]
        source: mlua::Error,
    },
}

#[derive(Debug, Error)]
pub enum CreateError {
    #[error("{} already exists", path.display())]
    Exists {
        path: PathBuf,
    },
    #[error(transparent)]
    Lookup(#[from] LoadError),
    #[error("failed to write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl LoadError {
    /// The file at fault, or the directory holding both.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::BothFiles { dir: path }
            | Self::Read { path, .. }
            | Self::TooLarge { path }
            | Self::Header { path, .. }
            | Self::Run { path, .. }
            | Self::Invalid { path, .. } => path,
        }
    }
}

/// A config file, read whole.
struct Source {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl Source {
    /// The config file in `dir`, under either of [`FILE_NAMES`], if it has one.
    fn in_dir(dir: &Path) -> Result<Option<Self>, LoadError> {
        let [plain, dotted] = FILE_NAMES.map(|name| dir.join(name));
        match (Self::read(plain)?, Self::read(dotted)?) {
            (Some(_), Some(_)) => Err(LoadError::BothFiles {
                dir: dir.to_path_buf(),
            }),
            (found, None) | (None, found) => Ok(found),
        }
    }

    /// The file at `path`, or `None` when there's none.
    fn read(path: PathBuf) -> Result<Option<Self>, LoadError> {
        let opened = match File::open(&path) {
            Ok(file) => file,
            Err(err)
                if matches!(err.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) =>
            {
                return Ok(None);
            }
            Err(source) => return Err(LoadError::Read { path, source }),
        };

        let mut bytes = Vec::new();
        // One byte past the limit, to tell a file at it from one over it.
        if let Err(source) = opened.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes) {
            return Err(LoadError::Read { path, source });
        }
        if bytes.len() as u64 > MAX_SOURCE_BYTES {
            return Err(LoadError::TooLarge { path });
        }
        Ok(Some(Self { path, bytes }))
    }

    /// The script this file is, once its header names a version this atuin reads.
    fn into_script(self) -> Result<Script, LoadError> {
        match header::parse(&self.bytes) {
            Ok(header) => Ok(Script {
                path: self.path,
                bytes: self.bytes,
                header,
            }),
            Err(source) => Err(LoadError::Header {
                path: self.path,
                source,
            }),
        }
    }
}

/// A config file whose header names a version this atuin reads, so it may run.
struct Script {
    path: PathBuf,
    bytes: Vec<u8>,
    header: Header,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use proptest::prelude::*;
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    /// The header of the tests whose subject is the rest of the file.
    const V1: &str = "-- atuin:version=1\n";

    const ACME: &str = r#"-- atuin:version=1
atuin.config = { orgs = { "acme" } }"#;

    #[fixture]
    fn root() -> TempDir {
        tempfile::tempdir().expect("a temp dir")
    }

    fn write(root: &Path, file: &str, source: &str) -> PathBuf {
        let path = root.join(file);
        fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        fs::write(&path, source).expect("write");
        path
    }

    fn orgs(body: &str) -> Result<Option<Vec<String>>, LoadError> {
        let root = root();
        write(root.path(), "atuin.lua", &format!("{V1}{body}"));
        let file = ConfigFile::in_dir(root.path())?.expect("a config file");
        let orgs = file.into_config().orgs;
        Ok(orgs.map(|orgs| orgs.iter().map(ToString::to_string).collect()))
    }

    #[rstest]
    #[case::assigned(r#"atuin.config = { orgs = { "acme" } }"#, Some(&["acme"][..]))]
    #[case::assigned_field_by_field(
        r#"atuin.config = {} atuin.config.orgs = { "acme" }"#,
        Some(&["acme"][..])
    )]
    #[case::computed(
        r#"local orgs = {} for _, org in ipairs({ "acme", "globex" }) do table.insert(orgs, org) end atuin.config = { orgs = orgs }"#,
        Some(&["acme", "globex"][..])
    )]
    #[case::an_empty_list_is_set_to_none(r"atuin.config = { orgs = {} }", Some(&[][..]))]
    #[case::unset(r"atuin.config = {}", None)]
    fn a_file_sets_its_config(#[case] source: &str, #[case] expected: Option<&[&str]>) {
        let orgs = orgs(source).expect("a config");

        let orgs: Option<Vec<&str>> =
            orgs.as_ref().map(|orgs| orgs.iter().map(String::as_str).collect());
        assert_eq!(orgs.as_deref(), expected);
    }

    /// A typo could be what keeps a project's threads to itself, so it fails the file rather than
    /// leaving the setting to a file further up. Nor can a file switch on what only the user's own
    /// config.toml does, as uploading their threads at all.
    #[rstest]
    #[case::unknown_key(r#"atuin.config = { org = { "acme" } }"#)]
    #[case::cannot_turn_uploads_on(r"atuin.config = { enabled = true, upload_output = true }")]
    #[case::nested_under_sharing(r#"atuin.config = { sharing = { orgs = { "acme" } } }"#)]
    #[case::wrong_type(r#"atuin.config = { orgs = "acme" }"#)]
    #[case::not_a_table(r"atuin.config = 5")]
    #[case::sets_nothing(r"local x = 1")]
    #[case::misspells_config(r"atuin.confg = {}")] // codespell:ignore confg
    #[case::returns_its_config(r#"return { orgs = { "acme" } }"#)]
    #[case::not_a_slug(r#"atuin.config = { orgs = { "ac me" } }"#)]
    #[case::an_empty_slug(r#"atuin.config = { orgs = { "" } }"#)]
    fn a_file_that_says_something_else_is_invalid(#[case] source: &str) {
        assert!(matches!(orgs(source), Err(LoadError::Invalid { .. })), "{source}");
    }

    /// Lua shares one string between the entries that hold it, but Rust copies it into each, so
    /// atuin must stop before it copies far more than the VM's memory limit lets a file hold.
    #[rstest]
    #[case::a_long_string_repeated(
        r#"local s = string.rep("x", 1024 * 1024) local orgs = {} for i = 1, 64 do orgs[i] = s end atuin.config = { orgs = orgs }"#
    )]
    #[case::a_slug_repeated(
        r#"local orgs = {} for i = 1, 100000 do orgs[i] = "acme" end atuin.config = { orgs = orgs }"#
    )]
    fn a_file_cant_make_atuin_copy_more_than_the_vm_holds(#[case] source: &str) {
        assert!(matches!(orgs(source), Err(LoadError::Invalid { .. })), "{source}");
    }

    /// The header decides what a file runs against, so a file whose header doesn't name a version
    /// this atuin reads never runs.
    #[rstest]
    #[case::no_header("while true do end")]
    #[case::a_newer_version("-- atuin:version=2\nwhile true do end")]
    fn a_file_without_a_version_it_reads_is_not_run(root: TempDir, #[case] source: &str) {
        let path = write(root.path(), "atuin.lua", source);

        let err = ConfigFile::in_dir(root.path()).expect_err("a file that isn't run");

        assert!(matches!(err, LoadError::Header { .. }), "{err:?}");
        assert_eq!(err.path(), path);
    }

    #[rstest]
    #[case::atuin_lua("atuin.lua")]
    #[case::dot_atuin_lua(".atuin.lua")]
    fn either_name_is_a_config_file(root: TempDir, #[case] name: &str) {
        let path = write(root.path(), name, ACME);

        let file = ConfigFile::in_dir(root.path()).expect("a lookup").expect("a config file");

        assert_eq!(file.path(), path);
    }

    #[rstest]
    fn a_directory_with_both_names_fails(root: TempDir) {
        write(root.path(), "atuin.lua", ACME);
        write(root.path(), ".atuin.lua", ACME);

        let err = ConfigFile::in_dir(root.path()).expect_err("an ambiguous directory");

        assert!(matches!(err, LoadError::BothFiles { .. }));
        assert_eq!(err.path(), root.path());
    }

    #[rstest]
    fn a_directory_without_one_has_none(root: TempDir) {
        assert_eq!(ConfigFile::in_dir(root.path()).expect("a lookup"), None);
        assert_eq!(ConfigFile::in_dir(&root.path().join("missing")).expect("a lookup"), None);
    }

    #[rstest]
    fn a_file_over_the_limit_is_not_run(root: TempDir) {
        let path = write(
            root.path(),
            "atuin.lua",
            &"-".repeat(usize::try_from(MAX_SOURCE_BYTES).expect("fits") + 1),
        );

        let err = ConfigFile::in_dir(root.path()).expect_err("a file too large");

        assert!(matches!(err, LoadError::TooLarge { .. }));
        assert_eq!(err.path(), path);
    }

    #[rstest]
    fn projects_are_found_nearest_first(root: TempDir) {
        let outer = write(root.path(), "work/atuin.lua", ACME);
        let inner = write(root.path(), "work/sub/.atuin.lua", ACME);

        let found: Vec<_> = ConfigFile::projects(&root.path().join("work/sub/deep"))
            .map(|file| file.expect("a config file").path().to_path_buf())
            .collect();

        assert_eq!(found, [inner, outer]);
    }

    /// The caller stops at the first file that settles what it's after, so one further up that's
    /// broken never fails it.
    #[rstest]
    fn projects_are_run_only_as_they_are_reached(root: TempDir) {
        write(root.path(), "work/atuin.lua", &format!("{V1}this is not lua"));
        let inner = write(root.path(), "work/sub/atuin.lua", ACME);

        let first = ConfigFile::projects(&root.path().join("work/sub")).next().expect("a file");

        assert_eq!(first.expect("the nearest file").path(), inner);
    }

    fn slug() -> impl Strategy<Value = OrgSlug> {
        "[A-Za-z0-9_]([A-Za-z0-9_-]{0,30}[A-Za-z0-9_])?"
            .prop_filter_map("a slug", |s| s.parse().ok())
    }

    fn config() -> impl Strategy<Value = Config> {
        proptest::option::of(proptest::collection::vec(slug(), 0..=Orgs::MAX)).prop_map(|orgs| {
            Config {
                orgs: orgs.map(|orgs| Orgs::try_from(orgs).expect("orgs within the limit")),
            }
        })
    }

    proptest! {
        /// `atuin workspace init` writes a file for the config the user gave, so atuin must read
        /// that same config back from it.
        #[test]
        fn a_created_file_sets_the_config_it_was_created_with(config in config()) {
            let root = root();

            let created = ConfigFile::create(root.path(), config).expect("a created file");
            let read = ConfigFile::in_dir(root.path()).expect("a lookup").expect("a config file");

            prop_assert_eq!(read, created);
        }
    }

    #[rstest]
    fn a_created_file_reads_as_one_written_by_hand(root: TempDir) {
        let orgs = ["acme", "globex"].map(|slug| slug.parse().expect("a slug"));
        let config = Config {
            orgs: Some(Orgs::try_from(orgs.to_vec()).expect("orgs")),
        };

        let file = ConfigFile::create(root.path(), config).expect("a created file");

        assert_eq!(file.path(), root.path().join(".atuin.lua"));
        assert_eq!(
            fs::read_to_string(file.path()).expect("the file"),
            r#"-- atuin:version=1

atuin.config = {
  -- Organizations that own this project.
  orgs = { "acme", "globex" },
}
"#
        );
    }

    #[rstest]
    #[case::atuin_lua("atuin.lua")]
    #[case::dot_atuin_lua(".atuin.lua")]
    fn a_directory_with_a_config_file_gets_no_second_one(root: TempDir, #[case] name: &str) {
        let path = write(root.path(), name, ACME);

        let err = ConfigFile::create(root.path(), Config::default()).expect_err("an existing file");

        assert!(
            matches!(&err, CreateError::Exists { path: existing } if *existing == path),
            "{err:?}"
        );
        assert_eq!(fs::read_to_string(&path).expect("the file"), ACME);
    }
}
