use eyre::{Result, ensure, eyre};
use rmp::{decode, encode};
use serde::Serialize;

use atuin_common::shell::{IsShell, ShellKind};

use crate::store::AliasStore;

pub mod bash;
pub mod fish;
pub mod powershell;
pub mod xonsh;
pub mod zsh;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Alias {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Var {
    pub name: String,
    pub value: String,

    // False? This is a _shell var_
    // True? This is an _env var_
    pub export: bool,
}

impl Var {
    /// Serialize into the given vec
    /// This is intended to be called by the store
    pub fn serialize(&self, output: &mut Vec<u8>) -> Result<()> {
        encode::write_array_len(output, 3)?; // 3 fields

        encode::write_str(output, self.name.as_str())?;
        encode::write_str(output, self.value.as_str())?;
        encode::write_bool(output, self.export)?;

        Ok(())
    }

    pub fn deserialize(bytes: &mut decode::Bytes) -> Result<Self> {
        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        let nfields = decode::read_array_len(bytes).map_err(error_report)?;

        ensure!(
            nfields == 3,
            "too many entries in v0 dotfiles env create record, got {}, expected {}",
            nfields,
            3
        );

        let bytes = bytes.remaining_slice();

        let (key, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
        let (value, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

        let mut bytes = decode::Bytes::new(bytes);
        let export = decode::read_bool(&mut bytes).map_err(error_report)?;

        ensure!(
            bytes.remaining_slice().is_empty(),
            "trailing bytes in encoded dotfiles env record, malformed"
        );

        Ok(Var {
            name: key.to_owned(),
            value: value.to_owned(),
            export,
        })
    }
}

/// Import aliases from the current shell.
///
/// Aliases already present in the store are skipped. Returns the aliases that
/// were newly set.
pub async fn import_aliases(store: &AliasStore) -> Result<Vec<Alias>> {
    let shell = ShellKind::current()
        .interface()
        .ok_or_else(|| eyre!("importing aliases is not supported for the current shell"))?;

    let shell_aliases = shell.aliases().await?;
    let store_aliases = store.aliases().await?;

    let mut res = Vec::new();

    for (name, value) in shell_aliases {
        // Aliases are arbitrary bytes in the shell, but the store speaks UTF-8
        // strings. Skip anything that isn't valid UTF-8 rather than failing the
        // whole import.
        let (Ok(name), Ok(value)) = (
            String::from_utf8(name.into()),
            String::from_utf8(value.shcmd().into()),
        ) else {
            continue;
        };

        let alias = Alias { name, value };

        // O(n), but n is small and imports are infrequent.
        if store_aliases.contains(&alias) {
            continue;
        }

        store.set(&alias.name, &alias.value).await?;
        res.push(alias);
    }

    Ok(res)
}
