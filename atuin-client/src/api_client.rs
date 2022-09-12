use std::collections::HashMap;

use chrono::Utc;
use eyre::{bail, Result};
use sodiumoxide::crypto::secretbox;

use atuin_common::api::{
    AddHistoryRequest, CountResponse, LoginRequest, LoginResponse, RegisterResponse,
    SyncHistoryResponse,
};
use ureq::{MiddlewareNext, Request};

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
    key: secretbox::Key,
    client: ureq::Agent,
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
    let resp = ureq::get(&url).call()?;

    if matches!(resp.status(), 200..=299) {
        bail!("username already in use");
    }

    let url = format!("{}/register", address);
    let resp = ureq::post(&url)
        .set("User-Agent", APP_USER_AGENT)
        .send_json(map)?;

    if !matches!(resp.status(), 200..=299) {
        bail!("failed to register user");
    }

    let session = resp.into_json::<RegisterResponse>()?;
    Ok(session)
}

pub fn login(address: &str, req: LoginRequest) -> Result<LoginResponse> {
    let url = format!("{}/login", address);

    let resp = ureq::post(&url)
        .set("User-Agent", APP_USER_AGENT)
        .send_json(req)?;

    if resp.status() != 200 {
        bail!("invalid login details");
    }

    let session = resp.into_json::<LoginResponse>()?;
    Ok(session)
}

impl<'a> Client<'a> {
    pub fn new(sync_addr: &'a str, session_token: &'a str, key: String) -> Result<Self> {
        let token = format!("Token {}", session_token);

        Ok(Client {
            sync_addr,
            key: decode_key(key)?,
            client: ureq::builder()
                .user_agent(APP_USER_AGENT)
                .middleware(move |req: Request, next: MiddlewareNext<'_>| {
                    next.handle(req.set("Authorization", &token))
                })
                .build(),
        })
    }

    pub fn count(&self) -> Result<i64> {
        let url = format!("{}/sync/count", self.sync_addr);

        let resp = self.client.get(&url).call()?;

        if resp.status() != 200 {
            bail!("failed to get count (are you logged in?)");
        }

        let count = resp.into_json::<CountResponse>()?;

        Ok(count.count)
    }

    pub fn get_history(
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

        let resp = self.client.get(&url).call()?;

        let history = resp.into_json::<SyncHistoryResponse>()?;
        let history = history
            .history
            .iter()
            .map(|h| serde_json::from_str(h).expect("invalid base64"))
            .map(|h| decrypt(&h, &self.key).expect("failed to decrypt history! check your key"))
            .collect();

        Ok(history)
    }

    pub fn post_history(&self, history: &[AddHistoryRequest]) -> Result<()> {
        let url = format!("{}/history", self.sync_addr);

        self.client.post(&url).send_json(history)?;

        Ok(())
    }
}
