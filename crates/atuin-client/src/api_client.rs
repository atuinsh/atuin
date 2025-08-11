use std::collections::HashMap;
use std::env;
use std::time::Duration;

use eyre::{Result, bail, eyre};
use reqwest::{
    Response, StatusCode, Url,
    header::{AUTHORIZATION, HeaderMap, USER_AGENT},
};

use atuin_common::{
    api::{ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, ATUIN_VERSION},
    record::{EncryptedData, HostId, Record, RecordIdx},
};
use atuin_common::{
    api::{
        AddHistoryRequest, ChangePasswordRequest, CountResponse, DeleteHistoryRequest,
        ErrorResponse, LoginRequest, LoginResponse, MeResponse, RegisterResponse,
        SendVerificationResponse, StatusResponse, SyncHistoryResponse, VerificationTokenRequest,
        VerificationTokenResponse,
    },
    record::RecordStatus,
};

use semver::Version;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{history::History, sync::hash_str, utils::get_host_user};

static APP_USER_AGENT: &str = concat!("atuin/", env!("CARGO_PKG_VERSION"),);

pub struct Client<'a> {
    sync_addr: &'a str,
    client: reqwest::Client,
}

fn make_url(address: &str, path: &str) -> Result<String> {
    // `join()` expects a trailing `/` in order to join paths
    // e.g. it treats `http://host:port/subdir` as a file called `subdir`
    let address = if address.ends_with("/") {
        address
    } else {
        &format!("{address}/")
    };

    // passing a path with a leading `/` will cause `join()` to replace the entire URL path
    let path = path.strip_prefix("/").unwrap_or(path);

    let url = Url::parse(address)
        .map(|url| url.join(path))?
        .map_err(|_| eyre!("invalid address"))?;

    Ok(url.to_string())
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

    let url = make_url(address, &format!("/user/{username}"))?;
    let resp = reqwest::get(url).await?;

    if resp.status().is_success() {
        bail!("username already in use");
    }

    let url = make_url(address, "/register")?;
    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .header(USER_AGENT, APP_USER_AGENT)
        .header(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION)
        .json(&map)
        .send()
        .await?;
    let resp = handle_resp_error(resp).await?;

    if !ensure_version(&resp)? {
        bail!("could not register user due to version mismatch");
    }

    let session = resp.json::<RegisterResponse>().await?;
    Ok(session)
}

pub async fn login(address: &str, req: LoginRequest) -> Result<LoginResponse> {
    let url = make_url(address, "/login")?;
    let client = reqwest::Client::new();

    let resp = client
        .post(url)
        .header(USER_AGENT, APP_USER_AGENT)
        .json(&req)
        .send()
        .await?;
    let resp = handle_resp_error(resp).await?;

    if !ensure_version(&resp)? {
        bail!("Could not login due to version mismatch");
    }

    let session = resp.json::<LoginResponse>().await?;
    Ok(session)
}

#[cfg(feature = "check-update")]
pub async fn latest_version() -> Result<Version> {
    use atuin_common::api::IndexResponse;

    let url = "https://api.atuin.sh";
    let client = reqwest::Client::new();

    let resp = client
        .get(url)
        .header(USER_AGENT, APP_USER_AGENT)
        .send()
        .await?;
    let resp = handle_resp_error(resp).await?;

    let index = resp.json::<IndexResponse>().await?;
    let version = Version::parse(index.version.as_str())?;

    Ok(version)
}

pub fn ensure_version(response: &Response) -> Result<bool> {
    let version = response.headers().get(ATUIN_HEADER_VERSION);

    let version = if let Some(version) = version {
        match version.to_str() {
            Ok(v) => Version::parse(v),
            Err(e) => bail!("failed to parse server version: {:?}", e),
        }
    } else {
        bail!("Server not reporting its version: it is either too old or unhealthy");
    }?;

    // If the client is newer than the server
    if version.major < ATUIN_VERSION.major {
        println!(
            "Atuin version mismatch! In order to successfully sync, the server needs to run a newer version of Atuin"
        );
        println!("Client: {ATUIN_CARGO_VERSION}");
        println!("Server: {version}");

        return Ok(false);
    }

    Ok(true)
}

