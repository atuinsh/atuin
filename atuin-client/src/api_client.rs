use std::collections::HashMap;
use std::collections::HashSet;
use std::env;

use chrono::Utc;
use eyre::{bail, Result};
use reqwest::{
    header::{HeaderMap, AUTHORIZATION, USER_AGENT},
    StatusCode, Url,
};

use atuin_common::api::{
    AddHistoryRequest, CountResponse, DeleteHistoryRequest, ErrorResponse, IndexResponse,
    LoginRequest, LoginResponse, RegisterResponse, StatusResponse, SyncHistoryResponse,
};
use semver::Version;
use xsalsa20poly1305::Key;

use crate::{
    encryption::{decode_key, decrypt},
    history::History,
    sync::hash_str,
};

static APP_USER_AGENT: &str = concat!("atuin/", env!("CARGO_PKG_VERSION"),);

// TODO: remove all references to the encryption key from this
// It should be handled *elsewhere*

pub struct Client<'a> {
    sync_addr: &'a str,
    key: Key,
    client: reqwest::Client,
}

pub async fn register(
    address: &str,
    username: &str,
    email: &str,
    password: &str,
) -> Result<RegisterResponse> {
    let mut map = HashMap::new();
    map.insert("username", username);
    map.insert("email", email);
    map.insert("password", password);

    let url = format!("{address}/user/{username}");
    let resp = reqwest::get(url).await?;

    if resp.status().is_success() {
        bail!("username already in use");
    }

    let url = format!("{address}/register");
    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .header(USER_AGENT, APP_USER_AGENT)
        .json(&map)
        .send()
        .await?;

    if !resp.status().is_success() {
        let error = resp.json::<ErrorResponse>().await?;
        bail!("failed to register user: {}", error.reason);
    }

    let session = resp.json::<RegisterResponse>().await?;
    Ok(session)
}

pub async fn login(address: &str, req: LoginRequest) -> Result<LoginResponse> {
    let url = format!("{address}/login");
    let client = reqwest::Client::new();

    let resp = client
        .post(url)
        .header(USER_AGENT, APP_USER_AGENT)
        .json(&req)
        .send()
        .await?;

    if resp.status() != reqwest::StatusCode::OK {
        let error = resp.json::<ErrorResponse>().await?;
        bail!("invalid login details: {}", error.reason);
    }

    let session = resp.json::<LoginResponse>().await?;
    Ok(session)
}

pub async fn latest_version() -> Result<Version> {
    let url = "https://api.atuin.sh";
    let client = reqwest::Client::new();

    let resp = client
        .get(url)
        .header(USER_AGENT, APP_USER_AGENT)
        .send()
        .await?;

    if resp.status() != reqwest::StatusCode::OK {
        let error = resp.json::<ErrorResponse>().await?;
        bail!("failed to check latest version: {}", error.reason);
    }

    let index = resp.json::<IndexResponse>().await?;
    let version = Version::parse(index.version.as_str())?;

    Ok(version)
}

impl<'a> Client<'a> {
    pub fn new(sync_addr: &'a str, session_token: &'a str, key: String) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, format!("Token {session_token}").parse()?);

        Ok(Client {
            sync_addr,
            key: decode_key(key)?,
            client: reqwest::Client::builder()
                .user_agent(APP_USER_AGENT)
                .default_headers(headers)
                .build()?,
        })
    }

    pub async fn count(&self) -> Result<i64> {
        let url = format!("{}/sync/count", self.sync_addr);
        let url = Url::parse(url.as_str())?;

        let resp = self.client.get(url).send().await?;

        if resp.status() != StatusCode::OK {
            bail!("failed to get count (are you logged in?)");
        }

        let count = resp.json::<CountResponse>().await?;

        Ok(count.count)
    }

    pub async fn status(&self) -> Result<StatusResponse> {
        let url = format!("{}/sync/status", self.sync_addr);
        let url = Url::parse(url.as_str())?;

        let resp = self.client.get(url).send().await?;

        if resp.status() != StatusCode::OK {
            bail!("failed to get status (are you logged in?)");
        }

        let status = resp.json::<StatusResponse>().await?;

        Ok(status)
    }

    pub async fn get_history(
        &self,
        sync_ts: chrono::DateTime<Utc>,
        history_ts: chrono::DateTime<Utc>,
        host: Option<String>,
        deleted: HashSet<String>,
    ) -> Result<Vec<History>> {
        let host = host.unwrap_or_else(|| {
            hash_str(&format!(
                "{}:{}",
                env::var("ATUIN_HOST_NAME").unwrap_or_else(|_| whoami::hostname()),
                env::var("ATUIN_HOST_USER").unwrap_or_else(|_| whoami::username())
            ))
        });

        let url = format!(
            "{}/sync/history?sync_ts={}&history_ts={}&host={}",
            self.sync_addr,
            urlencoding::encode(sync_ts.to_rfc3339().as_str()),
            urlencoding::encode(history_ts.to_rfc3339().as_str()),
            host,
        );

        let resp = self.client.get(url).send().await?;

        let history = resp.json::<SyncHistoryResponse>().await?;
        let history = history
            .history
            .iter()
            // TODO: handle deletion earlier in this chain
            .map(|h| serde_json::from_str(h).expect("invalid base64"))
            .map(|h| decrypt(h, &self.key).expect("failed to decrypt history! check your key"))
            .map(|mut h| {
                if deleted.contains(&h.id) {
                    h.deleted_at = Some(chrono::Utc::now());
                    h.command = String::from("");
                }

                h
            })
            .collect();

        Ok(history)
    }

    pub async fn post_history(&self, history: &[AddHistoryRequest]) -> Result<()> {
        let url = format!("{}/history", self.sync_addr);
        let url = Url::parse(url.as_str())?;

        self.client.post(url).json(history).send().await?;

        Ok(())
    }

    pub async fn delete_history(&self, h: History) -> Result<()> {
        let url = format!("{}/history", self.sync_addr);
        let url = Url::parse(url.as_str())?;

        self.client
            .delete(url)
            .json(&DeleteHistoryRequest { client_id: h.id })
            .send()
            .await?;

        Ok(())
    }

    pub async fn delete(&self) -> Result<()> {
        let url = format!("{}/account", self.sync_addr);
        let url = Url::parse(url.as_str())?;

        let resp = self.client.delete(url).send().await?;

        if resp.status() == 403 {
            bail!("invalid login details");
        } else if resp.status() == 200 {
            Ok(())
        } else {
            bail!("Unknown error");
        }
    }
}
