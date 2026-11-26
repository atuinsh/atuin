//! Install atuin's plugin, built from `integrations/ai-harness`, in the harnesses it has a build
//! for.
//!
//! The binary embeds the builds and hands them over as a [`Plugin`]. Installing writes a harness's
//! build to a directory of atuin's own and registers that directory with the harness:
//!
//! * Claude Code and Codex install a plugin from a marketplace, so their directory is one: a
//!   `.claude-plugin/marketplace.json`, which both read, beside the build in `atuin/`. Claude Code
//!   loads the build from there; Codex copies it into its own cache, which registering again
//!   refreshes.
//! * pi loads the directory in place once `pi install` has added it to its settings.
//! * opencode's `plugin add` takes only npm and Git packages, so atuin adds the directory to the
//!   `plugins` of `opencode.json` itself.
//!
//! A harness can have several configurations, as a second `CODEX_HOME` or `PI_CODING_AGENT_DIR`
//! is, and each gets a directory of its own (`Own`), so that taking the plugin out of one leaves
//! the others theirs. That directory is the last thing a removal deletes, which makes it the
//! record that the plugin is installed: a removal cut short is found, and finished, by the next.
//!
//! The plugin runs the atuin its `atuinPath` option names. Atuin sets it to itself wherever it can
//! read it back, and remembers what it set beside the build, so that a path set by hand, as to a
//! dev build, is told from its own and kept. Claude Code's is only written, through its CLI, so
//! it is set on every install.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use enum_dispatch::enum_dispatch;
use serde_json::{Value, json};
use toml_edit::DocumentMut;

use super::ccode::{self, Ccode};
use super::cli::{self, CliError};
use super::codex::{self, Codex};
use super::opencode::{self, Opencode};
use super::pi::{Flavour, Pi};
use super::{AnyHarness, Harness};
use crate::atuin_exe::{self, LaunchPathError};
use crate::dirs::{HomeError, try_atuin_home};
use crate::executable::resolve_in_path;
use crate::path::{AbsPathBuf, PathBufExt as _, PathExt as _};

/// The plugin's option for the atuin it runs.
const ATUIN_PATH: &str = "atuinPath";

/// The plugin's name, from `integrations/ai-harness/package.json`.
const NAME: &str = "atuin";
/// The marketplace atuin's own directory is to Claude Code and Codex.
const MARKETPLACE: &str = "atuin-local";
/// The plugin as Claude Code and Codex name it: [`NAME`] at [`MARKETPLACE`].
const SELECTOR: &str = "atuin@atuin-local";
/// The opencode whose plugin API the build is for.
const OPENCODE_MAJOR: u64 = 2;

