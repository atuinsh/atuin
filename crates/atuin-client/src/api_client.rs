use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use async_stream::try_stream;
use atuin_api_client::{
    ApiError, AuthHeaderProvider, CapClient, CapMismatch, MapApiError, ResponseValue,
    authorization, types,
};
use atuin_common::range::{Chunks, RangeExt};
use atuin_common::url::UrlAppendError;
use atuin_domain::api::{ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, ATUIN_VERSION};
use atuin_domain::record::{
    EncryptedData, Record, RecordId, RecordIdx, RecordSeriesKey, RecordStatus,
};
use easy_cast::Conv;
use eyre::{Result, bail, eyre};
use futures::{Stream, StreamExt, TryStreamExt, stream};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, InvalidHeaderValue};
use reqwest::{Response, StatusCode, Url};
use secrecy::{ExposeSecret, SecretString};
use semver::Version;
use tracing::{Instrument, instrument};

use crate::http::identity_headers;
use crate::packfile::PackedPackfile;
use crate::settings::Settings;

/// How many record pages to download in parallel. See [`Client::records`].
const MAX_RECORDS_CONCURRENT_DOWNLOAD: usize = 8;

/// How many packfile blobs [`Client::upload_packfiles`] transfers concurrently.
const MAX_CONCURRENT_PACKFILE_UPLOADS: usize = 16;

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
    Bearer(SecretString),
    /// Legacy CLI session token, used with "Token {token}" header
    Token(SecretString),
}

impl AuthToken {
    /// Format the token as a sensitive Authorization header value.
    pub(crate) fn to_header_value(&self) -> Result<HeaderValue, InvalidHeaderValue> {
        match self {
            Self::Bearer(token) => authorization("Bearer", token),
            Self::Token(token) => authorization("Token", token),
        }
    }
}

#[derive(Clone)]
pub struct Client {
    /// The sync API, negotiating capabilities through [`Self::caps`].
    api: atuin_api_client::Client,
    /// Used for uploading "LFS" data to S3. Carries no default headers, unlike [`Self::api`].
    lfs_client: reqwest::Client,
    caps: Arc<CapClient>,
}

/// A [`reqwest::ClientBuilder`] appropriate for the given extra headers.
///
/// reqwest only strips its own well-known sensitive headers (Authorization,
/// Cookie, ...) when following a cross-host redirect; user-configured extra
/// headers would be forwarded as-is. Since those often carry credentials
/// (e.g. Cloudflare Access secrets), refuse cross-origin redirects entirely
/// whenever extra headers are configured.
fn client_builder(extra_headers: &HashMap<String, SecretString>) -> reqwest::ClientBuilder {
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
                "refusing to follow cross-origin redirect: extra_headers are configured and will \
                 not be sent to a different origin",
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
///
/// Every value is marked sensitive, since these often carry credentials.
fn extra_headers_map(extra_headers: &HashMap<String, SecretString>) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, value) in extra_headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| eyre::eyre!("invalid extra_headers name {name:?}: {e}"))?;
        let mut value = HeaderValue::from_str(value.expose_secret())
            .map_err(|e| eyre::eyre!("invalid extra_headers value for {name:?}: {e}"))?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    Ok(headers)
}

/// The HTTP client for authenticated calls to the sync server: the user's `extra_headers`, with
/// `auth` and Atuin's identity over them.
pub(crate) fn authenticated_http(
    auth: &AuthToken,
    connect_timeout: Duration,
    timeout: Duration,
    extra_headers: &HashMap<String, SecretString>,
) -> Result<reqwest::Client> {
    let mut headers = extra_headers_map(extra_headers)?;
    headers.extend(identity_headers());
    headers.insert(AUTHORIZATION, auth.to_header_value()?);

    Ok(client_builder(extra_headers)
        .default_headers(headers)
        .connect_timeout(connect_timeout)
        .timeout(timeout)
        .build()?)
}

