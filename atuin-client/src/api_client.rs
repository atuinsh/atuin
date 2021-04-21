use std::collections::HashMap;

use chrono::Utc;
use eyre::{eyre, Result};
use reqwest::header::{HeaderMap, AUTHORIZATION, USER_AGENT};
use reqwest::{StatusCode, Url};
use sodiumoxide::crypto::secretbox;

use atuin_common::api::{
    AddHistoryRequest, CountResponse, LoginResponse, RegisterResponse, SyncHistoryResponse,
};
use atuin_common::utils::hash_str;

use crate::encryption::{decode_key, decrypt};
use crate::history::History;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// TODO: remove all references to the encryption key from this
// It should be handled *elsewhere*

pub struct Client<'a> {
    sync_addr: &'a str,
    token: &'a str,
    key: secretbox::Key,
    client: reqwest::Client,
}

pub fn register(
    address: &str,
    username: &str,
    email: &str,
    password: &str,
) -> Result<RegisterResponse> {
    let mut map = HashMap::new();
    map.insert("username", username);
    map.insert("email", email);
    map.insert("password", password);

    let url = format!("{}/user/{}", address, username);
    let resp = reqwest::blocking::get(url)?;

    if resp.status().is_success() {
        return Err(eyre!("username already in use"));
    }

    let url = format!("{}/register", address);
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(url)
        .header(USER_AGENT, format!("atuin/{}", VERSION))
        .json(&map)
        .send()?;

    if !resp.status().is_success() {
        return Err(eyre!("failed to register user"));
    }

    let session = resp.json::<RegisterResponse>()?;
    Ok(session)
}

pub fn login(address: &str, username: &str, password: &str) -> Result<LoginResponse> {
    let mut map = HashMap::new();
    map.insert("username", username);
    map.insert("password", password);

    let url = format!("{}/login", address);
    let client = reqwest::blocking::Client::new();

    let resp = client
        .post(url)
        .header(USER_AGENT, format!("atuin/{}", VERSION))
        .json(&map)
        .send()?;

    if resp.status() != reqwest::StatusCode::OK {
        return Err(eyre!("invalid login details"));
    }

    let session = resp.json::<LoginResponse>()?;
    Ok(session)
}

impl<'a> Client<'a> {
    pub fn new(sync_addr: &'a str, token: &'a str, key: String) -> Result<Self> {
        Ok(Client {
            sync_addr,
            token,
            key: decode_key(key)?,
            client: reqwest::Client::new(),
        })
    }

    pub async fn count(&self) -> Result<i64> {
        let url = format!("{}/sync/count", self.sync_addr);
        let url = Url::parse(url.as_str())?;
        let token = format!("Token {}", self.token);
        let token = token.parse()?;

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, token);

        let resp = self
            .client
            .get(url)
            .header(USER_AGENT, format!("atuin/{}", VERSION))
            .headers(headers)
            .send()
            .await?;

        if resp.status() != StatusCode::OK {
            return Err(eyre!("failed to get count (are you logged in?)"));
        }

        let count = resp.json::<CountResponse>().await?;

        Ok(count.count)
    }

    pub async fn get_history(
        &self,
        sync_ts: chrono::DateTime<Utc>,
        history_ts: chrono::DateTime<Utc>,
        host: Option<String>,
    ) -> Result<Vec<History>> {
        let host = match host {
            None => hash_str(&format!("{}:{}", whoami::hostname(), whoami::username())),
            Some(h) => h,
        };

        let url = format!(
            "{}/sync/history?sync_ts={}&history_ts={}&host={}",
            self.sync_addr,
            urlencoding::encode(sync_ts.to_rfc3339().as_str()),
            urlencoding::encode(history_ts.to_rfc3339().as_str()),
            host,
        );

        let resp = self
            .client
            .get(url)
            .header(AUTHORIZATION, format!("Token {}", self.token))
            .header(USER_AGENT, format!("atuin/{}", VERSION))
            .send()
            .await?;

        let history = resp.json::<SyncHistoryResponse>().await?;
        let history = history
            .history
            .iter()
            .map(|h| serde_json::from_str(h).expect("invalid base64"))
            .map(|h| decrypt(&h, &self.key).expect("failed to decrypt history! check your key"))
            .collect();

        Ok(history)
    }

    pub async fn post_history(&self, history: &[AddHistoryRequest]) -> Result<()> {
        let url = format!("{}/history", self.sync_addr);
        let url = Url::parse(url.as_str())?;

        self.client
            .post(url)
            .json(history)
            .header(AUTHORIZATION, format!("Token {}", self.token))
            .header(USER_AGENT, format!("atuin/{}", VERSION))
            .send()
            .await?;

        Ok(())
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<LoginResponse> {
        let mut map = HashMap::new();
        map.insert("username", username);
        map.insert("password", password);

        let url = format!("{}/login", self.sync_addr);
        let resp = self
            .client
            .post(url)
            .json(&map)
            .header(USER_AGENT, format!("atuin/{}", VERSION))
            .send()
            .await?;

        if resp.status() != reqwest::StatusCode::OK {
            return Err(eyre!("invalid login details"));
        }

        let session = resp.json::<LoginResponse>().await?;

        Ok(session)
    }
}
