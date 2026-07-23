//! Model listing and selection.
//!
//! The hub exposes the models available to this user at `/api/cli/models`.
//! Aliases are what we send on the wire; names and descriptions are for
//! display in the `/model` picker.

use std::time::Duration;

use atuin_common::url::UrlAppendExt;
use eyre::{Context, Result};
use reqwest::header::USER_AGENT;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ModelInfo {
    pub alias: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ModelList {
    /// Alias the server uses when a request doesn't specify a model.
    pub default: String,
    pub models: Vec<ModelInfo>,
}

/// Fetch the models available to this user. Sent authenticated because the
/// server includes feature-flag-gated models only for entitled users.
pub(crate) async fn fetch_models(endpoint: &reqwest::Url, token: &str) -> Result<ModelList> {
    atuin_common::tls::ensure_crypto_provider();
    let url = endpoint.append_path("api/cli/models")?;

    let mut request = reqwest::Client::new()
        .get(url)
        .header(USER_AGENT, crate::stream::APP_USER_AGENT)
        .timeout(Duration::from_secs(10));
    if !token.is_empty() {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.context("failed to fetch model list")?;

    let status = response.status();
    if !status.is_success() {
        eyre::bail!("model list request failed ({status})");
    }

    response
        .json::<ModelList>()
        .await
        .context("failed to parse model list")
}

/// Persist the chosen alias to `ai.model` in config.toml so it becomes the
/// default for future sessions. Already-running sessions keep the model they
/// read at startup.
pub(crate) async fn save_model_selection(alias: &str) -> Result<()> {
    let config_file = atuin_client::settings::Settings::get_config_path()?;
    let config_str = tokio::fs::read_to_string(&config_file)
        .await
        .unwrap_or_default();
    let mut doc = config_str.parse::<toml_edit::DocumentMut>()?;

    if !doc.contains_key("ai") {
        doc["ai"] = toml_edit::table();
    }
    doc["ai"]["model"] = toml_edit::value(alias);

    tokio::fs::write(&config_file, doc.to_string()).await?;
    Ok(())
}