/// A client for the sync server at `sync_addr` that sends the user's `extra_headers` and Atuin's
/// identity but no credentials of its own, with no timeouts.
fn anonymous_api(
    sync_addr: &Url,
    extra_headers: &HashMap<String, SecretString>,
) -> Result<atuin_api_client::Client> {
    let mut headers = extra_headers_map(extra_headers)?;
    headers.extend(identity_headers());

    let http = client_builder(extra_headers).default_headers(headers).build()?;
    Ok(atuin_api_client::Client::from_http(sync_addr, http)?)
}

/// Create the account `username` on the sync server at `address`, unless the name is taken.
#[instrument(level = "trace", skip_all, err)]
pub async fn register(
    address: &Url,
    username: &str,
    email: &str,
    password: &SecretString,
    extra_headers: &HashMap<String, SecretString>,
) -> Result<types::RegisterResponse> {
    let api = anonymous_api(address, extra_headers)?;

    if username_taken(&api, username).await? {
        bail!("username already in use");
    }

    let body = types::RegisterRequest {
        email: email.to_owned(),
        username: username.to_owned(),
        password: password.clone().into(),
    };
    let resp = api_call(api.legacy_register(&body)).await?;

    if !server_version_compatible(resp.headers())? {
        bail!("could not register user due to version mismatch");
    }

    Ok(resp.into_inner())
}

/// Whether the sync server answers its lookup of `username` with any 2xx.
///
/// A name with a `\` is never looked up, since the path would read it as `/`; no server accepts
/// one, so its registration gets the server's own refusal.
async fn username_taken(api: &atuin_api_client::Client, username: &str) -> Result<bool> {
    // The lookup puts `username` in the path, where the client would resolve these away.
    if matches!(username, "." | "..") {
        return Err(UrlAppendError::DotSegment.into());
    }
    if username.contains('\\') {
        return Ok(false);
    }

    match api.legacy_get_user(username).map_api_error().await {
        // A 200 whose body is not a user still names a taken account.
        Ok(_) | Err(ApiError::Decode(_)) => Ok(true),
        Err(ApiError::Status { status, .. }) => Ok(status.is_success()),
        Err(err @ (ApiError::Transport(_) | ApiError::NotSent(_))) => Err(api_error(err)),
    }
}

/// Log in to the sync server at `address` as `username`.
#[instrument(level = "trace", skip_all, err)]
pub async fn login(
    address: &Url,
    username: &str,
    password: &SecretString,
    extra_headers: &HashMap<String, SecretString>,
) -> Result<types::LoginResponse> {
    let api = anonymous_api(address, extra_headers)?;

    let body = types::LoginRequest {
        username: username.to_owned(),
        password: password.clone().into(),
        totp_code: None,
    };
    let resp = api_call(api.legacy_login(&body)).await?;

    if !server_version_compatible(resp.headers())? {
        bail!("Could not login due to version mismatch");
    }

    Ok(resp.into_inner())
}

#[cfg(feature = "check-update")]
#[instrument(level = "trace", skip_all, err)]
pub async fn latest_version() -> Result<Version> {
    let http = reqwest::Client::builder()
        .default_headers(HeaderMap::from_iter([(
            reqwest::header::USER_AGENT,
            HeaderValue::from_static(atuin_domain::api::ATUIN_USER_AGENT),
        )]))
        .build()?;
    let api = atuin_api_client::Client::from_http(&crate::settings::DEFAULT_SYNC_URL, http)?;

    let index = api_call(api.get_index()).await?.into_inner();
    let version = Version::parse(index.version.as_str())?;

    Ok(version)
}

/// Whether the server whose answer carried `headers` is new enough to sync with.
///
/// Prints the mismatch when it is not.
///
/// # Errors
///
/// When `headers` do not carry a parseable `Atuin-Version`.
pub fn server_version_compatible(headers: &HeaderMap) -> Result<bool> {
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
            "Atuin version mismatch! In order to successfully sync, the server needs to run a \
             newer version of Atuin"
        );
        println!("Client: {ATUIN_CARGO_VERSION}");
        println!("Server: {version}");

        return Ok(false);
    }

    Ok(true)
}

