use std::collections::HashMap;
use std::env;
use std::time::Duration;

use eyre::{Result, bail};
use reqwest::{
    Response, StatusCode, Url,
    header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT},
};

use atuin_common::{
    api::{ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, ATUIN_VERSION},
    record::{EncryptedData, HostId, Record, RecordIdx},
    tls::ensure_crypto_provider,
    url::UrlAppendExt,
};
use atuin_common::{
    api::{
        ChangePasswordRequest, ErrorResponse, LoginRequest, LoginResponse, MeResponse,
        RegisterResponse,
    },
    record::RecordStatus,
};

use semver::Version;

static APP_USER_AGENT: &str = concat!("atuin/", env!("CARGO_PKG_VERSION"),);

/// Authentication token for sync API requests.
///
/// The sync API supports two authentication methods:
/// - `Bearer`: Hub API tokens (for users authenticated via Atuin Hub)
/// - `Token`: Legacy CLI session tokens (for users registered via CLI or self-hosted)
///
/// When both are available, Hub tokens are preferred as they provide unified
/// authentication across CLI and Hub features.
#[derive(Debug, Clone)]
pub enum AuthToken {
    /// Hub API token, used with "Bearer {token}" header
    Bearer(String),
    /// Legacy CLI session token, used with "Token {token}" header
    Token(String),
}

impl AuthToken {
    /// Format the token as an Authorization header value
    fn to_header_value(&self) -> String {
        match self {
            AuthToken::Bearer(token) => format!("Bearer {token}"),
            AuthToken::Token(token) => format!("Token {token}"),
        }
    }
}

pub struct Client<'a> {
    sync_addr: &'a Url,
    client: reqwest::Client,
}

/// A [`reqwest::ClientBuilder`] appropriate for the given extra headers.
///
/// reqwest only strips its own well-known sensitive headers (Authorization,
/// Cookie, ...) when following a cross-host redirect; user-configured extra
/// headers would be forwarded as-is. Since those often carry credentials
/// (e.g. Cloudflare Access secrets), refuse cross-origin redirects entirely
/// whenever extra headers are configured.
pub(crate) fn client_builder(extra_headers: &HashMap<String, String>) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();

    if extra_headers.is_empty() {
        return builder;
    }

    builder.redirect(reqwest::redirect::Policy::custom(|attempt| {
        let same_origin = attempt.previous().last().is_some_and(|prev| {
            prev.scheme() == attempt.url().scheme()
                && prev.host_str() == attempt.url().host_str()
                && prev.port_or_known_default() == attempt.url().port_or_known_default()
        });

        if !same_origin {
            attempt.error(
                "refusing to follow cross-origin redirect: extra_headers are configured and will not be sent to a different origin",
            )
        } else if attempt.previous().len() > 10 {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    }))
}

/// Build a [`HeaderMap`] from user-configured extra headers (the
/// `extra_headers` setting). Headers Atuin sets itself should be inserted
/// after these so that Atuin's values win.
pub(crate) fn extra_headers_map(extra_headers: &HashMap<String, String>) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, value) in extra_headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| eyre::eyre!("invalid extra_headers name {name:?}: {e}"))?;
        let value = HeaderValue::from_str(value)
            .map_err(|e| eyre::eyre!("invalid extra_headers value for {name:?}: {e}"))?;
        headers.insert(name, value);
    }
    Ok(headers)
}

pub async fn register(
    address: &Url,
    username: &str,
    email: &str,
    password: &str,
    extra_headers: &HashMap<String, String>,
) -> Result<RegisterResponse> {
    ensure_crypto_provider();
    let mut map = HashMap::new();
    map.insert("username", username);
    map.insert("email", email);
    map.insert("password", password);

    let mut headers = extra_headers_map(extra_headers)?;
    headers.insert(USER_AGENT, APP_USER_AGENT.parse()?);
    headers.insert(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION.parse()?);

    let client = client_builder(extra_headers).build()?;

    let url = address.append(["user", username])?;
    let resp = client.get(url).headers(headers.clone()).send().await?;

    if resp.status().is_success() {
        bail!("username already in use");
    }

    let url = address.append(["register"])?;
    let resp = client.post(url).headers(headers).json(&map).send().await?;
    let resp = handle_resp_error(resp).await?;

    if !ensure_version(&resp)? {
        bail!("could not register user due to version mismatch");
    }

    let session = resp.json::<RegisterResponse>().await?;
    Ok(session)
}

pub async fn login(
    address: &Url,
    req: LoginRequest,
    extra_headers: &HashMap<String, String>,
) -> Result<LoginResponse> {
    ensure_crypto_provider();
    let url = address.append(["login"])?;
    let client = client_builder(extra_headers).build()?;

    let mut headers = extra_headers_map(extra_headers)?;
    headers.insert(USER_AGENT, APP_USER_AGENT.parse()?);

    let resp = client.post(url).headers(headers).json(&req).send().await?;
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

    ensure_crypto_provider();
    let url = crate::settings::DEFAULT_SYNC_URL.clone();
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
            Err(e) => {
                bail!("failed to parse server version: {:?}", e);
            }
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
    let url = resp.url().to_string();

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
                bail!("Invalid request to the service at {url}, {status} - {reason}.");
            }

            bail!(
                "There was an error with the atuin sync service at {url}, server error {status}: {reason}.\nIf the problem persists, contact the host"
            );
        }

        bail!(
            "There was an error with the atuin sync service at {url}, Status {status:?}.\nIf the problem persists, contact the host"
        );
    }

    Ok(resp)
}

