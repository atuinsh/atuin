//! Registering atuin's MCP server with Hermes: `mcp_servers.atuin` in `<home>/config.yaml`.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::cli::{HermesCli, HermesCliError};

/// The config Hermes reads its MCP servers from.
#[must_use]
pub fn config_path(home: &Path) -> PathBuf {
    home.join("config.yaml")
}

/// Register `atuin` (by absolute path) as the `atuin` server, replacing one already there, with
/// `hermes config set`. Hermes parses the value as a mapping and saves it with its own writer,
/// which keeps the rest of `config.yaml` and its comments (`set_config_value` in
/// `hermes_cli/config.py`, v0.23.0). Not `hermes mcp add`: it asks whether to enable the tools,
/// and gives up without a terminal. A running gateway picks the server up within about a minute.
pub async fn install(hermes: &impl HermesCli, atuin: &str) -> Result<(), HermesCliError> {
    let server = json!({"command": atuin, "args": ["mcp"], "env": {}}).to_string();
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

        install(&hermes, atuin).await.unwrap();

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

    #[rstest]
    #[tokio::test]
    async fn a_missing_hermes_is_an_error() {
        assert!(matches!(
            install(&FakeHermes::missing(), "/opt/atuin").await,
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