/// `resp` if it answered 2xx, else the CLI's message for the failure.
#[instrument(level = "trace", skip_all, err)]
async fn handle_resp_error(resp: Response) -> Result<Response> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    Err(api_error(ApiError::from_response(resp).await))
}

/// Await `call`, turning its failure into the CLI's message for it.
async fn api_call<T>(call: impl MapApiError<ResponseValue<T>>) -> Result<ResponseValue<T>> {
    call.map_api_error().await.map_err(api_error)
}

/// The CLI's message for a failed call to the sync server or its object storage.
fn api_error(err: ApiError) -> eyre::Report {
    let ApiError::Status {
        status,
        url,
        reason,
        body,
        ..
    } = err
    else {
        return err.into();
    };
    match (status, reason.or(body)) {
        (StatusCode::SERVICE_UNAVAILABLE, _) => eyre!(
            "Service unavailable: check https://status.atuin.sh (or get in touch with your host)"
        ),
        (StatusCode::TOO_MANY_REQUESTS, _) => {
            eyre!("Rate limited; please wait before doing that again")
        }
        (status, Some(reason)) if status.is_client_error() => {
            eyre!("Invalid request to the service at {url}, {status} - {reason}.")
        }
        (status, Some(reason)) => eyre!(
            "There was an error with the atuin sync service at {url}, server error {status}: \
             {reason}.\nIf the problem persists, contact the host"
        ),
        (status, None) => eyre!(
            "There was an error with the atuin sync service at {url}, Status {status:?}.\nIf the \
             problem persists, contact the host"
        ),
    }
}

/// Build the capability reader for a sync server.
#[instrument(level = "trace", skip_all, err)]
pub fn caps_client(settings: &Settings) -> Result<Arc<CapClient>> {
    let auth_settings = Arc::new(settings.clone());

    let auth = AuthHeaderProvider::new(move || {
        let settings = auth_settings.clone();
        Box::pin(async move {
            settings.sync_auth_token().await.ok().and_then(|t| t.to_header_value().ok())
        })
    });

    Ok(CapClient::new(
        anonymous_api(&settings.sync_address, &settings.extra_headers)?.with_auth(auth),
    ))
}

/// Build an anonymous capability reader: every fetch sees the server-global
/// document. For contexts with no user auth in play (tests, tooling).
pub fn caps_client_anonymous(
    sync_addr: &Url,
    extra_headers: &HashMap<String, SecretString>,
) -> Result<Arc<CapClient>> {
    Ok(CapClient::new(anonymous_api(sync_addr, extra_headers)?))
}

/// A pending records download for one series, produced by [`Client::records`].
///
/// Owns a cheap [`Client`] clone and the [`RecordSeriesKey`] so the streams it produces are
/// `'static` (spawnable). Choose [`Self::one`] to fetch just the first record, or [`Self::stream`]
/// to stream the pages a plan covers.
#[must_use]
pub struct RecordsRequest {
    client: Client,
    series: RecordSeriesKey,
}

impl RecordsRequest {
    /// Fetch the first record of the series, if it has any.
    pub async fn one(self) -> Result<Option<Record<EncryptedData>>> {
        let pages = self.stream((0..1).chunks(1));
        futures::pin_mut!(pages);
        match pages.next().await {
            Some(page) => Ok(page?.into_iter().next()),
            None => Ok(None),
        }
    }

