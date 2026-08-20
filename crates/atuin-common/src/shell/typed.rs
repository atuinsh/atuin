//! A type-driven decomposition of "a shell".
//!
//! Identity is a zero-sized [`IsShell`] marker (`Bash`, `Zsh`, …). The heavy,
//! subprocess-capable handle — [`InstalledShell`] — is built only on demand via
//! [`IsShell::install`], so merely *naming* or *classifying* a shell spawns
//! nothing. Pure syntax work (rendering config, parsing an alias listing,
//! validating variables) lives in [`AliasCodec`] / [`VarCodec`], which are
//! (mostly) ZSTs shared across shells.
//!
//! The same [`IsShell`] trait is implemented both by the compile-time markers
//! and by the runtime [`Shell`](super::Shell) enum (whose associated types are
//! erased to `enum_dispatch` enums), so one generic body serves static and
//! dynamic callers alike.

use std::{borrow::Cow, collections::HashMap, io, path::Path, process::Output};

use std::str::FromStr;

use bstr::{BStr, BString};
use enum_dispatch::enum_dispatch;
use sysinfo::{System, get_current_pid};

use super::{ShellError, UnknownShell};

#[cfg(feature = "shell-syntax")]
use super::{
    Command, ShellParser, commands as classify, common::PosixParser, fish::FishParser,
    parse::Fallback,
};

use super::{
    Alias, AliasValue, AliasesError, Rendered, RunError, Var, VarName, VarParsingError, VarValue,
};

/// A shell's aliases in atuin's neutral model, keyed by name.
pub type Aliases = HashMap<BString, AliasValue>;

/// Renders atuin's aliases into a shell's config syntax, and parses that
/// shell's own alias listing back into atuin's model. Pure — no subprocess.
#[enum_dispatch]
pub trait AliasCodec {
    /// Render `aliases` into this shell's config syntax (best-effort; anything
    /// the shell can't represent lands in [`Rendered::skipped`]).
    fn render(&self, aliases: &[Alias]) -> Rendered;

    /// Parse the output of this shell's alias-listing command into a map.
    fn parse(&self, listing: &[u8]) -> Result<Aliases, AliasesError>;
}

/// Validates, quotes, and renders variables in a shell's syntax. Pure.
pub trait VarCodec {
    /// Validate `name` as a variable name in this shell.
    fn validate_name(&self, name: impl AsRef<BStr>) -> Result<VarName, VarParsingError>;

    /// Wrap `value` as a [`VarValue`] for this shell.
    fn validate_value(&self, value: impl AsRef<BStr>) -> Result<VarValue, VarParsingError>;

    /// Quote `value` as a literal in this shell's syntax.
    fn quote<'a>(&self, value: &'a [u8]) -> Cow<'a, BStr>;

    /// Render `vars` into this shell's config syntax.
    fn render(&self, vars: &[Var]) -> BString;
}

/// A handle to an installed shell binary — the only capability that can spawn
/// the shell.
#[enum_dispatch]
#[allow(
    async_fn_in_trait,
    reason = "dispatched only over AnyInstalled within our code; never needs to be Send"
)]
pub trait InstalledShell {
    /// The resolved path to the shell binary.
    fn abspath(&self) -> &Path;

    /// Probe the shell's currently-defined aliases (spawns a subprocess).
    async fn aliases(&self) -> Result<Aliases, AliasesError>;

    /// Run `command` in this shell, interactively.
    async fn run(&self, command: &str) -> Result<Output, RunError>;
}

/// A shell as a zero-sized identity, tying the marker to its capabilities.
///
/// Implemented by the per-shell markers and, with erased associated types, by
/// the runtime [`Shell`](super::Shell) enum.
pub trait IsShell: Copy {
    /// This shell's pure alias codec (render + parse).
    type Aliases: AliasCodec;
    /// This shell's pure variable codec.
    type Vars: VarCodec;
    /// This shell's installed, subprocess-capable handle.
    type Installed: InstalledShell;

