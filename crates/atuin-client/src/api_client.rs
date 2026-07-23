//! Thin wrapper over the progenitor-generated sync-server client
//! (`atuin-api-client`, generated from `crates/atuin-client/openapi.json` —
//! see `crates/atuin-api-client/README.md` for regeneration instructions).
//!
//! This module preserves the historical public surface: free `register`/`login`
//! functions, the `Client` struct with per-endpoint methods, and
//! `atuin_common` request/response types at the boundary.

use std::collections::HashMap;
use std::time::Duration;

use eyre::{Result, bail, eyre};
use reqwest::{
    Response, StatusCode, Url,
    header::{AUTHORIZATION, HeaderMap},
};
use uuid::Uuid;

use atuin_api_client::{Client as GeneratedClient, Error as ApiError, types as api_types};

use atuin_common::{
    api::{ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, ATUIN_VERSION},
    record::{EncryptedData, Host, HostId, Record, RecordId, RecordIdx},
    tls::ensure_crypto_provider,
    url::UrlAppendExt,
};
use atuin_common::{
    api::{LoginRequest, LoginResponse, MeResponse, RegisterResponse},
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
    client: GeneratedClient,
}

/// The generated client joins paths as `format!("{baseurl}/{path}")`, so the
/// base URL must not end with a trailing slash (a path prefix is preserved).
fn base_url(address: &Url) -> String {
    address.as_str().trim_end_matches('/').to_string()
}

/// Map a generated-client error to the historical error messages.
///
/// `url` is the URL of the request, used in the error messages.
fn map_api_error(url: &str, err: ApiError<api_types::ErrorResponse>) -> eyre::Report {
    match err {
        // Transport errors were previously propagated as bare reqwest errors
        ApiError::CommunicationError(e) => eyre::Report::new(e),
        ApiError::ErrorResponse(rv) => {
            let status = rv.status();

            if status == StatusCode::SERVICE_UNAVAILABLE {
                return eyre!(
                    "Service unavailable: check https://status.atuin.sh (or get in touch with your host)"
                );
            }

            if status == StatusCode::TOO_MANY_REQUESTS {
                return eyre!("Rate limited; please wait before doing that again");
            }

            let reason = &rv.into_inner().reason;

            if status.is_client_error() {
                return eyre!("Invalid request to the service at {url}, {status} - {reason}.");
            }

            eyre!(
                "There was an error with the atuin sync service at {url}, server error {status}: {reason}.\nIf the problem persists, contact the host"
            )
        }
        // A status code that is not documented in the OpenAPI spec (e.g. a 3xx)
        ApiError::UnexpectedResponse(resp) => {
            let status = resp.status();

            if status == StatusCode::SERVICE_UNAVAILABLE {
                return eyre!(
                    "Service unavailable: check https://status.atuin.sh (or get in touch with your host)"
                );
            }

            if status == StatusCode::TOO_MANY_REQUESTS {
                return eyre!("Rate limited; please wait before doing that again");
            }

            eyre!(
                "There was an error with the atuin sync service at {url}, Status {status:?}.\nIf the problem persists, contact the host"
            )
        }
        // Includes InvalidResponsePayload (error body that is not valid
        // ErrorResponse JSON) and other client-side failures
        other => eyre!(
            "There was an error with the atuin sync service at {url}: {other}.\nIf the problem persists, contact the host"
        ),
    }
}

fn record_to_api(record: &Record<EncryptedData>) -> api_types::RecordEncrypted {
    api_types::RecordEncrypted {
        data: api_types::EncryptedData {
            content_encryption_key: record.data.content_encryption_key.clone(),
            data: record.data.data.clone(),
        },
        host: api_types::Host {
            id: record.host.id.0,
            name: record.host.name.clone(),
        },
        id: record.id.0,
        idx: record.idx,
        tag: record.tag.clone(),
        timestamp: record.timestamp,
        version: record.version.clone(),
    }
}

fn record_from_api(record: api_types::RecordEncrypted) -> Record<EncryptedData> {
    Record {
        id: RecordId(record.id),
        idx: record.idx,
        host: Host {
            id: HostId(record.host.id),
            name: record.host.name,
        },
        timestamp: record.timestamp,
        version: record.version,
        tag: record.tag,
        data: EncryptedData {
            data: record.data.data,
            content_encryption_key: record.data.content_encryption_key,
        },
    }
}

pub async fn register(
    address: &Url,
    username: &str,
    email: &str,
    password: &str,
) -> Result<RegisterResponse> {
    ensure_crypto_provider();

    let mut headers = HeaderMap::new();
    headers.insert(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION.parse()?);

    let http = reqwest::Client::builder()
        .user_agent(APP_USER_AGENT)
        .default_headers(headers)
        .build()?;
    let client = GeneratedClient::new_with_client(&base_url(address), http);

    // Check username availability first: the server returns a success only if
    // the user already exists.
    match client.get_user(username).await {
        Ok(_) => bail!("username already in use"),
        Err(ApiError::CommunicationError(e)) => return Err(e.into()),
        // Any other response (typically a 404) means the name is available
        Err(_) => {}
    }

    let url = address.append(["register"])?;
    let resp = client
        .register(&api_types::RegisterRequest {
            email: email.to_string(),
            password: password.to_string(),
            username: username.to_string(),
        })
        .await
        .map_err(|e| map_api_error(url.as_str(), e))?;

    if !ensure_version_from_headers(resp.headers())? {
        bail!("could not register user due to version mismatch");
    }

    let session = resp.into_inner();
    Ok(RegisterResponse {
        session: session.session,
        auth: session.auth,
    })
}