    /// Stream the record pages the `chunks` plan covers for this series.
    ///
    /// The plan lets us prefetch several pages in parallel at predictable offsets; if the server
    /// returns a short page mid-stream we fall back to a serial finish from real progress so no
    /// records are skipped.
    pub fn stream(
        self,
        chunks: Chunks<RecordIdx>,
    ) -> impl Stream<Item = Result<Vec<Record<EncryptedData>>>> + 'static {
        try_stream! {
            // Download the pages in parallel.
            let mut fetches = stream::iter(chunks)
                .map(|p| {
                    let width = p.end - p.start;
                    let fut = self.page(p);
                    async move { fut.await.map(|page| (width, page)) }
                })
                .buffered(MAX_RECORDS_CONCURRENT_DOWNLOAD);

            // Consume the stream and yield the values up.
            let mut progress = 0u64;
            let mut short_page = false;
            while let Some(result) = fetches.next().await {
                let (width, page) = result?;
                if page.is_empty() {
                    return;
                }

                let len = u64::conv(page.len());
                progress += len;
                yield page;

                // The server returned a short page; finish serially from here.
                if len < width {
                    short_page = true;
                    break;
                }
            }
            drop(fetches);

            // Download pages in series.
            //
            // A server could misbehave and return less data than we requested in the "parallel"
            // path.
            //
            // If it does, then we fall back to the serialized path, on the first misbehavior.
            if short_page {
                let recovery = stream::unfold(
                    (chunks.start() + progress, self),
                    move |(cursor, this)| async move {
                        if cursor >= chunks.end() {
                            return None;
                        }
                        let stop = (cursor + chunks.size().get()).min(chunks.end());
                        match this.page(cursor..stop).await {
                            Ok(page) if page.is_empty() => None,
                            Ok(page) => {
                                let next = cursor + u64::conv(page.len());
                                Some((Ok(page), (next, this)))
                            }
                            Err(e) => Some((Err(e), (chunks.end(), this))),
                        }
                    },
                );
                futures::pin_mut!(recovery);
                while let Some(p) = recovery.next().await {
                    yield p?;
                }
            }
        }
    }

    /// Fetch one page of `series`' records: `page.end - page.start` records starting at
    /// `page.start`.
    #[instrument(level = "trace", skip(self), err)]
    async fn page(&self, page: Range<RecordIdx>) -> Result<Vec<Record<EncryptedData>>> {
        let width = page.end - page.start;
        let records = api_call(self.client.api.get_next_records(
            &width,
            &self.series.host_id.0,
            Some(&page.start),
            self.series.tag.as_str(),
        ))
        .await?;
        Ok(records.into_inner())
    }
}

impl Client {
    #[instrument(level = "trace", skip_all, fields(connect_timeout, timeout), err)]
    pub fn new(
        sync_addr: impl Into<Arc<Url>>,
        auth: &AuthToken,
        connect_timeout: Duration,
        timeout: Duration,
        extra_headers: &HashMap<String, SecretString>,
        caps: Arc<CapClient>,
    ) -> Result<Self> {
        let sync_addr: Arc<Url> = sync_addr.into();

        let http = authenticated_http(auth, connect_timeout, timeout, extra_headers)?;
        let api = atuin_api_client::Client::from_http(&sync_addr, http)?
            .with_capabilities(Arc::clone(&caps), CapMismatch::Continue);

        Ok(Self {
            api,
            lfs_client: reqwest::Client::builder()
                .connect_timeout(connect_timeout)
                .timeout(timeout)
                .build()?,
            caps,
        })
    }