    /// The name atuin uses for this shell.
    fn name(self) -> &'static str;

    /// This shell's alias codec — a cheap value (usually a ZST).
    fn aliases(self) -> Self::Aliases;

    /// This shell's variable codec — a cheap value (usually a ZST).
    fn vars(self) -> Self::Vars;

    /// Resolve the shell binary on `$PATH` and return a handle to it. The only
    /// method that touches the filesystem / can fail.
    fn install(self) -> io::Result<Self::Installed>;
}

use super::{
    bash,
    common::{PosixAliases, PosixVars},
    dash, fish, ksh, nu, powershell, sh, xonsh, zsh,
};

/// The POSIX `sh` shell, as a zero-sized identity.
#[derive(Clone, Copy)]
pub struct Sh;

impl IsShell for Sh {
    type Aliases = PosixAliases;
    type Vars = PosixVars;
    type Installed = sh::Sh;

    fn name(self) -> &'static str {
        "sh"
    }

    fn aliases(self) -> PosixAliases {
        PosixAliases
    }

    fn vars(self) -> PosixVars {
        PosixVars { shell: "sh" }
    }

    fn install(self) -> io::Result<sh::Sh> {
        Ok(sh::Sh::new(Path::new("sh")))
    }
}

/// The `bash` shell.
#[derive(Clone, Copy)]
pub struct Bash;

impl IsShell for Bash {
    type Aliases = PosixAliases;
    type Vars = PosixVars;
    type Installed = bash::Bash;

    fn name(self) -> &'static str {
        "bash"
    }

    fn aliases(self) -> PosixAliases {
        PosixAliases
    }

    fn vars(self) -> PosixVars {
        PosixVars { shell: "bash" }
    }

    fn install(self) -> io::Result<bash::Bash> {
        Ok(bash::Bash::new(Path::new("bash")))
    }
}

/// The `dash` shell.
#[derive(Clone, Copy)]
pub struct Dash;

impl IsShell for Dash {
    type Aliases = PosixAliases;
    type Vars = PosixVars;
    type Installed = dash::Dash;

    fn name(self) -> &'static str {
        "dash"
    }

    fn aliases(self) -> PosixAliases {
        PosixAliases
    }

    fn vars(self) -> PosixVars {
        PosixVars { shell: "dash" }
    }

    fn install(self) -> io::Result<dash::Dash> {
        Ok(dash::Dash::new(Path::new("dash")))
    }
}

/// The `ksh` shell.
#[derive(Clone, Copy)]
pub struct Ksh;

impl IsShell for Ksh {
    type Aliases = PosixAliases;
    type Vars = PosixVars;
    type Installed = ksh::Ksh;

    fn name(self) -> &'static str {
        "ksh"
    }

    fn aliases(self) -> PosixAliases {
        PosixAliases
    }

    fn vars(self) -> PosixVars {
        PosixVars { shell: "ksh" }
    }

    fn install(self) -> io::Result<ksh::Ksh> {
        Ok(ksh::Ksh::new(Path::new("ksh")))
    }
}

/// The `zsh` shell. Renders as POSIX but parses its own `$'…'` alias listing.
#[derive(Clone, Copy)]
pub struct Zsh;

impl IsShell for Zsh {
    type Aliases = zsh::ZshAliases;
    type Vars = PosixVars;
    type Installed = zsh::Zsh;

    fn name(self) -> &'static str {
        "zsh"
    }

    fn aliases(self) -> zsh::ZshAliases {
        zsh::ZshAliases
    }

    fn vars(self) -> PosixVars {
        PosixVars { shell: "zsh" }
    }

    fn install(self) -> io::Result<zsh::Zsh> {
        Ok(zsh::Zsh::new(Path::new("zsh")))
    }
}

/// The `fish` shell.
#[derive(Clone, Copy)]
pub struct Fish;

