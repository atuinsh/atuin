//! Model listing and selection.
//!
//! The hub exposes the models available to this user at `/api/cli/models`.
//! Aliases are what we send on the wire; names and descriptions are for
//! display in the `/model` picker.

use atuin_api_client::{ApiError, MapApiError, types};
use eyre::{Context, Result};
use reqwest::Url;
use secrecy::SecretString;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub alias: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelList {
    /// Alias the server uses when a request doesn't specify a model.
    pub default: String,
    pub models: Vec<ModelInfo>,
}

impl From<types::ModelInfo> for ModelInfo {
    fn from(wire: types::ModelInfo) -> Self {
        Self {
            alias: wire.alias,
            name: wire.name,
            description: wire.description,
        }
    }
}

impl From<types::ModelList> for ModelList {
    fn from(wire: types::ModelList) -> Self {
        Self {
            default: wire.default,
            models: wire.models.into_iter().map(ModelInfo::from).collect(),
        }
    }
}

/// Fetch the models available to this user. Sent authenticated because the
/// server includes feature-flag-gated models only for entitled users.
pub async fn fetch_models(endpoint: &Url, token: Option<&SecretString>) -> Result<ModelList> {
    let client = crate::api::client(endpoint, token)?;
    match client.list_models().map_api_error().await {
        Ok(list) => Ok(list.into_inner().into()),
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

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const TOKEN: &str = "atapi_token";

    /// A server answering the model list with `response` to [`TOKEN`] only, and its endpoint.
    async fn serve(response: ResponseTemplate) -> (MockServer, Url) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/cli/models"))
            .and(header("authorization", format!("Bearer {TOKEN}")))
            .respond_with(response)
            .mount(&server)
            .await;
        let endpoint = Url::parse(&server.uri()).unwrap();
        (server, endpoint)
    }

    #[rstest]
    #[tokio::test]
    async fn failures_name_the_stage_that_failed() {
        let (_server, endpoint) = serve(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": "unauthorized", "message": "Invalid token"})),
        )
        .await;

        let err = fetch_models(&endpoint, Some(&SecretString::from(TOKEN))).await.unwrap_err();

        assert_eq!(err.to_string(), "model list request failed (401 Unauthorized)");
    }
}