async fn handle_resp_error(resp: Response) -> Result<Response> {
    let status = resp.status();

    if status == StatusCode::SERVICE_UNAVAILABLE {
        bail!(
            "Service unavailable: check https://status.atuin.sh (or get in touch with your host)"
        );
    }

    if status == StatusCode::TOO_MANY_REQUESTS {
        bail!("Rate limited; please wait before doing that again");
    }

    if !status.is_success() {
        if let Ok(error) = resp.json::<ErrorResponse>().await {
            let reason = error.reason;

            if status.is_client_error() {
                bail!("Invalid request to the service: {status} - {reason}.")
            }

            bail!(
                "There was an error with the atuin sync service, server error {status}: {reason}.\nIf the problem persists, contact the host"
            )
        }

        bail!(
            "There was an error with the atuin sync service: Status {status:?}.\nIf the problem persists, contact the host"
        )
    }

    Ok(resp)
}

impl<'a> Client<'a> {
    pub fn new(
        sync_addr: &'a str,
        session_token: &str,
        connect_timeout: u64,
        timeout: u64,
    ) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, format!("Token {session_token}").parse()?);

        // used for semver server check
        headers.insert(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION.parse()?);

        Ok(Client {
            sync_addr,
            client: reqwest::Client::builder()
                .user_agent(APP_USER_AGENT)
                .default_headers(headers)
                .connect_timeout(Duration::new(connect_timeout, 0))
                .timeout(Duration::new(timeout, 0))
                .build()?,
        })
    }

    pub async fn count(&self) -> Result<i64> {
        let url = make_url(self.sync_addr, "/sync/count")?;
        let url = Url::parse(url.as_str())?;

        let resp = self.client.get(url).send().await?;
        let resp = handle_resp_error(resp).await?;

        if !ensure_version(&resp)? {
            bail!("could not sync due to version mismatch");
        }

        if resp.status() != StatusCode::OK {
            bail!("failed to get count (are you logged in?)");
        }

        let count = resp.json::<CountResponse>().await?;

        Ok(count.count)
    }

    pub async fn status(&self) -> Result<StatusResponse> {
        let url = make_url(self.sync_addr, "/sync/status")?;
        let url = Url::parse(url.as_str())?;

        let resp = self.client.get(url).send().await?;
        let resp = handle_resp_error(resp).await?;

        if !ensure_version(&resp)? {
            bail!("could not sync due to version mismatch");
        }

        let status = resp.json::<StatusResponse>().await?;

        Ok(status)
    }

    pub async fn me(&self) -> Result<MeResponse> {
        let url = make_url(self.sync_addr, "/api/v0/me")?;
        let url = Url::parse(url.as_str())?;

        let resp = self.client.get(url).send().await?;
        let resp = handle_resp_error(resp).await?;

        let status = resp.json::<MeResponse>().await?;

        Ok(status)
    }

    pub async fn get_history(
        &self,
        sync_ts: OffsetDateTime,
        history_ts: OffsetDateTime,
        host: Option<String>,
    ) -> Result<SyncHistoryResponse> {
        let host = host.unwrap_or_else(|| hash_str(&get_host_user()));

        let url = make_url(
            self.sync_addr,
            &format!(
                "/sync/history?sync_ts={}&history_ts={}&host={}",
                urlencoding::encode(sync_ts.format(&Rfc3339)?.as_str()),
                urlencoding::encode(history_ts.format(&Rfc3339)?.as_str()),
                host,
            ),
        )?;

        let resp = self.client.get(url).send().await?;
        let resp = handle_resp_error(resp).await?;

        let history = resp.json::<SyncHistoryResponse>().await?;
        Ok(history)
    }

    pub async fn post_history(&self, history: &[AddHistoryRequest]) -> Result<()> {
        let url = make_url(self.sync_addr, "/history")?;
        let url = Url::parse(url.as_str())?;

        let resp = self.client.post(url).json(history).send().await?;
        handle_resp_error(resp).await?;

        Ok(())
    }

    pub async fn delete_history(&self, h: History) -> Result<()> {
        let url = make_url(self.sync_addr, "/history")?;
        let url = Url::parse(url.as_str())?;

        let resp = self
            .client
            .delete(url)
            .json(&DeleteHistoryRequest {
                client_id: h.id.to_string(),
            })
            .send()
            .await?;

        handle_resp_error(resp).await?;

        Ok(())
    }

    pub async fn delete_store(&self) -> Result<()> {
        let url = make_url(self.sync_addr, "/api/v0/store")?;
        let url = Url::parse(url.as_str())?;

        let resp = self.client.delete(url).send().await?;

        handle_resp_error(resp).await?;

        Ok(())
    }

    pub async fn post_records(&self, records: &[Record<EncryptedData>]) -> Result<()> {
        let url = make_url(self.sync_addr, "/api/v0/record")?;
        let url = Url::parse(url.as_str())?;

        debug!("uploading {} records to {url}", records.len());

        let resp = self.client.post(url).json(records).send().await?;
        handle_resp_error(resp).await?;

        Ok(())
    }

    pub async fn next_records(
        &self,
        host: HostId,
        tag: String,
        start: RecordIdx,
        count: u64,
    ) -> Result<Vec<Record<EncryptedData>>> {
        debug!("fetching record/s from host {}/{}/{}", host.0, tag, start);

        let url = make_url(
            self.sync_addr,
            &format!(
                "/api/v0/record/next?host={}&tag={}&count={}&start={}",
                host.0, tag, count, start
            ),
        )?;

        let url = Url::parse(url.as_str())?;

        let resp = self.client.get(url).send().await?;
        let resp = handle_resp_error(resp).await?;

        let records = resp.json::<Vec<Record<EncryptedData>>>().await?;

        Ok(records)
    }

    pub async fn record_status(&self) -> Result<RecordStatus> {
        let url = make_url(self.sync_addr, "/api/v0/record")?;
        let url = Url::parse(url.as_str())?;

        let resp = self.client.get(url).send().await?;
        let resp = handle_resp_error(resp).await?;

        if !ensure_version(&resp)? {
            bail!("could not sync records due to version mismatch");
        }

        let index = resp.json().await?;

        debug!("got remote index {index:?}");

        Ok(index)
    }

    pub async fn delete(&self) -> Result<()> {
        let url = make_url(self.sync_addr, "/account")?;
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

    pub async fn change_password(
        &self,
        current_password: String,
        new_password: String,
    ) -> Result<()> {
        let url = make_url(self.sync_addr, "/account/password")?;
        let url = Url::parse(url.as_str())?;

        let resp = self
            .client
            .patch(url)
            .json(&ChangePasswordRequest {
                current_password,
                new_password,
            })
            .send()
            .await?;

        if resp.status() == 401 {
            bail!("current password is incorrect")
        } else if resp.status() == 403 {
            bail!("invalid login details");
        } else if resp.status() == 200 {
            Ok(())
        } else {
            bail!("Unknown error");
        }
    }

    // Either request a verification email if token is null, or validate a token
    pub async fn verify(&self, token: Option<String>) -> Result<(bool, bool)> {
        // could dedupe this a bit, but it's simple at the moment
        let (email_sent, verified) = if let Some(token) = token {
            let url = make_url(self.sync_addr, "/api/v0/account/verify")?;
            let url = Url::parse(url.as_str())?;

            let resp = self
                .client
                .post(url)
                .json(&VerificationTokenRequest { token })
                .send()
                .await?;
            let resp = handle_resp_error(resp).await?;
            let resp = resp.json::<VerificationTokenResponse>().await?;

            (false, resp.verified)
        } else {
            let url = make_url(self.sync_addr, "/api/v0/account/send-verification")?;
            let url = Url::parse(url.as_str())?;

            let resp = self.client.post(url).send().await?;
            let resp = handle_resp_error(resp).await?;
            let resp = resp.json::<SendVerificationResponse>().await?;

            (resp.email_sent, resp.verified)
        };

        Ok((email_sent, verified))
    }
}