impl IsShell for Fish {
    type Aliases = fish::FishAliases;
    type Vars = fish::FishVars;
    type Installed = fish::Fish;

    fn name(self) -> &'static str {
        "fish"
    }

    fn aliases(self) -> fish::FishAliases {
        fish::FishAliases
    }

    fn vars(self) -> fish::FishVars {
        fish::FishVars
    }

    fn install(self) -> io::Result<fish::Fish> {
        Ok(fish::Fish::new(Path::new("fish")))
    }
}

/// The `xonsh` shell.
#[derive(Clone, Copy)]
pub struct Xonsh;

impl IsShell for Xonsh {
    type Aliases = xonsh::XonshAliases;
    type Vars = xonsh::XonshVars;
    type Installed = xonsh::Xonsh;

    fn name(self) -> &'static str {
        "xonsh"
    }

    fn aliases(self) -> xonsh::XonshAliases {
        xonsh::XonshAliases
    }

    fn vars(self) -> xonsh::XonshVars {
        xonsh::XonshVars
    }

    fn install(self) -> io::Result<xonsh::Xonsh> {
        Ok(xonsh::Xonsh::new(Path::new("xonsh")))
    }
}

/// The `nu` shell.
#[derive(Clone, Copy)]
pub struct Nu;

impl IsShell for Nu {
    type Aliases = nu::NuAliases;
    type Vars = nu::NuVars;
    type Installed = nu::Nu;

    fn name(self) -> &'static str {
        "nu"
    }

    fn aliases(self) -> nu::NuAliases {
        nu::NuAliases
    }

    fn vars(self) -> nu::NuVars {
        nu::NuVars
    }

    fn install(self) -> io::Result<nu::Nu> {
        Ok(nu::Nu::new(Path::new("nu")))
    }
}

/// The `powershell` shell.
#[derive(Clone, Copy)]
pub struct Powershell;

impl IsShell for Powershell {
    type Aliases = powershell::PowershellAliases;
    type Vars = powershell::PowershellVars;
    type Installed = powershell::Powershell;

    fn name(self) -> &'static str {
        "powershell"
    }

    fn aliases(self) -> powershell::PowershellAliases {
        powershell::PowershellAliases
    }

    fn vars(self) -> powershell::PowershellVars {
        powershell::PowershellVars
    }

    fn install(self) -> io::Result<powershell::Powershell> {
        Ok(powershell::Powershell::new(Path::new("pwsh")))
    }
}

// ── Runtime erasure ───────────────────────────────────────────────────────
// The associated types, erased to enums so a runtime `Shell` can carry them.
// `AliasCodec`/`InstalledShell` dispatch via `enum_dispatch`; `AnyVarCodec` is
// hand-rolled because `VarCodec`'s `impl AsRef<BStr>` argument is generic.

/// A shell's alias codec, erased.
#[enum_dispatch(AliasCodec)]
pub enum AnyAliasCodec {
    Posix(PosixAliases),
    Zsh(zsh::ZshAliases),
    Fish(fish::FishAliases),
    Xonsh(xonsh::XonshAliases),
    Nu(nu::NuAliases),
    Powershell(powershell::PowershellAliases),
}

/// A shell's installed handle, erased.
#[enum_dispatch(InstalledShell)]
pub enum AnyInstalled {
    Sh(sh::Sh),
    Bash(bash::Bash),
    Dash(dash::Dash),
    Ksh(ksh::Ksh),
    Zsh(zsh::Zsh),
    Fish(fish::Fish),
    Xonsh(xonsh::Xonsh),
    Nu(nu::Nu),
    Powershell(powershell::Powershell),
}

/// A shell's variable codec, erased. Hand-rolled dispatch (generic method).
pub enum AnyVarCodec {
    Posix(PosixVars),
    Fish(fish::FishVars),
    Xonsh(xonsh::XonshVars),
    Nu(nu::NuVars),
    Powershell(powershell::PowershellVars),
}