pub async fn login(address: &Url, req: LoginRequest) -> Result<LoginResponse> {
    ensure_crypto_provider();

    let http = reqwest::Client::builder()
        .user_agent(APP_USER_AGENT)
        .build()?;
    let client = GeneratedClient::new_with_client(&base_url(address), http);

    let url = address.append(["login"])?;
    let resp = client
        .login(&api_types::LoginRequest {
            password: req.password,
            username: req.username,
        })
        .await
        .map_err(|e| map_api_error(url.as_str(), e))?;

    if !ensure_version_from_headers(resp.headers())? {
        bail!("Could not login due to version mismatch");
    }

    let session = resp.into_inner();
    Ok(LoginResponse {
        session: session.session,
        auth: session.auth,
    })
}

#[cfg(feature = "check-update")]
pub async fn latest_version() -> Result<Version> {
    use atuin_common::api::IndexResponse;

    ensure_crypto_provider();
    let url = crate::settings::DEFAULT_SYNC_URL.clone();
    let client = reqwest::Client::new();

    let resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, APP_USER_AGENT)
        .send()
        .await?;
    let resp = handle_resp_error(resp).await?;

    let index = resp.json::<IndexResponse>().await?;
    let version = Version::parse(index.version.as_str())?;

    Ok(version)
}

pub fn ensure_version(response: &Response) -> Result<bool> {
    ensure_version_from_headers(response.headers())
}

fn ensure_version_from_headers(headers: &HeaderMap) -> Result<bool> {
    let version = headers.get(ATUIN_HEADER_VERSION);

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

#[cfg(feature = "check-update")]
async fn handle_resp_error(resp: Response) -> Result<Response> {
    use atuin_common::api::ErrorResponse;

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
    ) -> Result<Self> {
        ensure_crypto_provider();
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, auth.to_header_value().parse()?);

        // used for semver server check
        headers.insert(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION.parse()?);

        let http = reqwest::Client::builder()
            .user_agent(APP_USER_AGENT)
            .default_headers(headers)
            .connect_timeout(Duration::new(connect_timeout, 0))
            .timeout(Duration::new(timeout, 0))
            .build()?;

        Ok(Client {
            sync_addr,
            client: GeneratedClient::new_with_client(&base_url(sync_addr), http),
        })
    }

    pub async fn me(&self) -> Result<MeResponse> {
        let url = self.sync_addr.append_path("api/v0/me")?;

        let resp = self
            .client
            .me()
            .await
            .map_err(|e| map_api_error(url.as_str(), e))?;

        Ok(MeResponse {
            username: resp.into_inner().username,
        })
    }

    pub async fn delete_store(&self) -> Result<()> {
        let url = self.sync_addr.append_path("api/v0/store")?;

        self.client
            .delete_store()
            .await
            .map_err(|e| map_api_error(url.as_str(), e))?;

        Ok(())
    }

    pub async fn post_records(&self, records: &[Record<EncryptedData>]) -> Result<()> {
        let url = self.sync_addr.append_path("api/v0/record")?;

        debug!("uploading {} records to {url}", records.len());

        let body: Vec<api_types::RecordEncrypted> = records.iter().map(record_to_api).collect();

        self.client
            .post_records(&body)
            .await
            .map_err(|e| map_api_error(url.as_str(), e))?;

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

        let resp = self
            .client
            .next_records(count, &host.0, Some(start), &tag)
            .await
            .map_err(|e| map_api_error(url.as_str(), e))?;

        Ok(resp.into_inner().into_iter().map(record_from_api).collect())
    }

    pub async fn record_status(&self) -> Result<RecordStatus> {
        let url = self.sync_addr.append_path("api/v0/record")?;

        let resp = self
            .client
            .record_status()
            .await
            .map_err(|e| map_api_error(url.as_str(), e))?;

        if !ensure_version_from_headers(resp.headers())? {
            bail!("could not sync records due to version mismatch");
        }

        let status = resp.into_inner();

        let mut hosts: HashMap<HostId, HashMap<String, RecordIdx>> =
            HashMap::with_capacity(status.hosts.len());
        for (host, tags) in status.hosts {
            hosts.insert(HostId(Uuid::parse_str(&host)?), tags);
        }
        let index = RecordStatus { hosts };

        debug!("got remote index {index:?}");

        Ok(index)
    }

    pub async fn delete(&self) -> Result<()> {
        match self.client.delete_account().await {
            Ok(_) => Ok(()),
            Err(ApiError::CommunicationError(e)) => Err(e.into()),
            Err(e) if e.status() == Some(StatusCode::FORBIDDEN) => {
                bail!("invalid login details");
            }
            Err(_) => bail!("Unknown error"),
        }
    }

    pub async fn change_password(
        &self,
        current_password: String,
        new_password: String,
    ) -> Result<()> {
        let resp = self
            .client
            .change_password(&api_types::ChangePasswordRequest {
                current_password,
                new_password,
            })
            .await;

        match resp {
            Ok(_) => Ok(()),
            Err(ApiError::CommunicationError(e)) => Err(e.into()),
            Err(e) if e.status() == Some(StatusCode::UNAUTHORIZED) => {
                bail!("current password is incorrect");
            }
            Err(e) if e.status() == Some(StatusCode::FORBIDDEN) => {
                bail!("invalid login details");
            }
            Err(_) => bail!("Unknown error"),
        }
    }
}
