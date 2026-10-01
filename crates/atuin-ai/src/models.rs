//! Model listing and selection.
//!
//! The hub exposes the models available to this user at `/api/cli/models`.
//! Aliases are what we send on the wire; names and descriptions are for
//! display in the `/model` picker. The list's `default` is the alias the
//! server uses when a request doesn't specify a model.

use atuin_api_client::types::ModelList;
use atuin_api_client::{ApiError, MapApiError};
use eyre::{Context, Result};
use reqwest::Url;
use secrecy::SecretString;

/// Fetch the models available to this user. Sent authenticated because the
/// server includes feature-flag-gated models only for entitled users.
pub async fn fetch_models(endpoint: &Url, token: Option<&SecretString>) -> Result<ModelList> {
    let client = crate::api::client(endpoint, token)?;
    match client.list_models().map_api_error().await {
        Ok(list) => Ok(list.into_inner()),
        Err(ApiError::Status { status, .. }) => eyre::bail!("model list request failed ({status})"),
        Err(err @ ApiError::Decode(_)) => Err(err).context("failed to parse model list"),
        Err(err @ (ApiError::Transport(_) | ApiError::NotSent(_))) => {
            Err(err).context("failed to fetch model list")
        }
    }
}

/// Persist the chosen alias to `ai.model` in config.toml so it becomes the
/// default for future sessions. Already-running sessions keep the model they
/// read at startup.
pub async fn save_model_selection(alias: &str) -> Result<()> {
    let config_file = atuin_client::settings::Settings::get_config_path()?;
    let config_str = tokio::fs::read_to_string(&config_file).await.unwrap_or_default();
    let mut doc = config_str.parse::<toml_edit::DocumentMut>()?;

    if !doc.contains_key("ai") {
        doc["ai"] = toml_edit::table();
    }
    doc["ai"]["model"] = toml_edit::value(alias);

    tokio::fs::write(&config_file, doc.to_string()).await?;
    Ok(())
}