    /// The capability reader this client negotiates against, for capability-gated features to
    /// consult (e.g. `client.caps().get_server::<SomeCap>()`).
    #[must_use]
    pub fn caps(&self) -> &Arc<CapClient> {
        &self.caps
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn me(&self) -> Result<types::MeResponse> {
        Ok(api_call(self.api.get_me()).await?.into_inner())
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn delete_store(&self) -> Result<()> {
        api_call(self.api.delete_store()).await?;
        Ok(())
    }

    #[allow(clippy::ptr_arg, reason = "the generated post_records takes a &Vec")]
    #[instrument(level = "trace", skip_all, fields(count = records.len()), err)]
    pub async fn post_records(&self, records: &Vec<Record<EncryptedData>>) -> Result<()> {
        debug!("uploading {} records", records.len());
        api_call(self.api.post_records(records)).await?;
        Ok(())
    }

    /// Upload a stream of packfile blobs, transferring several concurrently.
    #[instrument(level = "trace", skip_all, err)]
    pub async fn upload_packfiles(
        &self,
        packfiles: impl Stream<Item = Result<PackedPackfile>>,
    ) -> Result<()> {
        let client = self.clone();
        packfiles
            .map(move |packfile| {
                let client = client.clone();
                async move {
                    let PackedPackfile {
                        manifest_id,
                        records,
                        blob,
                    } = packfile?;
                    client.upload_packfile(manifest_id, &records, blob).await
                }
            })
            .buffered(MAX_CONCURRENT_PACKFILE_UPLOADS)
            .try_collect::<()>()
            .await
    }

    /// Upload a single prepared packfile blob.
    #[instrument(level = "trace", skip_all, fields(id = ?manifest_id, count = record_ids.len()), err)]
    async fn upload_packfile(
        &self,
        manifest_id: RecordId,
        record_ids: &[RecordId],
        packfile: impl AsRef<[u8]> + Into<reqwest::Body>,
    ) -> Result<()> {
        let body = types::PackfileCreateRequest {
            manifest_id: manifest_id.0,
            packfile_size_bytes: u64::conv(packfile.as_ref().len()),
            records: record_ids.iter().map(|id| id.0).collect(),
        };
        let created = api_call(self.api.create_packfile(&body)).await?.into_inner();

        self.put_packfile(created.upload_url, packfile).await?;

        self.confirm_packfile(manifest_id).await?;

        Ok(())
    }

    /// Confirm a packfile body upload with the server.
    #[instrument(level = "trace", skip_all, fields(id = ?manifest_id), err)]
    async fn confirm_packfile(&self, manifest_id: RecordId) -> Result<()> {
        api_call(self.api.confirm_packfile(&manifest_id.0)).await?;
        Ok(())
    }

    /// Upload a packfile body to a presigned URL. Unauthenticated by design.
    #[instrument(level = "trace", skip_all, err)]
    async fn put_packfile(
        &self,
        upload_url: Url,
        packfile: impl Into<reqwest::Body>,
    ) -> Result<()> {
        // Not self.client: S3 rejects presigned requests that also carry an Authorization header.
        let resp = self
            .lfs_client
            .put(upload_url)
            .body(packfile)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        handle_resp_error(resp).await?;
        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = ?manifest_id), err)]
    async fn get_packfile_download_url(&self, manifest_id: RecordId) -> Result<Url> {
        let packfile = api_call(self.api.get_packfile(&manifest_id.0)).await?;
        Ok(packfile.into_inner().download_url)
    }

    /// Download the packfile for the given manifest id.
    #[instrument(level = "trace", skip_all, fields(id = ?manifest_id), err)]
    pub async fn download_packfile(&self, manifest_id: RecordId) -> Result<Vec<u8>> {
        let download_url = self.get_packfile_download_url(manifest_id).await?;
        let resp = self
            .lfs_client
            .get(download_url)
            .send()
            .instrument(tracing::trace_span!("lfs_download"))
            .await
            .map_err(reqwest::Error::without_url)?;
        let resp = handle_resp_error(resp).await?;
        Ok(resp.bytes().await.map_err(reqwest::Error::without_url)?.to_vec())
    }