impl<'a> Client<'a> {
    pub fn new(
        sync_addr: &'a Url,
        auth: AuthToken,
        connect_timeout: u64,
        timeout: u64,
        extra_headers: &HashMap<String, String>,
    ) -> Result<Self> {
        ensure_crypto_provider();
        let mut headers = extra_headers_map(extra_headers)?;
        headers.insert(AUTHORIZATION, auth.to_header_value().parse()?);
        headers.insert(USER_AGENT, APP_USER_AGENT.parse()?);

        // used for semver server check
        headers.insert(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION.parse()?);

        Ok(Client {
            sync_addr,
            client: client_builder(extra_headers)
                .default_headers(headers)
                .connect_timeout(Duration::from_secs(connect_timeout))
                .timeout(Duration::from_secs(timeout))
                .build()?,
        })
    }

    pub async fn me(&self) -> Result<MeResponse> {
        let url = self.sync_addr.append_path("api/v0/me")?;

        let resp = self.client.get(url).send().await?;
        let resp = handle_resp_error(resp).await?;

        let status = resp.json::<MeResponse>().await?;

        Ok(status)
    }

    pub async fn delete_store(&self) -> Result<()> {
        let url = self.sync_addr.append_path("api/v0/store")?;

        let resp = self.client.delete(url).send().await?;

        handle_resp_error(resp).await?;

        Ok(())
    }

    pub async fn post_records(&self, records: &[Record<EncryptedData>]) -> Result<()> {
        let url = self.sync_addr.append_path("api/v0/record")?;

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

        let mut url = self.sync_addr.append_path("api/v0/record/next")?;
        url.query_pairs_mut()
            .append_pair("host", &host.0.to_string())
            .append_pair("tag", &tag)
            .append_pair("count", &count.to_string())
            .append_pair("start", &start.to_string());

        let resp = self.client.get(url).send().await?;
        let resp = handle_resp_error(resp).await?;

        let records = resp.json::<Vec<Record<EncryptedData>>>().await?;

        Ok(records)
    }

    pub async fn record_status(&self) -> Result<RecordStatus> {
        let url = self.sync_addr.append_path("api/v0/record")?;

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
        let url = self.sync_addr.append(["account"])?;

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
        let url = self.sync_addr.append_path("account/password")?;

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
            bail!("current password is incorrect");
        } else if resp.status() == 403 {
            bail!("invalid login details");
        } else if resp.status() == 200 {
            Ok(())
        } else {
            bail!("Unknown error");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_headers_map_parses_headers() {
        let mut extra = HashMap::new();
        extra.insert("X-Auth-Token".to_string(), "secret".to_string());
        let headers = extra_headers_map(&extra).unwrap();
        assert_eq!(headers.get("x-auth-token").unwrap(), "secret");
    }

    #[test]
    fn atuin_headers_override_extra_headers() {
        let mut extra = HashMap::new();
        extra.insert("Authorization".to_string(), "Token user-value".to_string());

        let mut headers = extra_headers_map(&extra).unwrap();
        headers.insert(AUTHORIZATION, "Token atuin-value".parse().unwrap());

        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Token atuin-value");
        assert_eq!(headers.get_all(AUTHORIZATION).iter().count(), 1);
    }

    #[test]
    fn extra_headers_map_rejects_invalid_names() {
        let mut extra = HashMap::new();
        extra.insert("bad header".to_string(), "value".to_string());
        assert!(extra_headers_map(&extra).is_err());
    }

    /// Serve a single connection with a canned HTTP response.
    async fn serve_one(listener: &tokio::net::TcpListener, response: String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        sock.write_all(response.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn cross_origin_redirects_refused_with_extra_headers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // A different port on the same host is a different origin
        tokio::spawn(async move {
            serve_one(
                &listener,
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{}/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    port + 1
                ),
            )
            .await;
        });

        let mut extra = HashMap::new();
        extra.insert("X-Auth-Token".to_string(), "secret".to_string());

        ensure_crypto_provider();
        let client = client_builder(&extra).build().unwrap();
        let err = client
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap_err();

        assert!(
            err.is_redirect(),
            "expected a redirect policy error: {err:?}"
        );
    }

    #[tokio::test]
    async fn same_origin_redirects_followed_with_extra_headers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            serve_one(
                &listener,
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/ok\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
            )
            .await;
            serve_one(
                &listener,
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            )
            .await;
        });

        let mut extra = HashMap::new();
        extra.insert("X-Auth-Token".to_string(), "secret".to_string());

        ensure_crypto_provider();
        let client = client_builder(&extra).build().unwrap();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert_eq!(resp.url().path(), "/ok");
    }
}
