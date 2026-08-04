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

use bstr::{BStr, BString};

use super::{
    Alias, AliasValue, AliasesError, Rendered, RunError, Var, VarName, VarParsingError, VarValue,
};

/// A shell's aliases in atuin's neutral model, keyed by name.
pub type Aliases = HashMap<BString, AliasValue>;

/// Renders atuin's aliases into a shell's config syntax, and parses that
/// shell's own alias listing back into atuin's model. Pure — no subprocess.
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
    dash, ksh, sh, zsh,
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
