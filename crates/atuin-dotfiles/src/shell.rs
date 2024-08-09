use eyre::{ensure, eyre, Result};
use rmp::{decode, encode};
use serde::Serialize;

use atuin_common::shell::{Shell, ShellError};

use crate::store::AliasStore;

pub mod bash;
pub mod fish;
pub mod nu;
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

pub fn parse_alias(line: &str) -> Option<Alias> {
    // consider the fact we might be importing a fish alias
    // 'alias' output
    // fish: alias foo bar
    // posix: foo=bar

    let is_fish = line.split(' ').next().unwrap_or("") == "alias";

    let parts: Vec<&str> = if is_fish {
        line.split(' ')
            .enumerate()
            .filter_map(|(n, i)| if n == 0 { None } else { Some(i) })
            .collect()
    } else {
        line.split('=').collect()
    };

    if parts.len() <= 1 {
        return None;
    }

    let mut parts = parts.iter().map(|s| s.to_string());

    let name = parts.next().unwrap().to_string();

    let remaining = if is_fish {
        parts.collect::<Vec<String>>().join(" ").to_string()
    } else {
        parts.collect::<Vec<String>>().join("=").to_string()
    };

    Some(Alias {
        name,
        value: remaining.trim().to_string(),
    })
}

pub fn existing_aliases(shell: Option<Shell>) -> Result<Vec<Alias>, ShellError> {
    let shell = if let Some(shell) = shell {
        shell
    } else {
        Shell::current()
    };

    // this only supports posix-y shells atm
    if !shell.is_posixish() {
        return Err(ShellError::NotSupported);
    }

    // This will return a list of aliases, each on its own line
    // They will be in the form foo=bar
    let aliases = shell.run_interactive(["alias"])?;

    let aliases: Vec<Alias> = aliases.lines().filter_map(parse_alias).collect();

    Ok(aliases)
}

/// Import aliases from the current shell
/// This will not import aliases already in the store
/// Returns aliases that were set
pub async fn import_aliases(store: &AliasStore) -> Result<Vec<Alias>> {
    let shell_aliases = existing_aliases(None)?;
    let store_aliases = store.aliases().await?;

    let mut res = Vec::new();

    for alias in shell_aliases {
        // O(n), but n is small, and imports infrequent
        // can always make a map
        if store_aliases.contains(&alias) {
            continue;
        }

        res.push(alias.clone());
        store.set(&alias.name, &alias.value).await?;
    }

    Ok(res)
}

#[cfg(test)]
mod tests {
    use crate::shell::{parse_alias, Alias};

    #[test]
    fn test_parse_simple_alias() {
        let alias = super::parse_alias("foo=bar").expect("failed to parse alias");
        assert_eq!(alias.name, "foo");
        assert_eq!(alias.value, "bar");
    }

    #[test]
    fn test_parse_quoted_alias() {
        let alias = super::parse_alias("emacs='TERM=xterm-24bits emacs -nw'")
            .expect("failed to parse alias");

        assert_eq!(alias.name, "emacs");
        assert_eq!(alias.value, "'TERM=xterm-24bits emacs -nw'");

        let git_alias = super::parse_alias("gwip='git add -A; git rm $(git ls-files --deleted) 2> /dev/null; git commit --no-verify --no-gpg-sign --message \"--wip-- [skip ci]\"'").expect("failed to parse alias");
        assert_eq!(git_alias.name, "gwip");
        assert_eq!(git_alias.value, "'git add -A; git rm $(git ls-files --deleted) 2> /dev/null; git commit --no-verify --no-gpg-sign --message \"--wip-- [skip ci]\"'");
    }

    #[test]
    fn test_parse_quoted_alias_equals() {
        let alias = super::parse_alias("emacs='TERM=xterm-24bits emacs -nw --foo=bar'")
            .expect("failed to parse alias");
        assert_eq!(alias.name, "emacs");
        assert_eq!(alias.value, "'TERM=xterm-24bits emacs -nw --foo=bar'");
    }

    #[test]
    fn test_parse_fish() {
        let alias = super::parse_alias("alias foo bar").expect("failed to parse alias");
        assert_eq!(alias.name, "foo");
        assert_eq!(alias.value, "bar");

        let alias =
            super::parse_alias("alias x 'exa --icons --git --classify --group-directories-first'")
                .expect("failed to parse alias");

        assert_eq!(alias.name, "x");
        assert_eq!(
            alias.value,
            "'exa --icons --git --classify --group-directories-first'"
        );
    }

    #[test]
    fn test_parse_with_fortune() {
        // Because we run the alias command in an interactive subshell
        // there may be other output.
        // Ensure that the parser can handle it
        // Annoyingly not all aliases are picked up all the time if we use
        // a non-interactive subshell. Boo.
        let shell = "
/ In a consumer society there are     \\
| inevitably two kinds of slaves: the |
| prisoners of addiction and the      |
\\ prisoners of envy.                  /
 -------------------------------------
        \\   ^__^
         \\  (oo)\\_______
            (__)\\       )\\/\\
                ||----w |
                ||     ||
emacs='TERM=xterm-24bits emacs -nw --foo=bar'
k=kubectl
";

        let aliases: Vec<Alias> = shell.lines().filter_map(parse_alias).collect();
        assert_eq!(aliases[0].name, "emacs");
        assert_eq!(aliases[0].value, "'TERM=xterm-24bits emacs -nw --foo=bar'");

        assert_eq!(aliases[1].name, "k");
        assert_eq!(aliases[1].value, "kubectl");
    }
}
