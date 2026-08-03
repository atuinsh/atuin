use std::ops::Deref;

use bstr::{BStr, BString, ByteSlice};
use thiserror::Error;

/// A shell variable in atuin's neutral model, with a validated name and value.
///
/// Build the components through a [`Shell`](super::Shell)'s
/// [`validate_var_name`](super::Shell::validate_var_name) and
/// [`validate_var_value`](super::Shell::validate_var_value) factories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    pub name: VarName,
    pub value: VarValue,
    /// `true` for an exported environment variable, `false` for a plain shell
    /// variable. Shells that only have environment variables (xonsh) ignore it.
    pub export: bool,
}

/// A variable name considered valid by some shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarName(BString);

/// A variable value considered valid by some shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarValue(BString);

/// Why a raw string could not become a [`VarName`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VarParsingError {
    #[error("{name:?} is not a valid variable name for {shell}")]
    InvalidName { shell: &'static str, name: BString },
}

impl Deref for VarName {
    type Target = BStr;

    fn deref(&self) -> &BStr {
        self.0.as_bstr()
    }
}

impl Deref for VarValue {
    type Target = BStr;

    fn deref(&self) -> &BStr {
        self.0.as_bstr()
    }
}

impl From<VarName> for BString {
    fn from(name: VarName) -> Self {
        name.0
    }
}

impl From<VarValue> for BString {
    fn from(value: VarValue) -> Self {
        value.0
    }
}

#[allow(unsafe_code)]
impl VarName {
    /// Wrap `name` as a [`VarName`] without validating it, bypassing
    /// [`Shell::validate_var_name`](super::Shell::validate_var_name).
    ///
    /// # Safety
    ///
    /// `name` must be a valid variable name (non-empty, an ASCII letter or `_`
    /// first, then ASCII alphanumerics or `_`). An invalid name is not undefined
    /// behaviour, but it renders into malformed shell config that can misbehave
    /// when sourced. Prefer the safe factory unless the name is already known
    /// valid.
    pub unsafe fn new_unchecked(name: impl Into<BString>) -> Self {
        Self(name.into())
    }
}

#[allow(unsafe_code)]
impl VarValue {
    /// Wrap `value` as a [`VarValue`] without going through
    /// [`Shell::validate_var_value`](super::Shell::validate_var_value).
    ///
    /// # Safety
    ///
    /// No value is currently rejected, so there is no precondition today. The
    /// constructor stays `unsafe` to keep building a value from raw bytes a
    /// deliberate opt-in, and to remain sound if a shell later constrains
    /// values.
    pub unsafe fn new_unchecked(value: impl Into<BString>) -> Self {
        Self(value.into())
    }
}
