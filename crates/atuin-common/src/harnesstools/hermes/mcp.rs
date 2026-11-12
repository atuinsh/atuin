//! Registering atuin's MCP server with Hermes: `mcp_servers.atuin` in `<home>/config.yaml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::cli::{HermesCli, HermesCliError};
use crate::path::AbsPathBuf;

/// The config Hermes reads its MCP servers from.
#[must_use]
pub fn config_path(home: &Path) -> PathBuf {
    home.join("config.yaml")
}

/// The variables of `environment`, this shell's, that the server needs to read the same config
/// and data: the `ATUIN_` settings that say where something is, by name, those ending `_HOME`,
/// `_DIR` or `_PATH`, such as `ATUIN_HOME`, `ATUIN_CONFIG_DIR` or `ATUIN_DB_PATH`.
///
/// Only those, as a list of what to keep rather than of what to leave out: they are written to
/// Hermes's `config.yaml`, which people share, and an `ATUIN_` variable can hold a secret, such as
/// `ATUIN_AI__API_TOKEN`. A setting left out is read from atuin's config instead.
///
/// A relative `ATUIN_CONFIG_DIR` is made absolute against this process's directory, where atuin
/// reads it from (`dirs::config_path`): Hermes runs atuin from a directory of its own. The other
/// relative values stay as they are, since atuin reads them the same anywhere: `ATUIN_HOME` must
/// be absolute or is ignored, and a setting's path is relative to the config directory.
pub fn server_env(
    environment: impl IntoIterator<Item = (String, String)>,
) -> BTreeMap<String, String> {
    environment
        .into_iter()
        .filter(|(name, _)| {
            name.starts_with("ATUIN_")
                && ["_HOME", "_DIR", "_PATH"].iter().any(|suffix| name.ends_with(suffix))
        })
        .map(|(name, value)| {
            let value = if name == "ATUIN_CONFIG_DIR" {
                absolute(value)
            } else {
                value
            };
            (name, value)
        })
        .collect()
}

/// `path` made absolute against this process's directory, or as it was when it can't be: the
/// directory is gone, or isn't Unicode.
fn absolute(path: String) -> String {
    AbsPathBuf::from_current_dir(&path)
        .ok()
        .and_then(|absolute| PathBuf::from(absolute).into_os_string().into_string().ok())
        .unwrap_or(path)
}

/// Register `atuin` (by absolute path) as the `atuin` server, replacing one already there, with
/// `hermes config set`. Hermes parses the value as a mapping and saves it with its own writer,
/// which keeps the rest of `config.yaml` and its comments (`set_config_value` in
/// `hermes_cli/config.py`, v0.23.0). Not `hermes mcp add`: it asks whether to enable the tools,
/// and gives up without a terminal. A running gateway picks the server up within about a minute.
///
/// `env` ([`server_env`]) goes in the server's `env`: Hermes starts an MCP server from its
/// gateway with only a few of the gateway's variables (`PATH`, `HOME`, `XDG_*` and others,
/// `_build_safe_env` in `tools/mcp_tool_config.py`) and never an `ATUIN_` one, so without them
/// `atuin mcp` would read the default config and data rather than this shell's.
pub async fn install(
    hermes: &impl HermesCli,
    atuin: &str,
    env: &BTreeMap<String, String>,
) -> Result<(), HermesCliError> {
    let server = json!({"command": atuin, "args": ["mcp"], "env": env}).to_string();
    hermes.run(&["config", "set", "mcp_servers.atuin", &server]).await
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::Value;

    use super::super::cli::fake::FakeHermes;
    use super::*;

    #[rstest]
    #[case::plain("/opt/atuin/bin/atuin")]
    #[case::awkward(r#"/opt/Atuin's "bin": \atuin"#)]
    #[tokio::test]
    async fn sets_the_server_through_hermes(#[case] atuin: &str) {
        let hermes = FakeHermes::default();

        install(&hermes, atuin, &BTreeMap::new()).await.unwrap();

        let calls = hermes.calls();
        let [call] = calls.as_slice() else {
            panic!("{calls:?}")
        };
        let [config, set, key, value] = call.as_slice() else {
            panic!("{call:?}")
        };
        assert_eq!([config, set, key], ["config", "set", "mcp_servers.atuin"]);
        // JSON, which Hermes reads as YAML
        let value: Value = serde_json::from_str(value).unwrap();
        assert_eq!(value, json!({"command": atuin, "args": ["mcp"], "env": {}}));
    }

    /// What `env` holds goes in the server's `env`, which Hermes gives the server.
    #[rstest]
    #[tokio::test]
    async fn the_server_gets_its_env() {
        let hermes = FakeHermes::default();
        let env = BTreeMap::from([("ATUIN_CONFIG_DIR".to_owned(), "/srv/atuin/config".to_owned())]);

        install(&hermes, "/opt/atuin", &env).await.unwrap();

        let value: Value = serde_json::from_str(&hermes.calls()[0][3]).unwrap();
        assert_eq!(value["env"], json!({"ATUIN_CONFIG_DIR": "/srv/atuin/config"}));
    }

    /// Only where atuin's config and data are: never a secret, shell state, or anything else.
    #[rstest]
    #[case::home("ATUIN_HOME", true)]
    #[case::config_dir("ATUIN_CONFIG_DIR", true)]
    #[case::db_path("ATUIN_DB_PATH", true)]
    #[case::nested_path("ATUIN_DAEMON__SOCKET_PATH", true)]
    #[case::key_path("ATUIN_KEY_PATH", true)]
    #[case::ai_token("ATUIN_AI__API_TOKEN", false)]
    #[case::extra_header("ATUIN_EXTRA_HEADERS__CF_ACCESS_CLIENT_SECRET", false)]
    #[case::password("ATUIN_PASSWORD", false)]
    #[case::session("ATUIN_SESSION", false)]
    #[case::other_setting("ATUIN_SEARCH_MODE", false)]
    #[case::not_atuin("HISTDIR", false)]
    fn the_server_env_holds_only_locations(#[case] name: &str, #[case] kept: bool) {
        let env = server_env([(name.to_owned(), "x".to_owned())]);
        assert_eq!(env.contains_key(name), kept);
    }

    /// A relative `ATUIN_CONFIG_DIR` names the directory where it was set, not where Hermes runs;
    /// a relative setting's path is the config directory's, wherever atuin runs.
    #[rstest]
    fn a_relative_config_dir_is_made_absolute() {
        let env = server_env([
            ("ATUIN_CONFIG_DIR".to_owned(), "config".to_owned()),
            ("ATUIN_DB_PATH".to_owned(), "history.db".to_owned()),
            ("ATUIN_HOME".to_owned(), "/srv/atuin".to_owned()),
        ]);
        let here = std::env::current_dir().unwrap();
        assert_eq!(env["ATUIN_CONFIG_DIR"], here.join("config").to_str().unwrap());
        assert_eq!(env["ATUIN_DB_PATH"], "history.db");
        assert_eq!(env["ATUIN_HOME"], "/srv/atuin");
    }

    #[rstest]
    #[tokio::test]
    async fn a_missing_hermes_is_an_error() {
        assert!(matches!(
            install(&FakeHermes::missing(), "/opt/atuin", &BTreeMap::new()).await,
            Err(HermesCliError::NotFound)
        ));
    }

    #[rstest]
    fn the_config_is_in_the_home() {
        assert_eq!(
            config_path(Path::new("/home/me/.hermes/profiles/work")),
            Path::new("/home/me/.hermes/profiles/work/config.yaml")
        );
    }
}
