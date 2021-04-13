use std::collections::HashMap;

use base64;
use chrono::Utc;
use eyre::Result;
use reqwest::header::AUTHORIZATION;

use crate::api::{AddHistoryRequest, CountResponse, ListHistoryResponse};
use crate::local::encryption::{decrypt, load_key};
use crate::local::history::History;
use crate::settings::Settings;
use crate::utils::hash_str;

pub struct Client<'a> {
    settings: &'a Settings,
}

impl<'a> Client<'a> {
    pub const fn new(settings: &'a Settings) -> Self {
        Client { settings }
    }

    pub fn count(&self) -> Result<i64> {
        let url = format!("{}/sync/count", self.settings.local.sync_address);
        let client = reqwest::blocking::Client::new();

        let resp = client
            .get(url)
            .header(
                AUTHORIZATION,
                format!("Token {}", self.settings.local.session_token),
            )
            .send()?;

        let count = resp.json::<CountResponse>()?;

        Ok(count.count)
    }

    pub fn get_history(
        &self,
        sync_ts: chrono::DateTime<Utc>,
        history_ts: chrono::DateTime<Utc>,
        host: Option<String>,
    ) -> Result<Vec<History>> {
        let key = load_key(self.settings)?;

        let host = match host {
            None => hash_str(&format!("{}:{}", whoami::hostname(), whoami::username())),
            Some(h) => h,
        };

        // this allows for syncing between users on the same machine
        let url = format!(
            "{}/sync/history?sync_ts={}&history_ts={}&host={}",
            self.settings.local.sync_address,
            sync_ts.to_rfc3339(),
            history_ts.to_rfc3339(),
            host,
        );
        let client = reqwest::blocking::Client::new();

        let resp = client
            .get(url)
            .header(
                AUTHORIZATION,
                format!("Token {}", self.settings.local.session_token),
            )
            .send()?;

        let history = resp.json::<ListHistoryResponse>()?;
        let history = history
            .history
            .iter()
            .map(|h| serde_json::from_str(h).expect("invalid base64"))
            .map(|h| decrypt(&h, &key).expect("failed to decrypt history! check your key"))
            .collect();

        Ok(history)
    }

    pub fn post_history(&self, history: &[AddHistoryRequest]) -> Result<()> {
        let client = reqwest::blocking::Client::new();

        let url = format!("{}/history", self.settings.local.sync_address);
        client
            .post(url)
            .json(history)
            .header(
                AUTHORIZATION,
                format!("Token {}", self.settings.local.session_token),
            )
            .send()?;

        Ok(())
    }
}
