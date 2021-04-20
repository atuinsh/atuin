use chrono::Utc;
use eyre::Result;
use reqwest::header::{HeaderMap, AUTHORIZATION};
use reqwest::Url;
use sodiumoxide::crypto::secretbox;

use atuin_common::api::{AddHistoryRequest, CountResponse, SyncHistoryResponse};
use atuin_common::utils::hash_str;

use crate::encryption::decrypt;
use crate::history::History;

pub struct Client<'a> {
    sync_addr: &'a str,
    token: &'a str,
    key: secretbox::Key,
    client: reqwest::Client,
}

impl<'a> Client<'a> {
    pub fn new(sync_addr: &'a str, token: &'a str, key: secretbox::Key) -> Self {
        Client {
            sync_addr,
            token,
            key,
            client: reqwest::Client::new(),
        }
    }

    pub async fn count(&self) -> Result<i64> {
        let url = format!("{}/sync/count", self.sync_addr);
        let url = Url::parse(url.as_str())?;
        let token = format!("Token {}", self.token);
        let token = token.parse()?;

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, token);

        let resp = self.client.get(url).headers(headers).send().await?;

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
            .send()
            .await?;

        Ok(())
    }
}
