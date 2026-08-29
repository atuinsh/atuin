use std::path::PathBuf;

use crate::shell::{Alias, Var};
use crate::store::AliasStore;
use crate::store::var::VarStore;

async fn cached_aliases(path: PathBuf, store: &AliasStore) -> String {
    match tokio::fs::read_to_string(path).await {
        Ok(aliases) => aliases,
        Err(r) => {
            // we failed to read the file for some reason, but the file does exist
            // fallback to generating new aliases on the fly

            store.powershell().await.unwrap_or_else(|e| {
                format!("echo 'Atuin: failed to read and generate aliases: \n{r}\n{e}'")
            })
        }
    }
}

async fn cached_vars(path: PathBuf, store: &VarStore) -> String {
    match tokio::fs::read_to_string(path).await {
        Ok(vars) => vars,
        Err(r) => {
            // we failed to read the file for some reason, but the file does exist
            // fallback to generating new vars on the fly

            store.powershell().await.unwrap_or_else(|e| {
                format!("echo 'Atuin: failed to read and generate vars: \n{r}\n{e}'")
            })
        }
    }
}

/// Return powershell dotfile config
///
/// Do not return an error. We should not prevent the shell from starting.
///
/// In the worst case, Atuin should not function but the shell should start correctly.
///
/// While currently this only returns aliases, it will be extended to also return other synced dotfiles
#[must_use]
pub async fn alias_config(store: &AliasStore) -> String {
    // First try to read the cached config
    let aliases = atuin_common::utils::dotfiles_cache_dir().join("aliases.ps1");

    if aliases.exists() {
        return cached_aliases(aliases, store).await;
    }

    if let Err(e) = store.build().await {
        return format!("echo 'Atuin: failed to generate aliases: {e}'");
    }

    cached_aliases(aliases, store).await
}

#[must_use]
pub async fn var_config(store: &VarStore) -> String {
    // First try to read the cached config
    let vars = atuin_common::utils::dotfiles_cache_dir().join("vars.ps1");

    if vars.exists() {
        return cached_vars(vars, store).await;
    }

    if let Err(e) = store.build().await {
        return format!("echo 'Atuin: failed to generate vars: {e}'");
    }

    cached_vars(vars, store).await
}

#[must_use]
pub fn format_alias(alias: &Alias) -> String {
    // Set-Alias doesn't support adding implicit arguments, so use a function.
    // See https://github.com/PowerShell/PowerShell/issues/12962

    let mut result = secure_command(&format!(
        "function {} {{\n    {}{} @args\n}}",
        alias.name,
        if alias.value.starts_with(['"', '\'']) {
            "& "
        } else {
            ""
        },
        alias.value
    ));

    // This makes the file layout prettier
    result.insert(0, '\n');
    result
}

#[must_use]
pub fn format_var(var: &Var) -> String {
    secure_command(&format!(
        "${}{} = '{}'",
        if var.export {
            "env:"
        } else {
            ""
        },
        var.name,
        var.value.replace('\'', "''")
    ))
}

/// Wraps the given command in an Invoke-Expression to ensure the outer script is not halted
/// if the inner command contains a syntax error.
fn secure_command(command: &str) -> String {
    format!("Invoke-Expression -ErrorAction Continue -Command '{}'\n", command.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::simple("gp", "git push", "function gp {\n    git push @args\n}")]
    #[case::quoted_path(
        "spc",
        "\"path with spaces\" arg",
        "function spc {\n    & \"path with spaces\" arg @args\n}"
    )]
    fn aliases(#[case] name: &str, #[case] value: &str, #[case] expected_inner: &str) {
        assert_eq!(
            format_alias(&Alias {
                name: name.to_string(),
                value: value.to_string(),
            }),
            "\n".to_string() + &secure_command(expected_inner)
        );
    }

    #[rstest]
    #[case::exported("FOO", "bar 'baz'", true, "$env:FOO = 'bar ''baz'''")]
    #[case::local("TEST", "1", false, "$TEST = '1'")]
    fn vars(
        #[case] name: &str,
        #[case] value: &str,
        #[case] export: bool,
        #[case] expected_inner: &str,
    ) {
        assert_eq!(
            format_var(&Var {
                name: name.to_owned(),
                value: value.to_owned(),
                export,
            }),
            secure_command(expected_inner)
        );
    }

    #[test]
    fn invoke_expression() {
        assert_eq!(
            secure_command("echo 'foo'"),
            "Invoke-Expression -ErrorAction Continue -Command 'echo ''foo'''\n"
        );
    }
}