impl VarCodec for AnyVarCodec {
    fn validate_name(&self, name: impl AsRef<BStr>) -> Result<VarName, VarParsingError> {
        match self {
            Self::Posix(v) => v.validate_name(name),
            Self::Fish(v) => v.validate_name(name),
            Self::Xonsh(v) => v.validate_name(name),
            Self::Nu(v) => v.validate_name(name),
            Self::Powershell(v) => v.validate_name(name),
        }
    }

    fn validate_value(&self, value: impl AsRef<BStr>) -> Result<VarValue, VarParsingError> {
        match self {
            Self::Posix(v) => v.validate_value(value),
            Self::Fish(v) => v.validate_value(value),
            Self::Xonsh(v) => v.validate_value(value),
            Self::Nu(v) => v.validate_value(value),
            Self::Powershell(v) => v.validate_value(value),
        }
    }

    fn quote<'a>(&self, value: &'a [u8]) -> Cow<'a, BStr> {
        match self {
            Self::Posix(v) => v.quote(value),
            Self::Fish(v) => v.quote(value),
            Self::Xonsh(v) => v.quote(value),
            Self::Nu(v) => v.quote(value),
            Self::Powershell(v) => v.quote(value),
        }
    }

    fn render(&self, vars: &[Var]) -> BString {
        match self {
            Self::Posix(v) => v.render(vars),
            Self::Fish(v) => v.render(vars),
            Self::Xonsh(v) => v.render(vars),
            Self::Nu(v) => v.render(vars),
            Self::Powershell(v) => v.render(vars),
        }
    }
}

/// A shell determined at runtime — a `Copy` tag over the markers. Replaces the
/// old `ShellKind` (identity) and the old `Shell` enum_dispatch wrapper (driver)
/// in one type, with no `Unknown` variant: an unknown shell is `None`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Sh,
    Bash,
    Fish,
    Zsh,
    Dash,
    Ksh,
    Xonsh,
    Nu,
    Powershell,
}

impl IsShell for Shell {
    type Aliases = AnyAliasCodec;
    type Vars = AnyVarCodec;
    type Installed = AnyInstalled;

    fn name(self) -> &'static str {
        match self {
            Self::Sh => Sh.name(),
            Self::Bash => Bash.name(),
            Self::Fish => Fish.name(),
            Self::Zsh => Zsh.name(),
            Self::Dash => Dash.name(),
            Self::Ksh => Ksh.name(),
            Self::Xonsh => Xonsh.name(),
            Self::Nu => Nu.name(),
            Self::Powershell => Powershell.name(),
        }
    }

    fn aliases(self) -> AnyAliasCodec {
        match self {
            Self::Sh => Sh.aliases().into(),
            Self::Bash => Bash.aliases().into(),
            Self::Dash => Dash.aliases().into(),
            Self::Ksh => Ksh.aliases().into(),
            Self::Zsh => Zsh.aliases().into(),
            Self::Fish => Fish.aliases().into(),
            Self::Xonsh => Xonsh.aliases().into(),
            Self::Nu => Nu.aliases().into(),
            Self::Powershell => Powershell.aliases().into(),
        }
    }

    fn vars(self) -> AnyVarCodec {
        match self {
            Self::Sh => AnyVarCodec::Posix(Sh.vars()),
            Self::Bash => AnyVarCodec::Posix(Bash.vars()),
            Self::Dash => AnyVarCodec::Posix(Dash.vars()),
            Self::Ksh => AnyVarCodec::Posix(Ksh.vars()),
            Self::Zsh => AnyVarCodec::Posix(Zsh.vars()),
            Self::Fish => AnyVarCodec::Fish(Fish.vars()),
            Self::Xonsh => AnyVarCodec::Xonsh(Xonsh.vars()),
            Self::Nu => AnyVarCodec::Nu(Nu.vars()),
            Self::Powershell => AnyVarCodec::Powershell(Powershell.vars()),
        }
    }

    fn install(self) -> io::Result<AnyInstalled> {
        Ok(match self {
            Self::Sh => Sh.install()?.into(),
            Self::Bash => Bash.install()?.into(),
            Self::Dash => Dash.install()?.into(),
            Self::Ksh => Ksh.install()?.into(),
            Self::Zsh => Zsh.install()?.into(),
            Self::Fish => Fish.install()?.into(),
            Self::Xonsh => Xonsh.install()?.into(),
            Self::Nu => Nu.install()?.into(),
            Self::Powershell => Powershell.install()?.into(),
        })
    }
}

