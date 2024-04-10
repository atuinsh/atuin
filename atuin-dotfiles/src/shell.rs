use std::{ffi::OsStr, process::Command};

use atuin_common::shell::{shell, shell_name, ShellError};
use eyre::Result;

use crate::store::AliasStore;

pub mod bash;
pub mod fish;
pub mod xonsh;
pub mod zsh;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub name: String,
    pub value: String,
}

pub fn run_interactive<I, S>(args: I) -> Result<String, ShellError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let shell = shell_name(None);

    let output = Command::new(shell)
        .arg("-ic")
        .args(args)
        .output()
        .map_err(|e| ShellError::ExecError(e.to_string()))?;

    Ok(String::from_utf8(output.stdout).unwrap())
}

pub fn parse_alias(line: &str) -> Alias {
    let mut parts = line.split('=');

    let name = parts.next().unwrap().to_string();
    let remaining = parts.collect::<Vec<&str>>().join("=").to_string();

    Alias {
        name,
        value: remaining,
    }
}

pub fn existing_aliases() -> Result<Vec<Alias>, ShellError> {
    // this only supports posix-y shells atm
    if !shell().is_posixish() {
        return Err(ShellError::NotSupported);
    }

    // This will return a list of aliases, each on its own line
    // They will be in the form foo=bar
    let aliases = run_interactive(["alias"])?;
    let aliases: Vec<Alias> = aliases.lines().map(parse_alias).collect();

    Ok(aliases)
}

/// Import aliases from the current shell
/// This will not import aliases already in the store
/// Returns aliases that were set
pub async fn import_aliases(store: AliasStore) -> Result<Vec<Alias>> {
    let shell_aliases = existing_aliases()?;
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
    #[test]
    fn test_parse_simple_alias() {
        let alias = super::parse_alias("foo=bar");
        assert_eq!(alias.name, "foo");
        assert_eq!(alias.value, "bar");
    }

    #[test]
    fn test_parse_quoted_alias() {
        let alias = super::parse_alias("emacs='TERM=xterm-24bits emacs -nw'");
        assert_eq!(alias.name, "emacs");
        assert_eq!(alias.value, "'TERM=xterm-24bits emacs -nw'");

        let git_alias = super::parse_alias("gwip='git add -A; git rm $(git ls-files --deleted) 2> /dev/null; git commit --no-verify --no-gpg-sign --message \"--wip-- [skip ci]\"'");
        assert_eq!(git_alias.name, "gwip");
        assert_eq!(git_alias.value, "'git add -A; git rm $(git ls-files --deleted) 2> /dev/null; git commit --no-verify --no-gpg-sign --message \"--wip-- [skip ci]\"'");
    }

    #[test]
    fn test_parse_quoted_alias_equals() {
        let alias = super::parse_alias("emacs='TERM=xterm-24bits emacs -nw --foo=bar'");
        assert_eq!(alias.name, "emacs");
        assert_eq!(alias.value, "'TERM=xterm-24bits emacs -nw --foo=bar'");
    }
}