    /// Build a records request for `series`.
    pub fn records(&self, series: &RecordSeriesKey) -> RecordsRequest {
        RecordsRequest {
            client: self.clone(),
            series: series.clone(),
        }
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn record_status(&self) -> Result<RecordStatus> {
        let resp = api_call(self.api.get_record_status()).await?;

        if !server_version_compatible(resp.headers())? {
            bail!("could not sync records due to version mismatch");
        }

        let index = resp.into_inner();

        debug!("got remote index {index:?}");

        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use rstest::*;

    use super::*;

    #[fixture]
    fn extra_headers() -> HashMap<String, SecretString> {
        let mut extra = HashMap::new();
        extra.insert("X-Auth-Token".to_string(), "secret".into());
        extra
    }

    #[rstest]
    fn extra_headers_map_parses_headers(extra_headers: HashMap<String, SecretString>) {
        let headers = extra_headers_map(&extra_headers).unwrap();
        let value = headers.get("x-auth-token").unwrap();
        assert_eq!(value, "secret");
        assert!(value.is_sensitive());
    }

    #[rstest]
    fn extra_headers_map_rejects_invalid_names() {
        let mut extra = HashMap::new();
        extra.insert("bad header".to_string(), "value".into());
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

    #[rstest]
    #[tokio::test]
    async fn cross_origin_redirects_refused_with_extra_headers(
        extra_headers: HashMap<String, SecretString>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // A different port on the same host is a different origin
        tokio::spawn(async move {
            serve_one(
                &listener,
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{}/\r\nContent-Length: \
                     0\r\nConnection: close\r\n\r\n",
                    port + 1
                ),
            )
            .await;
        });

        let client = client_builder(&extra_headers).build().unwrap();
        let err = client.get(format!("http://127.0.0.1:{port}/")).send().await.unwrap_err();

        assert!(err.is_redirect(), "expected a redirect policy error: {err:?}");
    }

    #[rstest]
    #[tokio::test]
    async fn same_origin_redirects_followed_with_extra_headers(
        extra_headers: HashMap<String, SecretString>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            serve_one(
                &listener,
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: \
                     http://127.0.0.1:{port}/ok\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
            )
            .await;
            serve_one(
                &listener,
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            )
            .await;
        });

        let client = client_builder(&extra_headers).build().unwrap();
        let resp = client.get(format!("http://127.0.0.1:{port}/")).send().await.unwrap();

        assert_eq!(resp.status(), 200);
        assert_eq!(resp.url().path(), "/ok");
    }

    #[rstest]
    #[tokio::test]
    async fn bootstrap_enables_packfiles_then_is_idempotent() {
        use atuin_domain::caps::{CapServer, CapabilitiesCap, PackfileCap};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // A server advertising PackfileCap; serve its exact wire document.
        let advertised = CapServer::new()
            .add(CapabilitiesCap { version: 1 })
            .unwrap()
            .add(PackfileCap {
                version: 1,
                record_count: 500,
            })
            .unwrap();

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_string(advertised.body().to_owned()))
            .expect(1)
            .mount(&server)
            .await;

        let addr: Url = server.uri().parse().unwrap();
        let caps = caps_client_anonymous(&addr, &HashMap::new()).unwrap();
        let client = Client::new(
            addr,
            &AuthToken::Token("t".into()),
            Duration::from_secs(30),
            Duration::from_secs(30),
            &HashMap::new(),
            caps,
        )
        .unwrap();

        // The client observes the server's advertised packfile cap; a second read stays warm
        // (the mock expects a single capabilities fetch).
        assert_eq!(
            client.caps().get_server::<PackfileCap>().await.unwrap(),
            Some(PackfileCap {
                version: 1,
                record_count: 500,
            })
        );
        assert_eq!(
            client.caps().get_server::<PackfileCap>().await.unwrap(),
            Some(PackfileCap {
                version: 1,
                record_count: 500,
            })
        );
    }
}

#[cfg(test)]
mod records_stream_tests {
    use atuin_common::range::RangeExt;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{EncryptedData, Host, HostId, Record, RecordSeriesKey, RecordTag};
    use futures::TryStreamExt;
    use rstest::rstest;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn history_record(host: HostId, idx: u64) -> Record<EncryptedData> {
        Record::builder()
            .host(Host::new(host))
            .version("v1".into())
            .tag(RecordTag::History)
            .idx(idx)
            .data(EncryptedData {
                raw: format!("r{idx}"),
                cek: String::new(),
            })
            .build()
    }