impl FromStr for Shell {
    type Err = UnknownShell;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Ok(match name {
            "sh" => Self::Sh,
            "bash" => Self::Bash,
            "fish" => Self::Fish,
            "zsh" => Self::Zsh,
            "dash" => Self::Dash,
            "ksh" => Self::Ksh,
            "xonsh" => Self::Xonsh,
            "nu" => Self::Nu,
            "powershell" => Self::Powershell,
            other => return Err(UnknownShell(other.to_owned())),
        })
    }
}

impl std::fmt::Display for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl Shell {
    /// The shell running atuin, or `None` if it isn't one atuin knows.
    pub fn current() -> Option<Self> {
        let sys = System::new_all();
        let process = sys.process(get_current_pid().ok()?)?;
        let parent = sys.process(process.parent()?)?;
        let name = parent.name().trim().to_lowercase();
        let name = name.strip_prefix('-').unwrap_or(&name);
        name.parse().ok()
    }

    /// The shell named by `$ATUIN_SHELL`, or `None`.
    pub fn from_env() -> Option<Self> {
        std::env::var("ATUIN_SHELL")
            .ok()?
            .trim()
            .to_lowercase()
            .parse()
            .ok()
    }

    /// Best-effort detection of the user's default login shell (`None` if it
    /// isn't one atuin knows).
    pub fn default_shell() -> Result<Option<Self>, ShellError> {
        let os = System::name().unwrap_or_default().to_lowercase();
        let out = if os.contains("darwin") {
            run_sh("dscl localhost -read \"/Local/Default/Users/$USER\" shell | awk '{print $2}'")?
        } else if cfg!(windows) {
            return Ok(Some(Self::Powershell));
        } else {
            run_sh("getent passwd $LOGNAME | cut -d: -f7")?
        };
        let path = Path::new(out.trim());
        Ok(path
            .file_name()
            .and_then(|n| n.to_string_lossy().parse().ok()))
    }
}

/// Run `command` under `sh -ic`, capturing stdout. Used by shell detection.
fn run_sh(command: &str) -> Result<String, ShellError> {
    let output = std::process::Command::new("sh")
        .arg("-ic")
        .arg(command)
        .output()
        .map_err(|e| ShellError::ExecError(e.to_string()))?;
    String::from_utf8(output.stdout).map_err(|e| ShellError::ExecError(e.to_string()))
}

/// The syntax parser for `shell`'s dialect, falling back to the word-level
/// parser when the shell is unknown (`None`) or has no grammar.
#[cfg(feature = "shell-syntax")]
pub fn parser(shell: Option<Shell>) -> &'static dyn ShellParser {
    match shell {
        Some(Shell::Bash | Shell::Sh | Shell::Zsh | Shell::Dash | Shell::Ksh) => &PosixParser,
        Some(Shell::Fish) => &FishParser,
        _ => &Fallback,
    }
}

/// Every command that will run in `code`, per `shell`'s dialect (fallback when
/// the shell is unknown).
#[cfg(feature = "shell-syntax")]
pub fn commands<'a>(shell: Option<Shell>, code: &'a str) -> Vec<Command<'a>> {
    classify(parser(shell), code)
}