/// One build of the plugin: each file's `/`-separated path under the build's root, and what it
/// holds.
#[derive(Clone, Copy, derive_more::Debug)]
#[debug("PluginFiles({} files)", _0.len())]
pub struct PluginFiles(pub &'static [(&'static str, &'static [u8])]);

/// The plugin as built for each harness that loads it.
#[derive(Debug, Clone, Copy)]
pub struct Plugin {
    pub claude_code: PluginFiles,
    pub codex: PluginFiles,
    pub opencode: PluginFiles,
    pub pi: PluginFiles,
}

impl Plugin {
    /// The directory atuin keeps every build it installs under:
    /// `<atuin home>/integrations/ai-harness`.
    pub fn dir() -> Result<PathBuf, HomeError> {
        Ok(PathBuf::from(try_atuin_home()?).joined("integrations").joined("ai-harness"))
    }
}

/// Error returned when installing or removing atuin's plugin fails.
#[derive(Debug, thiserror::Error)]
pub enum InstallPluginError {
    #[error(transparent)]
    Home(#[from] HomeError),

    #[error(transparent)]
    Executable(#[from] LaunchPathError),

    #[error(transparent)]
    Cli(#[from] CliError),

    #[error("{} is not valid UTF-8", .0.display())]
    NonUtf8Path(PathBuf),

    #[error("could not write the plugin to {}: {cause}", .dir.display())]
    Write {
        dir: PathBuf,
        cause: std::io::Error,
    },

    #[error("could not remove the plugin from {}: {cause}", .dir.display())]
    Remove {
        dir: PathBuf,
        cause: std::io::Error,
    },

    #[error("could not read {}: {cause}", .path.display())]
    Read {
        path: PathBuf,
        cause: std::io::Error,
    },

    #[error("could not parse {} as JSON: {cause}", .path.display())]
    Json {
        path: PathBuf,
        cause: serde_json::Error,
    },

    #[error("could not parse {} as TOML: {cause}", .path.display())]
    Toml {
        path: PathBuf,
        cause: toml_edit::TomlError,
    },

    #[error("{} has an unexpected shape: {wanted}", .path.display())]
    Malformed {
        path: PathBuf,
        wanted: &'static str,
    },

    #[error("could not save {}: {cause}", .path.display())]
    Save {
        path: PathBuf,
        cause: std::io::Error,
    },

    /// `found` is opencode's major version, `None` when `opencode --version` doesn't tell.
    #[error(
        "the plugin needs opencode {OPENCODE_MAJOR} or newer, and this is {}",
        .found.map_or_else(|| "one whose version can't be told".to_owned(), |major| format!("opencode {major}"))
    )]
    OpencodeTooOld {
        found: Option<u64>,
    },
}

/// Installs atuin's plugin in a harness, for the current user and the configuration of the harness
/// that the environment selects.
#[enum_dispatch]
pub trait InstallPlugin {
    /// Write this harness's build of `plugin` to atuin's own directory for the configuration and
    /// register it with the harness, returning where the build is.
    ///
    /// A build already there is replaced only when its contents differ, and registering again
    /// changes nothing.
    #[allow(async_fn_in_trait)]
    async fn install_plugin(&self, plugin: &Plugin) -> Result<PathBuf, InstallPluginError>;

    /// Whether atuin has installed the plugin for this configuration, and not yet wholly taken
    /// it out: its own directory for the configuration is there.
    fn has_plugin(&self) -> Result<bool, InstallPluginError>;

    /// Unregister the plugin and remove atuin's directory for the configuration. `false` when
    /// there was neither.
    #[allow(async_fn_in_trait)]
    async fn uninstall_plugin(&self) -> Result<bool, InstallPluginError>;
}

/// The harnesses atuin's plugin has a build for.
#[enum_dispatch(InstallPlugin)]
#[derive(Debug, Clone, Copy)]
pub enum PluginHarness {
    ClaudeCode(Ccode),
    Codex(Codex),
    Opencode(Opencode),
    Pi(Pi),
}

impl AnyHarness {
    /// This harness as one atuin's plugin has a build for, `None` for the rest.
    #[must_use]
    pub const fn with_plugin(self) -> Option<PluginHarness> {
        match self {
            Self::ClaudeCode(ccode) => Some(PluginHarness::ClaudeCode(ccode)),
            Self::Codex(codex) => Some(PluginHarness::Codex(codex)),
            Self::Opencode(opencode) => Some(PluginHarness::Opencode(opencode)),
            Self::Pi(pi) => match pi.flavour() {
                Flavour::Pi => Some(PluginHarness::Pi(pi)),
                // The build is for pi's extension API, and nobody has run it in omp.
                Flavour::Omp => None,
            },
            Self::Amp(_)
            | Self::Copilot(_)
            | Self::Grok(_)
            | Self::Hermes(_)
            | Self::Kiro(_)
            | Self::Vibe(_) => None,
        }
    }
}

impl Ccode {
    fn own(self) -> Result<Own, InstallPluginError> {
        Own::of(self.name(), &ccode::config_dir())
    }

    fn plugin_registered() -> Result<bool, InstallPluginError> {
        let Some((path, read)) = read_config(&ccode::config_dir().joined("settings.json"))? else {
            return Ok(false);
        };
        let root = parse_json(&path, &read)?;
        Ok(root.get("enabledPlugins").is_some_and(|on| on.get(SELECTOR).is_some()))
    }
}

impl InstallPlugin for Ccode {
    async fn install_plugin(&self, plugin: &Plugin) -> Result<PathBuf, InstallPluginError> {
        let dir = Tree::marketplace(plugin.claude_code).write_to(self.own()?.build())?;
        let atuin_path = format!("{ATUIN_PATH}={}", utf8(atuin_exe::launch_path().await?)?);
        let add = ["plugin", "marketplace", "add", utf8(&dir)?, "--scope", "user"];
        cli::run(self.program(), &add).await?;
        let install = ["plugin", "install", SELECTOR, "--scope", "user", "--config", &atuin_path];
        cli::run(self.program(), &install).await?;
        Ok(dir)
    }

    fn has_plugin(&self) -> Result<bool, InstallPluginError> {
        Ok(self.own()?.exists())
    }

    async fn uninstall_plugin(&self) -> Result<bool, InstallPluginError> {
        let (own, registered) = (self.own()?, Self::plugin_registered()?);
        if !registered && !own.exists() {
            return Ok(false);
        }
        if registered {
            let uninstall = ["plugin", "uninstall", SELECTOR, "--scope", "user"];
            cli::run(self.program(), &uninstall).await?;
        }
        let remove = ["plugin", "marketplace", "remove", MARKETPLACE];
        cli::run_ignoring_failure(self.program(), &remove).await;
        own.remove()?;
        Ok(true)
    }
}

impl Codex {
    fn own(self) -> Result<Own, InstallPluginError> {
        Own::of(self.name(), &codex::home())
    }

    fn plugin_registered() -> Result<bool, InstallPluginError> {
        let Some((path, read)) = read_config(&codex::home().joined("config.toml"))? else {
            return Ok(false);
        };
        let doc = String::from_utf8_lossy(&read).parse::<DocumentMut>();
        let doc = doc.map_err(|cause| InstallPluginError::Toml { path, cause })?;
        Ok(doc.get("plugins").is_some_and(|plugins| plugins.get(SELECTOR).is_some()))
    }
}

impl InstallPlugin for Codex {
    async fn install_plugin(&self, plugin: &Plugin) -> Result<PathBuf, InstallPluginError> {
        let own = self.own()?;
        let dir = Tree::marketplace(plugin.codex).write_to(own.build())?;
        cli::run(self.program(), &["plugin", "marketplace", "add", utf8(&dir)?]).await?;
        // Also what copies a changed build over the one in Codex's cache.
        cli::run(self.program(), &["plugin", "add", SELECTOR]).await?;
        let atuin = utf8(atuin_exe::launch_path().await?)?;
        own.set_modkit_atuin(&modkit_settings(codex::home()), atuin)?;
        Ok(dir)
    }

    fn has_plugin(&self) -> Result<bool, InstallPluginError> {
        Ok(self.own()?.exists())
    }

    async fn uninstall_plugin(&self) -> Result<bool, InstallPluginError> {
        let (own, registered) = (self.own()?, Self::plugin_registered()?);
        if !registered && !own.exists() {
            return Ok(false);
        }
        if registered {
            cli::run(self.program(), &["plugin", "remove", SELECTOR]).await?;
        }
        let remove = ["plugin", "marketplace", "remove", MARKETPLACE];
        cli::run_ignoring_failure(self.program(), &remove).await;
        unset_modkit_atuin(&modkit_settings(codex::home()), own.wrote_atuin().as_deref())?;
        own.remove()?;
        Ok(true)
    }
}

impl Opencode {
    fn own(self) -> Result<Own, InstallPluginError> {
        Own::of(self.name(), &opencode::config_dir())
    }
}

impl InstallPlugin for Opencode {
    async fn install_plugin(&self, plugin: &Plugin) -> Result<PathBuf, InstallPluginError> {
        let program =
            resolve_in_path(Path::new(self.program())).ok_or(CliError::NotFound(self.program()))?;
        match opencode::rehydrate::major_version(&program).await.ok() {
            Some(major) if major >= OPENCODE_MAJOR => {}
            found => return Err(InstallPluginError::OpencodeTooOld { found }),
        }
        let own = self.own()?;
        let dir = Tree::of(plugin.opencode).write_to(own.build())?;
        let atuin = utf8(atuin_exe::launch_path().await?)?;
        let wrote = own.wrote_atuin();
        if register_opencode(&opencode_config(), &dir, atuin, wrote.as_deref())? {
            own.record_atuin(atuin)?;
        }
        Ok(dir)
    }

    fn has_plugin(&self) -> Result<bool, InstallPluginError> {
        Ok(self.own()?.exists())
    }

    async fn uninstall_plugin(&self) -> Result<bool, InstallPluginError> {
        let own = self.own()?;
        let registered = unregister_opencode(&opencode_config(), &own.build())?;
        Ok(own.remove()? || registered)
    }
}

impl Pi {
    fn own(self) -> Result<Own, InstallPluginError> {
        Own::of(self.name(), &self.flavour().agent_dir())
    }

    /// pi keeps a local package as a path relative to its settings, either bare or as the
    /// `source` of an object that filters what the package loads.
    fn plugin_registered(self) -> Result<bool, InstallPluginError> {
        let (build, agent_dir) = (self.own()?.build(), self.flavour().agent_dir());
        let Some((path, read)) = read_config(&agent_dir.join("settings.json"))? else {
            return Ok(false);
        };
        let root = parse_json(&path, &read)?;
        let packages = root.get("packages").and_then(Value::as_array);
        Ok(packages.into_iter().flatten().any(|package| {
            let source = package.get("source").unwrap_or(package).as_str();
            source.is_some_and(|source| agent_dir.join(source).is_same_path(&build))
        }))
    }
}

impl InstallPlugin for Pi {
    async fn install_plugin(&self, plugin: &Plugin) -> Result<PathBuf, InstallPluginError> {
        let own = self.own()?;
        let dir = Tree::of(plugin.pi).write_to(own.build())?;
        cli::run(self.program(), &["install", utf8(&dir)?]).await?;
        let atuin = utf8(atuin_exe::launch_path().await?)?;
        own.set_modkit_atuin(&modkit_settings(self.flavour().agent_dir()), atuin)?;
        Ok(dir)
    }

    fn has_plugin(&self) -> Result<bool, InstallPluginError> {
        Ok(self.own()?.exists())
    }

    async fn uninstall_plugin(&self) -> Result<bool, InstallPluginError> {
        let (own, registered) = (self.own()?, self.plugin_registered()?);
        if registered {
            cli::run(self.program(), &["remove", utf8(&own.build())?]).await?;
        }
        let settings = modkit_settings(self.flavour().agent_dir());
        unset_modkit_atuin(&settings, own.wrote_atuin().as_deref())?;
        Ok(own.remove()? || registered)
    }
}

/// Atuin's own directory for one configuration of a harness:
/// `<atuin home>/integrations/ai-harness/<harness>/<configuration>`. It holds the build the
/// harness is pointed at, and the `atuinPath` atuin last set for it.
#[derive(Debug)]
struct Own(PathBuf);

impl Own {
    /// The directory for the configuration of the harness named `harness` whose config is in
    /// `config_home`, named by the xxh3 of that path as 16 hex digits.
    fn of(harness: &str, config_home: &Path) -> Result<Self, InstallPluginError> {
        // xxh3 takes no per-process seed and is the same in every version of the crate, so a
        // configuration keeps its directory from run to run. The path is hashed as the harness's
        // variables give it but for how its separators are spelled (`~/.codex/` is `~/.codex`),
        // links unresolved: a home that isn't there yet can't be resolved, and has to name the
        // same directory once it is.
        let config_home: PathBuf = config_home.components().collect();
        let hash = xxhash_rust::xxh3::xxh3_64(config_home.as_os_str().as_encoded_bytes());
        Ok(Self(Plugin::dir()?.joined(harness).joined(format!("{hash:016x}"))))
    }

    /// Where the build is, which is what the harness is given.
    fn build(&self) -> PathBuf {
        self.0.join("build")
    }

    fn exists(&self) -> bool {
        self.0.is_dir()
    }

    /// The `atuinPath` atuin last set for this configuration, if it has set one.
    fn wrote_atuin(&self) -> Option<String> {
        std::fs::read_to_string(self.0.join("atuin-path")).ok()
    }

    fn record_atuin(&self, atuin: &str) -> Result<(), InstallPluginError> {
        let path = self.0.join("atuin-path");
        std::fs::write(&path, atuin).map_err(|cause| InstallPluginError::Save { path, cause })
    }

    /// [`set_modkit_atuin`], remembering a path it set.
    fn set_modkit_atuin(&self, settings: &Path, atuin: &str) -> Result<(), InstallPluginError> {
        if set_modkit_atuin(settings, atuin, self.wrote_atuin().as_deref())? {
            self.record_atuin(atuin)?;
        }
        Ok(())
    }

    /// Remove the directory, and the harness's once it is empty. `false` when there was none.
    fn remove(self) -> Result<bool, InstallPluginError> {
        match std::fs::remove_dir_all(&self.0) {
            Ok(()) => {
                // Fails while another configuration's directory is in it, as it should.
                let _ = self.0.parent().map(std::fs::remove_dir);
                Ok(true)
            }
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(cause) => Err(InstallPluginError::Remove { dir: self.0, cause }),
        }
    }
}

/// Whether atuin sets a plugin's `atuinPath`, which is `current` and which atuin last set to
/// `wrote`: when none is set, or the one set is atuin's own. A path set by hand is kept.
fn sets_atuin_path(current: Option<&str>, wrote: Option<&str>) -> bool {
    current.is_none() || current == wrote
}

/// Where the plugin reads its options in pi and Codex, under the harness's `home`: a JSON object
/// of each option's name and value (modkit's `settingsFrom`, for both targets).
fn modkit_settings(home: PathBuf) -> PathBuf {
    home.joined("modkit").joined(NAME).joined("settings.json")
}

/// Set `atuinPath` to `atuin` in the plugin's options at `settings`, keeping the others, unless
/// it is set to a path other than `wrote`. Whether it now holds `atuin` as atuin's own.
fn set_modkit_atuin(
    settings: &Path,
    atuin: &str,
    wrote: Option<&str>,
) -> Result<bool, InstallPluginError> {
    let read = read_config(settings)?;
    let (path, read) = match &read {
        Some((path, read)) => (path.as_path(), Some(read.as_slice())),
        None => (settings, None),
    };
    let mut root = match read {
        Some(read) => parse_json(path, read)?,
        None => json!({}),
    };
    let options = root.as_object_mut().ok_or_else(|| InstallPluginError::Malformed {
        path: path.to_owned(),
        wanted: "it is not an object",
    })?;
    let current = options.get(ATUIN_PATH).and_then(Value::as_str);
    if current == Some(atuin) {
        return Ok(true);
    }
    if !sets_atuin_path(current, wrote) {
        return Ok(false);
    }
    options.insert(ATUIN_PATH.to_owned(), atuin.into());
    save_config(path, read, &root)?;
    Ok(true)
}

/// Take `atuinPath` out of the plugin's options at `settings` if it is `wrote`, the path atuin
/// set, keeping the others. Options left empty are removed with their file.
fn unset_modkit_atuin(settings: &Path, wrote: Option<&str>) -> Result<(), InstallPluginError> {
    let Some((path, read)) = read_config(settings)? else {
        return Ok(());
    };
    let mut root = parse_json(&path, &read)?;
    let Some(options) = root.as_object_mut() else {
        return Ok(());
    };
    let current = options.get(ATUIN_PATH).and_then(Value::as_str);
    if current.is_none() || current != wrote {
        return Ok(());
    }
    options.remove(ATUIN_PATH);
    if options.is_empty() {
        return std::fs::remove_file(&path)
            .map_err(|cause| InstallPluginError::Save { path, cause });
    }
    save_config(&path, Some(&read), &root)
}

/// `path` as an argument or a JSON string, which a harness's CLI and config take as text.
fn utf8(path: &Path) -> Result<&str, InstallPluginError> {
    path.to_str().ok_or_else(|| InstallPluginError::NonUtf8Path(path.to_owned()))
}

/// Everything a directory holds: each file's `/`-separated path under it, and its contents.
#[derive(Debug, PartialEq, Eq)]
struct Tree(BTreeMap<String, Cow<'static, [u8]>>);

impl Tree {
    fn of(files: PluginFiles) -> Self {
        Self(files.0.iter().map(|&(path, bytes)| (path.to_owned(), bytes.into())).collect())
    }

    /// A marketplace of one plugin: `files` in a directory named for it, listed by the
    /// `.claude-plugin/marketplace.json` that Claude Code and Codex both read.
    fn marketplace(files: PluginFiles) -> Self {
        let manifest = json!({
            "name": MARKETPLACE,
            "description": "The atuin plugin that `atuin setup` installed from this atuin",
            "owner": {"name": "Atuin"},
            "plugins": [{"name": NAME, "source": format!("./{NAME}")}],
        });
        let manifest = serde_json::to_vec_pretty(&manifest).expect("a JSON value serializes");
        let files = files.0.iter().map(|&(path, bytes)| (format!("{NAME}/{path}"), bytes.into()));
        let manifest = (".claude-plugin/marketplace.json".to_owned(), manifest.into());
        Self(files.chain([manifest]).collect())
    }

    /// What `dir` holds, `None` when it isn't a directory of plain files, or can't be read.
    fn read(dir: &Path) -> Option<Self> {
        let mut files = BTreeMap::new();
        let mut stack = vec![(dir.to_owned(), String::new())];
        while let Some((dir, prefix)) = stack.pop() {
            for entry in std::fs::read_dir(dir).ok()? {
                let entry = entry.ok()?;
                let path = format!("{prefix}{}", entry.file_name().to_str()?);
                let kind = entry.file_type().ok()?;
                if kind.is_dir() {
                    stack.push((entry.path(), path + "/"));
                } else if kind.is_file() {
                    files.insert(path, std::fs::read(entry.path()).ok()?.into());
                } else {
                    return None;
                }
            }
        }
        Some(Self(files))
    }

    /// Make `dir` hold exactly this, and hand it back. A `dir` that already does is left as it
    /// is, since a running harness may be loading it; one that doesn't is replaced whole.
    fn write_to(&self, dir: PathBuf) -> Result<PathBuf, InstallPluginError> {
        if Self::read(&dir).as_ref() == Some(self) {
            return Ok(dir);
        }
        match self.replace(&dir) {
            Ok(()) => Ok(dir),
            Err(cause) => Err(InstallPluginError::Write { dir, cause }),
        }
    }

    /// Write this beside `dir` and move it into `dir`'s place, so that a write cut short leaves
    /// no half of a plugin for a harness to load.
    fn replace(&self, dir: &Path) -> std::io::Result<()> {
        let parent = dir.parent().expect("a build is in atuin's own directory");
        std::fs::create_dir_all(parent)?;
        let new = tempfile::Builder::new().prefix(".new-").tempdir_in(parent)?;
        for (path, bytes) in &self.0 {
            let file = new.path().join(path);
            std::fs::create_dir_all(file.parent().expect("a file in the new directory"))?;
            std::fs::write(file, bytes)?;
        }
        match std::fs::remove_dir_all(dir) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        std::fs::rename(new.keep(), dir)
    }
}

/// opencode's global config, which atuin's MCP server is registered in too.
fn opencode_config() -> PathBuf {
    opencode::config_dir().joined("opencode.json")
}

/// The config file at `path`, as it is and its path with its links resolved, so that a write goes
/// where it is. `None` when there's no file.
fn read_config(path: &Path) -> Result<Option<(PathBuf, Vec<u8>)>, InstallPluginError> {
    // A config linked from a dotfiles repo is saved there, keeping the link.
    let path = AbsPathBuf::canonicalize(path).map_or_else(|_| path.to_owned(), PathBuf::from);
    match std::fs::read(&path) {
        Ok(read) => Ok(Some((path, read))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(cause) => Err(InstallPluginError::Read { path, cause }),
    }
}

fn parse_json(path: &Path, read: &[u8]) -> Result<Value, InstallPluginError> {
    serde_json::from_slice(read).map_err(|cause| InstallPluginError::Json {
        path: path.to_owned(),
        cause,
    })
}

/// Save `root` as the config at `path` that read `read`, `None` when there was no file. Never
/// written in place, so a failed write can't lose the rest of the config, and only over the
/// config read, not one opencode rewrote meanwhile.
fn save_config(path: &Path, read: Option<&[u8]>, root: &Value) -> Result<(), InstallPluginError> {
    let save = |cause| InstallPluginError::Save {
        path: path.to_owned(),
        cause,
    };
    let text = serde_json::to_string_pretty(root).expect("a JSON value serializes") + "\n";
    let Some(read) = read else {
        std::fs::create_dir_all(path.parent().expect("a config file is in a directory"))
            .map_err(save)?;
        return crate::fs::write_new(path, text.as_bytes()).map_err(save);
    };
    match crate::fs::replace(path, text.as_bytes(), |now| now == read) {
        Err(crate::fs::ReplaceError::NotReplaced(cause)) => Err(save(cause)),
        Err(crate::fs::ReplaceError::Unsynced(_)) | Ok(()) => Ok(()),
    }
}

/// Whether `entry` of opencode's `plugins` is the plugin in `dir`: its path, either bare or as
/// the `package` of an object that carries its options.
fn names_dir(entry: &Value, dir: &Path) -> bool {
    entry.get("package").unwrap_or(entry).as_str().is_some_and(|package| Path::new(package) == dir)
}

/// Add the plugin in `dir` to the `plugins` of the opencode config at `path`, keeping the rest of
/// the config and the plugin's other options. Its `atuinPath` becomes `atuin` unless it is set to
/// a path other than `wrote`. Whether it now holds `atuin` as atuin's own.
fn register_opencode(
    path: &Path,
    dir: &Path,
    atuin: &str,
    wrote: Option<&str>,
) -> Result<bool, InstallPluginError> {
    let package = utf8(dir)?;
    let read = read_config(path)?;
    let (path, read) = match &read {
        Some((path, read)) => (path.as_path(), Some(read.as_slice())),
        None => (path, None),
    };
    let mut root = match read {
        Some(read) => parse_json(path, read)?,
        None => json!({"$schema": "https://opencode.ai/config.json"}),
    };
    let plugins = root
        .as_object_mut()
        .and_then(|root| root.entry("plugins").or_insert_with(|| json!([])).as_array_mut())
        .ok_or_else(|| InstallPluginError::Malformed {
            path: path.to_owned(),
            wanted: "`plugins` is not an array",
        })?;
    let at = plugins.iter().position(|entry| names_dir(entry, dir));
    let mut options = at
        .and_then(|at| plugins[at].get_mut("options").map(Value::take))
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let current = options.get(ATUIN_PATH).and_then(Value::as_str);
    let sets = current == Some(atuin) || sets_atuin_path(current, wrote);
    if sets {
        options[ATUIN_PATH] = atuin.into();
    }
    let entry = json!({"package": package, "options": options});
    match at {
        Some(at) => plugins[at] = entry,
        None => plugins.push(entry),
    }
    save_config(path, read, &root)?;
    Ok(sets)
}

/// Take the plugin in `dir` out of the `plugins` of the opencode config at `path`, keeping the
/// rest of the config, and no `plugins` that is empty. `false` when it wasn't there.
fn unregister_opencode(path: &Path, dir: &Path) -> Result<bool, InstallPluginError> {
    let Some((path, read)) = read_config(path)? else {
        return Ok(false);
    };
    let mut root = parse_json(&path, &read)?;
    let Some(plugins) = root.get_mut("plugins").and_then(Value::as_array_mut) else {
        return Ok(false);
    };
    let before = plugins.len();
    plugins.retain(|entry| !names_dir(entry, dir));
    if plugins.len() == before {
        return Ok(false);
    }
    if plugins.is_empty()
        && let Some(root) = root.as_object_mut()
    {
        root.remove("plugins");
    }
    save_config(&path, Some(&read), &root)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::*;
    use crate::env::MockEnv;

    #[rstest::fixture]
    fn home() -> (tempfile::TempDir, MockEnv) {
        let dir = tempfile::tempdir().unwrap();
        let env = MockEnv::install();
        env.set("HOME", dir.path());
        (dir, env)
    }

    fn tree(files: &[(&str, &str)]) -> Tree {
        let file =
            |&(path, text): &(&str, &str)| (path.to_owned(), text.as_bytes().to_vec().into());
        Tree(files.iter().map(file).collect())
    }

    #[rstest]
    #[case::claude_code("claude-code", true)]
    #[case::codex("codex", true)]
    #[case::opencode("opencode", true)]
    #[case::pi("pi", true)]
    #[case::omp("omp", false)]
    #[case::amp("amp", false)]
    #[case::copilot("copilot", false)]
    #[case::grok("grok", false)]
    #[case::hermes("hermes", false)]
    #[case::kiro("kiro", false)]
    #[case::vibe("vibe", false)]
    fn only_the_harnesses_with_a_build_have_a_plugin(#[case] name: &str, #[case] has: bool) {
        assert_eq!(AnyHarness::from_name(name).unwrap().with_plugin().is_some(), has);
    }

    /// Whatever the directory held, it ends up holding the build and nothing else, with nothing
    /// left beside it.
    #[rstest]
    #[case::absent(None)]
    #[case::same(Some(&[("package.json", "{}"), ("hooks/hooks.json", "[]")][..]))]
    #[case::older_build(Some(&[("package.json", "{}"), ("hooks/hooks.json", "[1]")][..]))]
    #[case::dropped_file(Some(&[("package.json", "{}"), ("hooks/hooks.json", "[]"), ("old.js", "")][..]))]
    #[case::missing_file(Some(&[("package.json", "{}")][..]))]
    fn a_written_directory_holds_exactly_the_build(#[case] before: Option<&[(&str, &str)]>) {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("ai-harness").join("pi");
        if let Some(before) = before {
            tree(before).replace(&dir).unwrap();
        }
        let build = tree(&[("package.json", "{}"), ("hooks/hooks.json", "[]")]);

        build.write_to(dir.clone()).unwrap();

        assert_eq!(Tree::read(&dir), Some(build));
        assert_eq!(std::fs::read_dir(dir.parent().unwrap()).unwrap().count(), 1);
    }

    /// A configuration's directory is named by what its home hashes to, which is the same in
    /// every run and build of atuin, and no other configuration's. Not on Windows, which spells
    /// these paths with its own separator and so hashes them to other names.
    #[cfg(unix)]
    #[rstest]
    #[case::default("/home/me/.pi/agent", "pi/35ab8f40496c04d9")]
    #[case::trailing_slash("/home/me/.pi/agent/", "pi/35ab8f40496c04d9")]
    #[case::second("/home/me/work/.pi", "pi/55620f6b7a79be7d")]
    fn each_configuration_has_a_directory_of_its_own(
        home: (tempfile::TempDir, MockEnv),
        #[case] config_home: &str,
        #[case] own: &str,
    ) {
        let (home, _env) = home;
        let ai_harness = home.path().join(".atuin").join("integrations").join("ai-harness");
        assert_eq!(Own::of("pi", Path::new(config_home)).unwrap().0, ai_harness.join(own));
    }

    #[rstest]
    #[case::unset(None, None, true)]
    #[case::unset_after_atuin_set_one(None, Some("/bin/atuin"), true)]
    #[case::atuins_own(Some("/old/atuin"), Some("/old/atuin"), true)]
    #[case::by_hand(Some("/dev/atuin"), Some("/old/atuin"), false)]
    #[case::by_hand_before_atuin_set_any(Some("/dev/atuin"), None, false)]
    fn atuin_path_is_set_unless_set_by_hand(
        #[case] current: Option<&str>,
        #[case] wrote: Option<&str>,
        #[case] sets: bool,
    ) {
        assert_eq!(sets_atuin_path(current, wrote), sets);
    }

    /// The plugin's options in pi and Codex get atuin's path beside what they hold, and lose
    /// only that. `wrote` is the path atuin set before, and `kept` what a path set by hand leaves.
    #[rstest]
    #[case::none(None, None, json!({"atuinPath": "/bin/atuin"}), None)]
    #[case::others(
        Some(json!({"summaryModel": "s"})),
        None,
        json!({"summaryModel": "s", "atuinPath": "/bin/atuin"}),
        Some(json!({"summaryModel": "s"})),
    )]
    #[case::atuin_moved(
        Some(json!({"atuinPath": "/old/atuin"})),
        Some("/old/atuin"),
        json!({"atuinPath": "/bin/atuin"}),
        None,
    )]
    #[case::by_hand(
        Some(json!({"atuinPath": "/dev/atuin"})),
        Some("/old/atuin"),
        json!({"atuinPath": "/dev/atuin"}),
        Some(json!({"atuinPath": "/dev/atuin"})),
    )]
    fn modkit_settings_get_atuins_path_and_lose_only_that(
        #[case] before: Option<Value>,
        #[case] wrote: Option<&str>,
        #[case] set: Value,
        #[case] unset: Option<Value>,
    ) {
        let home = tempfile::tempdir().unwrap();
        let path = modkit_settings(home.path().to_owned());
        if let Some(before) = &before {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, before.to_string()).unwrap();
        }
        let settings = || -> Option<Value> {
            std::fs::read(&path).ok().map(|read| serde_json::from_slice(&read).unwrap())
        };

        let is_own = set_modkit_atuin(&path, "/bin/atuin", wrote).unwrap();
        assert_eq!(settings(), Some(set.clone()));
        assert_eq!(is_own, set["atuinPath"] == "/bin/atuin");

        let wrote = if is_own {
            Some("/bin/atuin")
        } else {
            wrote
        };
        unset_modkit_atuin(&path, wrote).unwrap();
        assert_eq!(settings(), unset);
    }

    fn registered(harness: PluginHarness) -> bool {
        match harness {
            PluginHarness::ClaudeCode(_) => Ccode::plugin_registered(),
            PluginHarness::Codex(_) => Codex::plugin_registered(),
            PluginHarness::Pi(pi) => pi.plugin_registered(),
            PluginHarness::Opencode(_) => {
                unreachable!("atuin writes opencode's registration, which has a test of its own")
            }
        }
        .unwrap()
    }

    /// Each harness's registration is found in its config as its own CLI wrote it there (Claude
    /// Code 2.1.332, Codex 0.171.0, pi 1.6.0). `BUILD` stands for the build's path under the
    /// home directory.
    #[rstest]
    #[case::claude_code(
        "claude-code",
        ".claude/settings.json",
        r#"{"enabledPlugins": {"atuin@atuin-local": true}}"#
    )]
    #[case::codex(
        "codex",
        ".codex/config.toml",
        "[plugins.\"atuin@atuin-local\"]\nenabled = true\n"
    )]
    #[case::pi("pi", ".pi/agent/settings.json", r#"{"packages": ["npm:other", "../../BUILD"]}"#)]
    #[case::pi_filtered(
        "pi",
        ".pi/agent/settings.json",
        r#"{"packages": [{"source": "../../BUILD", "skills": []}]}"#
    )]
    fn a_registered_plugin_is_found(
        home: (tempfile::TempDir, MockEnv),
        #[case] name: &str,
        #[case] config: &str,
        #[case] registration: &str,
    ) {
        let (home, _env) = home;
        let harness = AnyHarness::from_name(name).unwrap().with_plugin().unwrap();
        let config = home.path().join(config);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        // pi's path is followed, so what it names has to be there.
        let build = Own::of(name, config.parent().unwrap()).unwrap().build();
        std::fs::create_dir_all(&build).unwrap();
        let build = build.strip_prefix(home.path()).unwrap().to_str().unwrap();

        assert!(!registered(harness));
        std::fs::write(&config, registration.replace("BUILD", &build.replace('\\', "/"))).unwrap();
        assert!(registered(harness));
    }

    /// Taking the plugin out of one opencode configuration leaves another's build and
    /// registration, and is found to be done only once atuin's directory for it is gone.
    #[rstest]
    #[tokio::test]
    async fn removal_from_one_configuration_leaves_the_others(home: (tempfile::TempDir, MockEnv)) {
        let (home, env) = home;
        let install = |config_home: &str| {
            env.set("XDG_CONFIG_HOME", home.path().join(config_home));
            let build = Opencode.own().unwrap().build();
            tree(&[("package.json", "{}")]).write_to(build.clone()).unwrap();
            register_opencode(&opencode_config(), &build, "/bin/atuin", None).unwrap();
            build
        };
        let (work, personal) = (install("work"), install("personal"));
        assert_ne!(work, personal);

        assert!(Opencode.has_plugin().unwrap());
        assert!(Opencode.uninstall_plugin().await.unwrap());
        assert!(!Opencode.has_plugin().unwrap());
        assert!(!Opencode.uninstall_plugin().await.unwrap());
        assert!(!personal.exists());

        env.set("XDG_CONFIG_HOME", home.path().join("work"));
        assert!(Opencode.has_plugin().unwrap());
        assert_eq!(Tree::read(&work), Some(tree(&[("package.json", "{}")])));
        assert!(unregister_opencode(&opencode_config(), &work).unwrap());
    }

    /// A removal that unregistered the plugin and then stopped is still there to finish.
    #[rstest]
    #[tokio::test]
    async fn a_removal_cut_short_is_found_and_finished(home: (tempfile::TempDir, MockEnv)) {
        let (home, env) = home;
        // No `codex` to run: the marketplace's removal is tried, and may fail.
        env.set("PATH", home.path());
        let own = Codex.own().unwrap();
        tree(&[("package.json", "{}")]).write_to(own.build()).unwrap();

        assert!(Codex.has_plugin().unwrap());
        assert!(Codex.uninstall_plugin().await.unwrap());
        assert!(!Codex.has_plugin().unwrap());
        assert!(!Codex.uninstall_plugin().await.unwrap());
    }

    /// Registering with opencode keeps the other plugins and the plugin's other options, and
    /// registering again changes nothing. Unregistering takes out only the plugin.
    #[rstest]
    #[case::no_config(
        None,
        json!({"$schema": "https://opencode.ai/config.json", "plugins": [
            {"package": "/atuin/opencode", "options": {"atuinPath": "/bin/atuin"}},
        ]}),
        json!({"$schema": "https://opencode.ai/config.json"}),
    )]
    #[case::other_plugins(
        Some(json!({"model": "m", "plugins": ["other"]})),
        json!({"model": "m", "plugins": [
            "other",
            {"package": "/atuin/opencode", "options": {"atuinPath": "/bin/atuin"}},
        ]}),
        json!({"model": "m", "plugins": ["other"]}),
    )]
    #[case::bare_path(
        Some(json!({"plugins": ["/atuin/opencode"]})),
        json!({"plugins": [{"package": "/atuin/opencode", "options": {"atuinPath": "/bin/atuin"}}]}),
        json!({}),
    )]
    #[case::other_options(
        Some(json!({"plugins": [
            {"package": "/atuin/opencode", "options": {"summaryModel": "s", "atuinPath": "/old"}},
        ]})),
        json!({"plugins": [
            {"package": "/atuin/opencode", "options": {"summaryModel": "s", "atuinPath": "/bin/atuin"}},
        ]}),
        json!({}),
    )]
    #[case::path_set_by_hand(
        Some(json!({"plugins": [{"package": "/atuin/opencode", "options": {"atuinPath": "/dev/atuin"}}]})),
        json!({"plugins": [{"package": "/atuin/opencode", "options": {"atuinPath": "/dev/atuin"}}]}),
        json!({}),
    )]
    fn opencode_registers_the_plugin_beside_the_rest(
        #[case] before: Option<Value>,
        #[case] registered: Value,
        #[case] unregistered: Value,
    ) {
        let home = tempfile::tempdir().unwrap();
        let (path, dir) = (home.path().join("opencode.json"), Path::new("/atuin/opencode"));
        if let Some(before) = &before {
            std::fs::write(&path, before.to_string()).unwrap();
        }
        let config =
            || -> Value { serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap() };

        // Atuin set `/old` before, in the case that has it.
        let is_own = register_opencode(&path, dir, "/bin/atuin", Some("/old")).unwrap();
        assert_eq!(config(), registered);
        assert_eq!(is_own, registered.to_string().contains("/bin/atuin"));
        register_opencode(&path, dir, "/bin/atuin", Some("/bin/atuin")).unwrap();
        assert_eq!(config(), registered);

        assert!(unregister_opencode(&path, dir).unwrap());
        assert_eq!(config(), unregistered);
        assert!(!unregister_opencode(&path, dir).unwrap());
    }
}