    fn mock_client(addr: &Url) -> Client {
        let caps = caps_client_anonymous(addr, &HashMap::new()).unwrap();
        Client::new(
            addr.clone(),
            &AuthToken::Token("t".into()),
            Duration::from_secs(30),
            Duration::from_secs(30),
            &HashMap::new(),
            caps,
        )
        .unwrap()
    }

    /// Serve `records` in pages of `serve_size`, keyed on the `start` query param
    /// (`idx >= start ORDER BY idx ASC LIMIT count`, dense). `serve_size` may be smaller than the
    /// client's page size to emulate a server that clamps `count`. Any `start` past the end -> empty.
    async fn mount_paged(
        server: &MockServer,
        records: &[Record<EncryptedData>],
        serve_size: usize,
    ) {
        for start in (0..records.len()).step_by(serve_size) {
            let end = (start + serve_size).min(records.len());
            Mock::given(method("GET"))
                .and(path("/api/v0/record/next"))
                .and(query_param("start", start.to_string()))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(records[start..end].to_vec()),
                )
                .mount(server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(Vec::<Record<EncryptedData>>::new()),
            )
            .mount(server)
            .await;
    }

    async fn collect_idxs(
        stream: impl Stream<Item = Result<Vec<Record<EncryptedData>>>>,
    ) -> Vec<u64> {
        let pages: Vec<Vec<Record<EncryptedData>>> = stream.try_collect().await.unwrap();
        pages.into_iter().flatten().map(|r| r.idx).collect()
    }

    /// The fast path predicts offsets (`start + i * page_size`) and pipelines the fetches; every
    /// page must still be reassembled in idx order.
    #[rstest]
    #[tokio::test]
    async fn records_reassembles_pages_in_order() {
        let host = HostId(uuid_v7());
        let all: Vec<_> = (0..5).map(|i| history_record(host, i)).collect();

        let server = MockServer::start().await;
        // page_size 2 -> offsets 0, 2, 4; the last page (idx 4) is a short tail.
        mount_paged(&server, &all, 2).await;

        let addr: Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        let idxs = collect_idxs(
            client
                .records(&RecordSeriesKey::new(host, RecordTag::History))
                .stream((0..5).chunks(2)),
        )
        .await;
        assert_eq!(idxs, vec![0, 1, 2, 3, 4]);
    }

    /// GUARD: a server that clamps `count` below the client's `page_size` returns a short page
    /// *mid-stream*. The predicted offsets past it would skip records, so the stream must detect the
    /// short page and finish serially from the real progress -- losing nothing.
    #[rstest]
    #[tokio::test]
    async fn records_recovers_from_a_short_midstream_page() {
        let host = HostId(uuid_v7());
        let all: Vec<_> = (0..6).map(|i| history_record(host, i)).collect();

        let server = MockServer::start().await;
        // Client asks for page_size 4, but the server only ever returns 2 (a clamp). Predicted
        // offsets would be 0 and 4, skipping idx 2..4 -- the guard must recover them.
        mount_paged(&server, &all, 2).await;

        let addr: Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        let idxs = collect_idxs(
            client
                .records(&RecordSeriesKey::new(host, RecordTag::History))
                .stream((0..6).chunks(4)),
        )
        .await;
        assert_eq!(idxs, vec![0, 1, 2, 3, 4, 5], "a short mid-stream page must not skip records");
    }

    #[rstest]
    #[tokio::test]
    async fn records_yields_nothing_when_server_is_empty() {
        let host = HostId(uuid_v7());

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v0/record/next"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(Vec::<Record<EncryptedData>>::new()),
            )
            .mount(&server)
            .await;

        let addr: Url = server.uri().parse().unwrap();
        let client = mock_client(&addr);

        let idxs = collect_idxs(
            client
                .records(&RecordSeriesKey::new(host, RecordTag::History))
                .stream((0..10).chunks(4)),
        )
        .await;
        assert!(idxs.is_empty(), "an empty server must yield no records");
    }
}
