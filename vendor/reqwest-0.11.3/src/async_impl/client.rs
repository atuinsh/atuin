#[cfg(any(feature = "native-tls", feature = "__rustls",))]
use std::any::Any;
use std::convert::TryInto;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, str};

use bytes::Bytes;
use http::header::{
    Entry, HeaderMap, HeaderValue, ACCEPT, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH,
    CONTENT_TYPE, LOCATION, PROXY_AUTHORIZATION, RANGE, REFERER, TRANSFER_ENCODING, USER_AGENT,
};
use http::uri::Scheme;
use http::Uri;
use hyper::client::ResponseFuture;
#[cfg(feature = "native-tls-crate")]
use native_tls_crate::TlsConnector;
use pin_project_lite::pin_project;
#[cfg(feature = "rustls-tls-native-roots")]
use rustls::RootCertStore;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::time::Sleep;

use log::debug;

use super::decoder::Accepts;
use super::request::{Request, RequestBuilder};
use super::response::Response;
use super::Body;
use crate::connect::{Connector, HttpConnector};
#[cfg(feature = "cookies")]
use crate::cookie;
use crate::error;
use crate::into_url::{expect_uri, try_uri};
use crate::redirect::{self, remove_sensitive_headers};
#[cfg(feature = "__tls")]
use crate::tls::TlsBackend;
#[cfg(feature = "__tls")]
use crate::Certificate;
#[cfg(any(feature = "native-tls", feature = "__rustls"))]
use crate::Identity;
use crate::{IntoUrl, Method, Proxy, StatusCode, Url};

/// An asynchronous `Client` to make Requests with.
///
/// The Client has various configuration values to tweak, but the defaults
/// are set to what is usually the most commonly desired value. To configure a
/// `Client`, use `Client::builder()`.
///
/// The `Client` holds a connection pool internally, so it is advised that
/// you create one and **reuse** it.
///
/// You do **not** have to wrap the `Client` it in an [`Rc`] or [`Arc`] to **reuse** it,
/// because it already uses an [`Arc`] internally.
///
/// [`Rc`]: std::rc::Rc
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientRef>,
}

/// A `ClientBuilder` can be used to create a `Client` with  custom configuration.
#[must_use]
pub struct ClientBuilder {
    config: Config,
}

struct Config {
    // NOTE: When adding a new field, update `fmt::Debug for ClientBuilder`
    accepts: Accepts,
    headers: HeaderMap,
    #[cfg(feature = "native-tls")]
    hostname_verification: bool,
    #[cfg(feature = "__tls")]
    certs_verification: bool,
    connect_timeout: Option<Duration>,
    connection_verbose: bool,
    pool_idle_timeout: Option<Duration>,
    pool_max_idle_per_host: usize,
    tcp_keepalive: Option<Duration>,
    #[cfg(any(feature = "native-tls", feature = "__rustls"))]
    identity: Option<Identity>,
    proxies: Vec<Proxy>,
    auto_sys_proxy: bool,
    redirect_policy: redirect::Policy,
    referer: bool,
    timeout: Option<Duration>,
    #[cfg(feature = "__tls")]
    root_certs: Vec<Certificate>,
    #[cfg(feature = "__tls")]
    tls_built_in_root_certs: bool,
    #[cfg(feature = "__tls")]
    tls: TlsBackend,
    http2_only: bool,
    http1_title_case_headers: bool,
    http2_initial_stream_window_size: Option<u32>,
    http2_initial_connection_window_size: Option<u32>,
    http2_adaptive_window: bool,
    http2_max_frame_size: Option<u32>,
    local_address: Option<IpAddr>,
    nodelay: bool,
    #[cfg(feature = "cookies")]
    cookie_store: Option<Arc<dyn cookie::CookieStore>>,
    trust_dns: bool,
    error: Option<crate::Error>,
    https_only: bool,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientBuilder {
    /// Constructs a new `ClientBuilder`.
    ///
    /// This is the same as `Client::builder()`.
    pub fn new() -> ClientBuilder {
        let mut headers: HeaderMap<HeaderValue> = HeaderMap::with_capacity(2);
        headers.insert(ACCEPT, HeaderValue::from_static("*/*"));

        ClientBuilder {
            config: Config {
                error: None,
                accepts: Accepts::default(),
                headers,
                #[cfg(feature = "native-tls")]
                hostname_verification: true,
                #[cfg(feature = "__tls")]
                certs_verification: true,
                connect_timeout: None,
                connection_verbose: false,
                pool_idle_timeout: Some(Duration::from_secs(90)),
                pool_max_idle_per_host: std::usize::MAX,
                // TODO: Re-enable default duration once hyper's HttpConnector is fixed
                // to no longer error when an option fails.
                tcp_keepalive: None, //Some(Duration::from_secs(60)),
                proxies: Vec::new(),
                auto_sys_proxy: true,
                redirect_policy: redirect::Policy::default(),
                referer: true,
                timeout: None,
                #[cfg(feature = "__tls")]
                root_certs: Vec::new(),
                #[cfg(feature = "__tls")]
                tls_built_in_root_certs: true,
                #[cfg(any(feature = "native-tls", feature = "__rustls"))]
                identity: None,
                #[cfg(feature = "__tls")]
                tls: TlsBackend::default(),
                http2_only: false,
                http1_title_case_headers: false,
                http2_initial_stream_window_size: None,
                http2_initial_connection_window_size: None,
                http2_adaptive_window: false,
                http2_max_frame_size: None,
                local_address: None,
                nodelay: true,
                trust_dns: cfg!(feature = "trust-dns"),
                #[cfg(feature = "cookies")]
                cookie_store: None,
                https_only: false,
            },
        }
    }

    /// Returns a `Client` that uses this `ClientBuilder` configuration.
    ///
    /// # Errors
    ///
    /// This method fails if TLS backend cannot be initialized, or the resolver
    /// cannot load the system configuration.
    pub fn build(self) -> crate::Result<Client> {
        let config = self.config;

        if let Some(err) = config.error {
            return Err(err);
        }

        let mut proxies = config.proxies;
        if config.auto_sys_proxy {
            proxies.push(Proxy::system());
        }
        let proxies = Arc::new(proxies);

        let mut connector = {
            #[cfg(feature = "__tls")]
            fn user_agent(headers: &HeaderMap) -> Option<HeaderValue> {
                headers.get(USER_AGENT).cloned()
            }

            let http = match config.trust_dns {
                false => HttpConnector::new_gai(),
                #[cfg(feature = "trust-dns")]
                true => HttpConnector::new_trust_dns()?,
                #[cfg(not(feature = "trust-dns"))]
                true => unreachable!("trust-dns shouldn't be enabled unless the feature is"),
            };

            #[cfg(feature = "__tls")]
            match config.tls {
                #[cfg(feature = "default-tls")]
                TlsBackend::Default => {
                    let mut tls = TlsConnector::builder();

                    #[cfg(feature = "native-tls")]
                    {
                        tls.danger_accept_invalid_hostnames(!config.hostname_verification);
                    }

                    tls.danger_accept_invalid_certs(!config.certs_verification);

                    tls.disable_built_in_roots(!config.tls_built_in_root_certs);

                    for cert in config.root_certs {
                        cert.add_to_native_tls(&mut tls);
                    }

                    #[cfg(feature = "native-tls")]
                    {
                        if let Some(id) = config.identity {
                            id.add_to_native_tls(&mut tls)?;
                        }
                    }

                    Connector::new_default_tls(
                        http,
                        tls,
                        proxies.clone(),
                        user_agent(&config.headers),
                        config.local_address,
                        config.nodelay,
                    )?
                }
                #[cfg(feature = "native-tls")]
                TlsBackend::BuiltNativeTls(conn) => Connector::from_built_default_tls(
                    http,
                    conn,
                    proxies.clone(),
                    user_agent(&config.headers),
                    config.local_address,
                    config.nodelay,
                ),
                #[cfg(feature = "__rustls")]
                TlsBackend::BuiltRustls(conn) => Connector::new_rustls_tls(
                    http,
                    conn,
                    proxies.clone(),
                    user_agent(&config.headers),
                    config.local_address,
                    config.nodelay,
                ),
                #[cfg(feature = "__rustls")]
                TlsBackend::Rustls => {
                    use crate::tls::NoVerifier;

                    let mut tls = rustls::ClientConfig::new();
                    if config.http2_only {
                        tls.set_protocols(&["h2".into()]);
                    } else {
                        tls.set_protocols(&["h2".into(), "http/1.1".into()]);
                    }
                    #[cfg(feature = "rustls-tls-webpki-roots")]
                    if config.tls_built_in_root_certs {
                        tls.root_store
                            .add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
                    }
                    #[cfg(feature = "rustls-tls-native-roots")]
                    if config.tls_built_in_root_certs {
                        let roots_slice = NATIVE_ROOTS.as_ref().unwrap().roots.as_slice();
                        tls.root_store.roots.extend_from_slice(roots_slice);
                    }

                    if !config.certs_verification {
                        tls.dangerous()
                            .set_certificate_verifier(Arc::new(NoVerifier));
                    }

                    for cert in config.root_certs {
                        cert.add_to_rustls(&mut tls)?;
                    }

                    if let Some(id) = config.identity {
                        id.add_to_rustls(&mut tls)?;
                    }

                    Connector::new_rustls_tls(
                        http,
                        tls,
                        proxies.clone(),
                        user_agent(&config.headers),
                        config.local_address,
                        config.nodelay,
                    )
                }
                #[cfg(any(feature = "native-tls", feature = "__rustls",))]
                TlsBackend::UnknownPreconfigured => {
                    return Err(crate::error::builder(
                        "Unknown TLS backend passed to `use_preconfigured_tls`",
                    ));
                }
            }

            #[cfg(not(feature = "__tls"))]
            Connector::new(http, proxies.clone(), config.local_address, config.nodelay)
        };

        connector.set_timeout(config.connect_timeout);
        connector.set_verbose(config.connection_verbose);

        let mut builder = hyper::Client::builder();
        if config.http2_only {
            builder.http2_only(true);
        }

        if let Some(http2_initial_stream_window_size) = config.http2_initial_stream_window_size {
            builder.http2_initial_stream_window_size(http2_initial_stream_window_size);
        }
        if let Some(http2_initial_connection_window_size) =
            config.http2_initial_connection_window_size
        {
            builder.http2_initial_connection_window_size(http2_initial_connection_window_size);
        }
        if config.http2_adaptive_window {
            builder.http2_adaptive_window(true);
        }
        if let Some(http2_max_frame_size) = config.http2_max_frame_size {
            builder.http2_max_frame_size(http2_max_frame_size);
        }

        builder.pool_idle_timeout(config.pool_idle_timeout);
        builder.pool_max_idle_per_host(config.pool_max_idle_per_host);
        connector.set_keepalive(config.tcp_keepalive);

        if config.http1_title_case_headers {
            builder.http1_title_case_headers(true);
        }

        let hyper_client = builder.build(connector);

        let proxies_maybe_http_auth = proxies.iter().any(|p| p.maybe_has_http_auth());

        Ok(Client {
            inner: Arc::new(ClientRef {
                accepts: config.accepts,
                #[cfg(feature = "cookies")]
                cookie_store: config.cookie_store,
                hyper: hyper_client,
                headers: config.headers,
                redirect_policy: config.redirect_policy,
                referer: config.referer,
                request_timeout: config.timeout,
                proxies,
                proxies_maybe_http_auth,
                https_only: config.https_only,
            }),
        })
    }

    // Higher-level options

    /// Sets the `User-Agent` header to be used by this client.
    ///
    /// # Example
    ///
    /// ```rust
    /// # async fn doc() -> Result<(), reqwest::Error> {
    /// // Name your user agent after your app?
    /// static APP_USER_AGENT: &str = concat!(
    ///     env!("CARGO_PKG_NAME"),
    ///     "/",
    ///     env!("CARGO_PKG_VERSION"),
    /// );
    ///
    /// let client = reqwest::Client::builder()
    ///     .user_agent(APP_USER_AGENT)
    ///     .build()?;
    /// let res = client.get("https://www.rust-lang.org").send().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn user_agent<V>(mut self, value: V) -> ClientBuilder
    where
        V: TryInto<HeaderValue>,
        V::Error: Into<http::Error>,
    {
        match value.try_into() {
            Ok(value) => {
                self.config.headers.insert(USER_AGENT, value);
            }
            Err(e) => {
                self.config.error = Some(crate::error::builder(e.into()));
            }
        };
        self
    }
    /// Sets the default headers for every request.
    ///
    /// # Example
    ///
    /// ```rust
    /// use reqwest::header;
    /// # async fn doc() -> Result<(), reqwest::Error> {
    /// let mut headers = header::HeaderMap::new();
    /// headers.insert("X-MY-HEADER", header::HeaderValue::from_static("value"));
    ///
    /// // Consider marking security-sensitive headers with `set_sensitive`.
    /// let mut auth_value = header::HeaderValue::from_static("secret");
    /// auth_value.set_sensitive(true);
    /// headers.insert(header::AUTHORIZATION, auth_value);
    ///
    /// // get a client builder
    /// let client = reqwest::Client::builder()
    ///     .default_headers(headers)
    ///     .build()?;
    /// let res = client.get("https://www.rust-lang.org").send().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Override the default headers:
    ///
    /// ```rust
    /// use reqwest::header;
    /// # async fn doc() -> Result<(), reqwest::Error> {
    /// let mut headers = header::HeaderMap::new();
    /// headers.insert("X-MY-HEADER", header::HeaderValue::from_static("value"));
    ///
    /// // get a client builder
    /// let client = reqwest::Client::builder()
    ///     .default_headers(headers)
    ///     .build()?;
    /// let res = client
    ///     .get("https://www.rust-lang.org")
    ///     .header("X-MY-HEADER", "new_value")
    ///     .send()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn default_headers(mut self, headers: HeaderMap) -> ClientBuilder {
        for (key, value) in headers.iter() {
            self.config.headers.insert(key, value.clone());
        }
        self
    }

    /// Enable a persistent cookie store for the client.
    ///
    /// Cookies received in responses will be preserved and included in
    /// additional requests.
    ///
    /// By default, no cookie store is used.
    ///
    /// # Optional
    ///
    /// This requires the optional `cookies` feature to be enabled.
    #[cfg(feature = "cookies")]
    #[cfg_attr(docsrs, doc(cfg(feature = "cookies")))]
    pub fn cookie_store(mut self, enable: bool) -> ClientBuilder {
        if enable {
            self.cookie_provider(Arc::new(cookie::Jar::default()))
        } else {
            self.config.cookie_store = None;
            self
        }
    }

    /// Set the persistent cookie store for the client.
    ///
    /// Cookies received in responses will be passed to this store, and
    /// additional requests will query this store for cookies.
    ///
    /// By default, no cookie store is used.
    ///
    /// # Optional
    ///
    /// This requires the optional `cookies` feature to be enabled.
    #[cfg(feature = "cookies")]
    #[cfg_attr(docsrs, doc(cfg(feature = "cookies")))]
    pub fn cookie_provider<C: cookie::CookieStore + 'static>(
        mut self,
        cookie_store: Arc<C>,
    ) -> ClientBuilder {
        self.config.cookie_store = Some(cookie_store as _);
        self
    }

    /// Enable auto gzip decompression by checking the `Content-Encoding` response header.
    ///
    /// If auto gzip decompression is turned on:
    ///
    /// - When sending a request and if the request's headers do not already contain
    ///   an `Accept-Encoding` **and** `Range` values, the `Accept-Encoding` header is set to `gzip`.
    ///   The request body is **not** automatically compressed.
    /// - When receiving a response, if it's headers contain a `Content-Encoding` value that
    ///   equals to `gzip`, both values `Content-Encoding` and `Content-Length` are removed from the
    ///   headers' set. The response body is automatically decompressed.
    ///
    /// If the `gzip` feature is turned on, the default option is enabled.
    ///
    /// # Optional
    ///
    /// This requires the optional `gzip` feature to be enabled
    #[cfg(feature = "gzip")]
    #[cfg_attr(docsrs, doc(cfg(feature = "gzip")))]
    pub fn gzip(mut self, enable: bool) -> ClientBuilder {
        self.config.accepts.gzip = enable;
        self
    }

    /// Enable auto brotli decompression by checking the `Content-Encoding` response header.
    ///
    /// If auto brotli decompression is turned on:
    ///
    /// - When sending a request and if the request's headers do not already contain
    ///   an `Accept-Encoding` **and** `Range` values, the `Accept-Encoding` header is set to `br`.
    ///   The request body is **not** automatically compressed.
    /// - When receiving a response, if it's headers contain a `Content-Encoding` value that
    ///   equals to `br`, both values `Content-Encoding` and `Content-Length` are removed from the
    ///   headers' set. The response body is automatically decompressed.
    ///
    /// If the `brotli` feature is turned on, the default option is enabled.
    ///
    /// # Optional
    ///
    /// This requires the optional `brotli` feature to be enabled
    #[cfg(feature = "brotli")]
    #[cfg_attr(docsrs, doc(cfg(feature = "brotli")))]
    pub fn brotli(mut self, enable: bool) -> ClientBuilder {
        self.config.accepts.brotli = enable;
        self
    }

    /// Disable auto response body gzip decompression.
    ///
    /// This method exists even if the optional `gzip` feature is not enabled.
    /// This can be used to ensure a `Client` doesn't use gzip decompression
    /// even if another dependency were to enable the optional `gzip` feature.
    pub fn no_gzip(self) -> ClientBuilder {
        #[cfg(feature = "gzip")]
        {
            self.gzip(false)
        }

        #[cfg(not(feature = "gzip"))]
        {
            self
        }
    }

    /// Disable auto response body brotli decompression.
    ///
    /// This method exists even if the optional `brotli` feature is not enabled.
    /// This can be used to ensure a `Client` doesn't use brotli decompression
    /// even if another dependency were to enable the optional `brotli` feature.
    pub fn no_brotli(self) -> ClientBuilder {
        #[cfg(feature = "brotli")]
        {
            self.brotli(false)
        }

        #[cfg(not(feature = "brotli"))]
        {
            self
        }
    }

    // Redirect options

    /// Set a `RedirectPolicy` for this client.
    ///
    /// Default will follow redirects up to a maximum of 10.
    pub fn redirect(mut self, policy: redirect::Policy) -> ClientBuilder {
        self.config.redirect_policy = policy;
        self
    }

    /// Enable or disable automatic setting of the `Referer` header.
    ///
    /// Default is `true`.
    pub fn referer(mut self, enable: bool) -> ClientBuilder {
        self.config.referer = enable;
        self
    }

    // Proxy options

    /// Add a `Proxy` to the list of proxies the `Client` will use.
    ///
    /// # Note
    ///
    /// Adding a proxy will disable the automatic usage of the "system" proxy.
    pub fn proxy(mut self, proxy: Proxy) -> ClientBuilder {
        self.config.proxies.push(proxy);
        self.config.auto_sys_proxy = false;
        self
    }

    /// Clear all `Proxies`, so `Client` will use no proxy anymore.
    ///
    /// This also disables the automatic usage of the "system" proxy.
    pub fn no_proxy(mut self) -> ClientBuilder {
        self.config.proxies.clear();
        self.config.auto_sys_proxy = false;
        self
    }

    // Timeout options

    /// Enables a request timeout.
    ///
    /// The timeout is applied from when the request starts connecting until the
    /// response body has finished.
    ///
    /// Default is no timeout.
    pub fn timeout(mut self, timeout: Duration) -> ClientBuilder {
        self.config.timeout = Some(timeout);
        self
    }

    /// Set a timeout for only the connect phase of a `Client`.
    ///
    /// Default is `None`.
    ///
    /// # Note
    ///
    /// This **requires** the futures be executed in a tokio runtime with
    /// a tokio timer enabled.
    pub fn connect_timeout(mut self, timeout: Duration) -> ClientBuilder {
        self.config.connect_timeout = Some(timeout);
        self
    }

    /// Set whether connections should emit verbose logs.
    ///
    /// Enabling this option will emit [log][] messages at the `TRACE` level
    /// for read and write operations on connections.
    ///
    /// [log]: https://crates.io/crates/log
    pub fn connection_verbose(mut self, verbose: bool) -> ClientBuilder {
        self.config.connection_verbose = verbose;
        self
    }

    // HTTP options

    /// Set an optional timeout for idle sockets being kept-alive.
    ///
    /// Pass `None` to disable timeout.
    ///
    /// Default is 90 seconds.
    pub fn pool_idle_timeout<D>(mut self, val: D) -> ClientBuilder
    where
        D: Into<Option<Duration>>,
    {
        self.config.pool_idle_timeout = val.into();
        self
    }

    /// Sets the maximum idle connection per host allowed in the pool.
    pub fn pool_max_idle_per_host(mut self, max: usize) -> ClientBuilder {
        self.config.pool_max_idle_per_host = max;
        self
    }

    /// Enable case sensitive headers.
    pub fn http1_title_case_headers(mut self) -> ClientBuilder {
        self.config.http1_title_case_headers = true;
        self
    }

    /// Only use HTTP/2.
    pub fn http2_prior_knowledge(mut self) -> ClientBuilder {
        self.config.http2_only = true;
        self
    }

    /// Sets the `SETTINGS_INITIAL_WINDOW_SIZE` option for HTTP2 stream-level flow control.
    ///
    /// Default is currently 65,535 but may change internally to optimize for common uses.
    pub fn http2_initial_stream_window_size(mut self, sz: impl Into<Option<u32>>) -> ClientBuilder {
        self.config.http2_initial_stream_window_size = sz.into();
        self
    }

    /// Sets the max connection-level flow control for HTTP2
    ///
    /// Default is currently 65,535 but may change internally to optimize for common uses.
    pub fn http2_initial_connection_window_size(
        mut self,
        sz: impl Into<Option<u32>>,
    ) -> ClientBuilder {
        self.config.http2_initial_connection_window_size = sz.into();
        self
    }

    /// Sets whether to use an adaptive flow control.
    ///
    /// Enabling this will override the limits set in `http2_initial_stream_window_size` and
    /// `http2_initial_connection_window_size`.
    pub fn http2_adaptive_window(mut self, enabled: bool) -> ClientBuilder {
        self.config.http2_adaptive_window = enabled;
        self
    }

    /// Sets the maximum frame size to use for HTTP2.
    ///
    /// Default is currently 16,384 but may change internally to optimize for common uses.
    pub fn http2_max_frame_size(mut self, sz: impl Into<Option<u32>>) -> ClientBuilder {
        self.config.http2_max_frame_size = sz.into();
        self
    }

    // TCP options

    /// Set whether sockets have `SO_NODELAY` enabled.
    ///
    /// Default is `true`.
    pub fn tcp_nodelay(mut self, enabled: bool) -> ClientBuilder {
        self.config.nodelay = enabled;
        self
    }

    /// Bind to a local IP Address.
    ///
    /// # Example
    ///
    /// ```
    /// use std::net::IpAddr;
    /// let local_addr = IpAddr::from([12, 4, 1, 8]);
    /// let client = reqwest::Client::builder()
    ///     .local_address(local_addr)
    ///     .build().unwrap();
    /// ```
    pub fn local_address<T>(mut self, addr: T) -> ClientBuilder
    where
        T: Into<Option<IpAddr>>,
    {
        self.config.local_address = addr.into();
        self
    }

    /// Set that all sockets have `SO_KEEPALIVE` set with the supplied duration.
    ///
    /// If `None`, the option will not be set.
    pub fn tcp_keepalive<D>(mut self, val: D) -> ClientBuilder
    where
        D: Into<Option<Duration>>,
    {
        self.config.tcp_keepalive = val.into();
        self
    }

    // TLS options

    /// Add a custom root certificate.
    ///
    /// This can be used to connect to a server that has a self-signed
    /// certificate for example.
    ///
    /// # Optional
    ///
    /// This requires the optional `default-tls`, `native-tls`, or `rustls-tls(-...)`
    /// feature to be enabled.
    #[cfg(feature = "__tls")]
    #[cfg_attr(
        docsrs,
        doc(cfg(any(
            feature = "default-tls",
            feature = "native-tls",
            feature = "rustls-tls"
        )))
    )]
    pub fn add_root_certificate(mut self, cert: Certificate) -> ClientBuilder {
        self.config.root_certs.push(cert);
        self
    }

    /// Controls the use of built-in/preloaded certificates during certificate validation.
    ///
    /// Defaults to `true` -- built-in system certs will be used.
    ///
    /// # Optional
    ///
    /// This requires the optional `default-tls`, `native-tls`, or `rustls-tls(-...)`
    /// feature to be enabled.
    #[cfg(feature = "__tls")]
    #[cfg_attr(
        docsrs,
        doc(cfg(any(
            feature = "default-tls",
            feature = "native-tls",
            feature = "rustls-tls"
        )))
    )]
    pub fn tls_built_in_root_certs(mut self, tls_built_in_root_certs: bool) -> ClientBuilder {
        self.config.tls_built_in_root_certs = tls_built_in_root_certs;
        self
    }

    /// Sets the identity to be used for client certificate authentication.
    ///
    /// # Optional
    ///
    /// This requires the optional `native-tls` or `rustls-tls(-...)` feature to be
    /// enabled.
    #[cfg(any(feature = "native-tls", feature = "__rustls"))]
    #[cfg_attr(docsrs, doc(cfg(any(feature = "native-tls", feature = "rustls-tls"))))]
    pub fn identity(mut self, identity: Identity) -> ClientBuilder {
        self.config.identity = Some(identity);
        self
    }

    /// Controls the use of hostname verification.
    ///
    /// Defaults to `false`.
    ///
    /// # Warning
    ///
    /// You should think very carefully before you use this method. If
    /// hostname verification is not used, any valid certificate for any
    /// site will be trusted for use from any other. This introduces a
    /// significant vulnerability to man-in-the-middle attacks.
    ///
    /// # Optional
    ///
    /// This requires the optional `native-tls` feature to be enabled.
    #[cfg(feature = "native-tls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "native-tls")))]
    pub fn danger_accept_invalid_hostnames(
        mut self,
        accept_invalid_hostname: bool,
    ) -> ClientBuilder {
        self.config.hostname_verification = !accept_invalid_hostname;
        self
    }

    /// Controls the use of certificate validation.
    ///
    /// Defaults to `false`.
    ///
    /// # Warning
    ///
    /// You should think very carefully before using this method. If
    /// invalid certificates are trusted, *any* certificate for *any* site
    /// will be trusted for use. This includes expired certificates. This
    /// introduces significant vulnerabilities, and should only be used
    /// as a last resort.
    ///
    /// # Optional
    ///
    /// This requires the optional `default-tls`, `native-tls`, or `rustls-tls(-...)`
    /// feature to be enabled.
    #[cfg(feature = "__tls")]
    #[cfg_attr(
        docsrs,
        doc(cfg(any(
            feature = "default-tls",
            feature = "native-tls",
            feature = "rustls-tls"
        )))
    )]
    pub fn danger_accept_invalid_certs(mut self, accept_invalid_certs: bool) -> ClientBuilder {
        self.config.certs_verification = !accept_invalid_certs;
        self
    }

    /// Force using the native TLS backend.
    ///
    /// Since multiple TLS backends can be optionally enabled, this option will
    /// force the `native-tls` backend to be used for this `Client`.
    ///
    /// # Optional
    ///
    /// This requires the optional `native-tls` feature to be enabled.
    #[cfg(feature = "native-tls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "native-tls")))]
    pub fn use_native_tls(mut self) -> ClientBuilder {
        self.config.tls = TlsBackend::Default;
        self
    }

    /// Force using the Rustls TLS backend.
    ///
    /// Since multiple TLS backends can be optionally enabled, this option will
    /// force the `rustls` backend to be used for this `Client`.
    ///
    /// # Optional
    ///
    /// This requires the optional `rustls-tls(-...)` feature to be enabled.
    #[cfg(feature = "__rustls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rustls-tls")))]
    pub fn use_rustls_tls(mut self) -> ClientBuilder {
        self.config.tls = TlsBackend::Rustls;
        self
    }

    /// Use a preconfigured TLS backend.
    ///
    /// If the passed `Any` argument is not a TLS backend that reqwest
    /// understands, the `ClientBuilder` will error when calling `build`.
    ///
    /// # Advanced
    ///
    /// This is an advanced option, and can be somewhat brittle. Usage requires
    /// keeping the preconfigured TLS argument version in sync with reqwest,
    /// since version mismatches will result in an "unknown" TLS backend.
    ///
    /// If possible, it's preferable to use the methods on `ClientBuilder`
    /// to configure reqwest's TLS.
    ///
    /// # Optional
    ///
    /// This requires one of the optional features `native-tls` or
    /// `rustls-tls(-...)` to be enabled.
    #[cfg(any(feature = "native-tls", feature = "__rustls",))]
    #[cfg_attr(docsrs, doc(cfg(any(feature = "native-tls", feature = "rustls-tls"))))]
    pub fn use_preconfigured_tls(mut self, tls: impl Any) -> ClientBuilder {
        let mut tls = Some(tls);
        #[cfg(feature = "native-tls")]
        {
            if let Some(conn) =
                (&mut tls as &mut dyn Any).downcast_mut::<Option<native_tls_crate::TlsConnector>>()
            {
                let tls = conn.take().expect("is definitely Some");
                let tls = crate::tls::TlsBackend::BuiltNativeTls(tls);
                self.config.tls = tls;
                return self;
            }
        }
        #[cfg(feature = "__rustls")]
        {
            if let Some(conn) =
                (&mut tls as &mut dyn Any).downcast_mut::<Option<rustls::ClientConfig>>()
            {
                let tls = conn.take().expect("is definitely Some");
                let tls = crate::tls::TlsBackend::BuiltRustls(tls);
                self.config.tls = tls;
                return self;
            }
        }

        // Otherwise, we don't recognize the TLS backend!
        self.config.tls = crate::tls::TlsBackend::UnknownPreconfigured;
        self
    }

    /// Enables the [trust-dns](trust_dns_resolver) async resolver instead of a default threadpool using `getaddrinfo`.
    ///
    /// If the `trust-dns` feature is turned on, the default option is enabled.
    ///
    /// # Optional
    ///
    /// This requires the optional `trust-dns` feature to be enabled
    #[cfg(feature = "trust-dns")]
    #[cfg_attr(docsrs, doc(cfg(feature = "trust-dns")))]
    pub fn trust_dns(mut self, enable: bool) -> ClientBuilder {
        self.config.trust_dns = enable;
        self
    }

    /// Disables the trust-dns async resolver.
    ///
    /// This method exists even if the optional `trust-dns` feature is not enabled.
    /// This can be used to ensure a `Client` doesn't use the trust-dns async resolver
    /// even if another dependency were to enable the optional `trust-dns` feature.
    pub fn no_trust_dns(self) -> ClientBuilder {
        #[cfg(feature = "trust-dns")]
        {
            self.trust_dns(false)
        }

        #[cfg(not(feature = "trust-dns"))]
        {
            self
        }
    }

    /// Restrict the Client to be used with HTTPS only requests.
    ///
    /// Defaults to false.
    pub fn https_only(mut self, enabled: bool) -> ClientBuilder {
        self.config.https_only = enabled;
        self
    }
}

type HyperClient = hyper::Client<Connector, super::body::ImplStream>;

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Constructs a new `Client`.
    ///
    /// # Panics
    ///
    /// This method panics if TLS backend cannot initialized, or the resolver
    /// cannot load the system configuration.
    ///
    /// Use `Client::builder()` if you wish to handle the failure as an `Error`
    /// instead of panicking.
    pub fn new() -> Client {
        ClientBuilder::new().build().expect("Client::new()")
    }

    /// Creates a `ClientBuilder` to configure a `Client`.
    ///
    /// This is the same as `ClientBuilder::new()`.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Convenience method to make a `GET` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn get<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::GET, url)
    }

    /// Convenience method to make a `POST` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn post<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::POST, url)
    }

    /// Convenience method to make a `PUT` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn put<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::PUT, url)
    }

    /// Convenience method to make a `PATCH` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn patch<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::PATCH, url)
    }

    /// Convenience method to make a `DELETE` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn delete<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::DELETE, url)
    }

    /// Convenience method to make a `HEAD` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn head<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::HEAD, url)
    }

    /// Start building a `Request` with the `Method` and `Url`.
    ///
    /// Returns a `RequestBuilder`, which will allow setting headers and
    /// request body before sending.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        let req = url.into_url().map(move |url| Request::new(method, url));
        RequestBuilder::new(self.clone(), req)
    }

    /// Executes a `Request`.
    ///
    /// A `Request` can be built manually with `Request::new()` or obtained
    /// from a RequestBuilder with `RequestBuilder::build()`.
    ///
    /// You should prefer to use the `RequestBuilder` and
    /// `RequestBuilder::send()`.
    ///
    /// # Errors
    ///
    /// This method fails if there was an error while sending request,
    /// redirect loop was detected or redirect limit was exhausted.
    pub fn execute(
        &self,
        request: Request,
    ) -> impl Future<Output = Result<Response, crate::Error>> {
        self.execute_request(request)
    }

    pub(super) fn execute_request(&self, req: Request) -> Pending {
        let (method, url, mut headers, body, timeout) = req.pieces();
        if url.scheme() != "http" && url.scheme() != "https" {
            return Pending::new_err(error::url_bad_scheme(url));
        }

        // check if we're in https_only mode and check the scheme of the current URL
        if self.inner.https_only && url.scheme() != "https" {
            return Pending::new_err(error::url_bad_scheme(url));
        }

        // insert default headers in the request headers
        // without overwriting already appended headers.
        for (key, value) in &self.inner.headers {
            if let Entry::Vacant(entry) = headers.entry(key) {
                entry.insert(value.clone());
            }
        }

        // Add cookies from the cookie store.
        #[cfg(feature = "cookies")]
        {
            if let Some(cookie_store) = self.inner.cookie_store.as_ref() {
                if headers.get(crate::header::COOKIE).is_none() {
                    add_cookie_header(&mut headers, &**cookie_store, &url);
                }
            }
        }

        let accept_encoding = self.inner.accepts.as_str();

        if let Some(accept_encoding) = accept_encoding {
            if !headers.contains_key(ACCEPT_ENCODING) && !headers.contains_key(RANGE) {
                headers.insert(ACCEPT_ENCODING, HeaderValue::from_static(accept_encoding));
            }
        }

        let uri = expect_uri(&url);

        let (reusable, body) = match body {
            Some(body) => {
                let (reusable, body) = body.try_reuse();
                (Some(reusable), body)
            }
            None => (None, Body::empty()),
        };

        self.proxy_auth(&uri, &mut headers);

        let mut req = hyper::Request::builder()
            .method(method.clone())
            .uri(uri)
            .body(body.into_stream())
            .expect("valid request parts");

        let timeout = timeout
            .or(self.inner.request_timeout)
            .map(tokio::time::sleep)
            .map(Box::pin);

        *req.headers_mut() = headers.clone();

        let in_flight = self.inner.hyper.request(req);

        Pending {
            inner: PendingInner::Request(PendingRequest {
                method,
                url,
                headers,
                body: reusable,

                urls: Vec::new(),

                client: self.inner.clone(),

                in_flight,
                timeout,
            }),
        }
    }

    fn proxy_auth(&self, dst: &Uri, headers: &mut HeaderMap) {
        if !self.inner.proxies_maybe_http_auth {
            return;
        }

        // Only set the header here if the destination scheme is 'http',
        // since otherwise, the header will be included in the CONNECT tunnel
        // request instead.
        if dst.scheme() != Some(&Scheme::HTTP) {
            return;
        }

        if headers.contains_key(PROXY_AUTHORIZATION) {
            return;
        }

        for proxy in self.inner.proxies.iter() {
            if proxy.is_match(dst) {
                if let Some(header) = proxy.http_basic_auth(dst) {
                    headers.insert(PROXY_AUTHORIZATION, header);
                }

                break;
            }
        }
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut builder = f.debug_struct("Client");
        self.inner.fmt_fields(&mut builder);
        builder.finish()
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut builder = f.debug_struct("ClientBuilder");
        self.config.fmt_fields(&mut builder);
        builder.finish()
    }
}

impl Config {
    fn fmt_fields(&self, f: &mut fmt::DebugStruct<'_, '_>) {
        // Instead of deriving Debug, only print fields when their output
        // would provide relevant or interesting data.

        #[cfg(feature = "cookies")]
        {
            if let Some(_) = self.cookie_store {
                f.field("cookie_store", &true);
            }
        }

        f.field("accepts", &self.accepts);

        if !self.proxies.is_empty() {
            f.field("proxies", &self.proxies);
        }

        if !self.redirect_policy.is_default() {
            f.field("redirect_policy", &self.redirect_policy);
        }

        if self.referer {
            f.field("referer", &true);
        }

        f.field("default_headers", &self.headers);

        if self.http1_title_case_headers {
            f.field("http1_title_case_headers", &true);
        }

        if self.http2_only {
            f.field("http2_prior_knowledge", &true);
        }

        if let Some(ref d) = self.connect_timeout {
            f.field("connect_timeout", d);
        }

        if let Some(ref d) = self.timeout {
            f.field("timeout", d);
        }

        if let Some(ref v) = self.local_address {
            f.field("local_address", v);
        }

        if self.nodelay {
            f.field("tcp_nodelay", &true);
        }

        #[cfg(feature = "native-tls")]
        {
            if !self.hostname_verification {
                f.field("danger_accept_invalid_hostnames", &true);
            }
        }

        #[cfg(feature = "__tls")]
        {
            if !self.certs_verification {
                f.field("danger_accept_invalid_certs", &true);
            }
        }

        #[cfg(all(feature = "native-tls-crate", feature = "__rustls"))]
        {
            f.field("tls_backend", &self.tls);
        }
    }
}

struct ClientRef {
    accepts: Accepts,
    #[cfg(feature = "cookies")]
    cookie_store: Option<Arc<dyn cookie::CookieStore>>,
    headers: HeaderMap,
    hyper: HyperClient,
    redirect_policy: redirect::Policy,
    referer: bool,
    request_timeout: Option<Duration>,
    proxies: Arc<Vec<Proxy>>,
    proxies_maybe_http_auth: bool,
    https_only: bool,
}

impl ClientRef {
    fn fmt_fields(&self, f: &mut fmt::DebugStruct<'_, '_>) {
        // Instead of deriving Debug, only print fields when their output
        // would provide relevant or interesting data.

        #[cfg(feature = "cookies")]
        {
            if let Some(_) = self.cookie_store {
                f.field("cookie_store", &true);
            }
        }

        f.field("accepts", &self.accepts);

        if !self.proxies.is_empty() {
            f.field("proxies", &self.proxies);
        }

        if !self.redirect_policy.is_default() {
            f.field("redirect_policy", &self.redirect_policy);
        }

        if self.referer {
            f.field("referer", &true);
        }

        f.field("default_headers", &self.headers);

        if let Some(ref d) = self.request_timeout {
            f.field("timeout", d);
        }
    }
}

pin_project! {
    pub(super) struct Pending {
        #[pin]
        inner: PendingInner,
    }
}

enum PendingInner {
    Request(PendingRequest),
    Error(Option<crate::Error>),
}

pin_project! {
    struct PendingRequest {
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Option<Option<Bytes>>,

        urls: Vec<Url>,

        client: Arc<ClientRef>,

        #[pin]
        in_flight: ResponseFuture,
        #[pin]
        timeout: Option<Pin<Box<Sleep>>>,
    }
}

impl PendingRequest {
    fn in_flight(self: Pin<&mut Self>) -> Pin<&mut ResponseFuture> {
        self.project().in_flight
    }

    fn timeout(self: Pin<&mut Self>) -> Pin<&mut Option<Pin<Box<Sleep>>>> {
        self.project().timeout
    }

    fn urls(self: Pin<&mut Self>) -> &mut Vec<Url> {
        self.project().urls
    }

    fn headers(self: Pin<&mut Self>) -> &mut HeaderMap {
        self.project().headers
    }
}

impl Pending {
    pub(super) fn new_err(err: crate::Error) -> Pending {
        Pending {
            inner: PendingInner::Error(Some(err)),
        }
    }

    fn inner(self: Pin<&mut Self>) -> Pin<&mut PendingInner> {
        self.project().inner
    }
}

impl Future for Pending {
    type Output = Result<Response, crate::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = self.inner();
        match inner.get_mut() {
            PendingInner::Request(ref mut req) => Pin::new(req).poll(cx),
            PendingInner::Error(ref mut err) => Poll::Ready(Err(err
                .take()
                .expect("Pending error polled more than once"))),
        }
    }
}

impl Future for PendingRequest {
    type Output = Result<Response, crate::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(delay) = self.as_mut().timeout().as_mut().as_pin_mut() {
            if let Poll::Ready(()) = delay.poll(cx) {
                return Poll::Ready(Err(
                    crate::error::request(crate::error::TimedOut).with_url(self.url.clone())
                ));
            }
        }

        loop {
            let res = match self.as_mut().in_flight().as_mut().poll(cx) {
                Poll::Ready(Err(e)) => {
                    return Poll::Ready(Err(crate::error::request(e).with_url(self.url.clone())));
                }
                Poll::Ready(Ok(res)) => res,
                Poll::Pending => return Poll::Pending,
            };

            #[cfg(feature = "cookies")]
            {
                if let Some(ref cookie_store) = self.client.cookie_store {
                    let mut cookies =
                        cookie::extract_response_cookie_headers(&res.headers()).peekable();
                    if cookies.peek().is_some() {
                        cookie_store.set_cookies(&mut cookies, &self.url);
                    }
                }
            }
            let should_redirect = match res.status() {
                StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER => {
                    self.body = None;
                    for header in &[
                        TRANSFER_ENCODING,
                        CONTENT_ENCODING,
                        CONTENT_TYPE,
                        CONTENT_LENGTH,
                    ] {
                        self.headers.remove(header);
                    }

                    match self.method {
                        Method::GET | Method::HEAD => {}
                        _ => {
                            self.method = Method::GET;
                        }
                    }
                    true
                }
                StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT => {
                    match self.body {
                        Some(Some(_)) | None => true,
                        Some(None) => false,
                    }
                }
                _ => false,
            };
            if should_redirect {
                let loc = res.headers().get(LOCATION).and_then(|val| {
                    let loc = (|| -> Option<Url> {
                        // Some sites may send a utf-8 Location header,
                        // even though we're supposed to treat those bytes
                        // as opaque, we'll check specifically for utf8.
                        self.url.join(str::from_utf8(val.as_bytes()).ok()?).ok()
                    })();

                    // Check that the `url` is also a valid `http::Uri`.
                    //
                    // If not, just log it and skip the redirect.
                    let loc = loc.and_then(|url| {
                        if try_uri(&url).is_some() {
                            Some(url)
                        } else {
                            None
                        }
                    });

                    if loc.is_none() {
                        debug!("Location header had invalid URI: {:?}", val);
                    }
                    loc
                });
                if let Some(loc) = loc {
                    if self.client.referer {
                        if let Some(referer) = make_referer(&loc, &self.url) {
                            self.headers.insert(REFERER, referer);
                        }
                    }
                    let url = self.url.clone();
                    self.as_mut().urls().push(url);
                    let action = self
                        .client
                        .redirect_policy
                        .check(res.status(), &loc, &self.urls);

                    match action {
                        redirect::ActionKind::Follow => {
                            debug!("redirecting '{}' to '{}'", self.url, loc);
                            self.url = loc;

                            let mut headers =
                                std::mem::replace(self.as_mut().headers(), HeaderMap::new());

                            remove_sensitive_headers(&mut headers, &self.url, &self.urls);
                            let uri = expect_uri(&self.url);
                            let body = match self.body {
                                Some(Some(ref body)) => Body::reusable(body.clone()),
                                _ => Body::empty(),
                            };
                            let mut req = hyper::Request::builder()
                                .method(self.method.clone())
                                .uri(uri.clone())
                                .body(body.into_stream())
                                .expect("valid request parts");

                            // Add cookies from the cookie store.
                            #[cfg(feature = "cookies")]
                            {
                                if let Some(ref cookie_store) = self.client.cookie_store {
                                    add_cookie_header(&mut headers, &**cookie_store, &self.url);
                                }
                            }

                            *req.headers_mut() = headers.clone();
                            std::mem::swap(self.as_mut().headers(), &mut headers);
                            *self.as_mut().in_flight().get_mut() = self.client.hyper.request(req);
                            continue;
                        }
                        redirect::ActionKind::Stop => {
                            debug!("redirect policy disallowed redirection to '{}'", loc);
                        }
                        redirect::ActionKind::Error(err) => {
                            return Poll::Ready(Err(crate::error::redirect(err, self.url.clone())));
                        }
                    }
                }
            }

            debug!("response '{}' for {}", res.status(), self.url);
            let res = Response::new(
                res,
                self.url.clone(),
                self.client.accepts,
                self.timeout.take(),
            );
            return Poll::Ready(Ok(res));
        }
    }
}

impl fmt::Debug for Pending {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.inner {
            PendingInner::Request(ref req) => f
                .debug_struct("Pending")
                .field("method", &req.method)
                .field("url", &req.url)
                .finish(),
            PendingInner::Error(ref err) => f.debug_struct("Pending").field("error", err).finish(),
        }
    }
}

fn make_referer(next: &Url, previous: &Url) -> Option<HeaderValue> {
    if next.scheme() == "http" && previous.scheme() == "https" {
        return None;
    }

    let mut referer = previous.clone();
    let _ = referer.set_username("");
    let _ = referer.set_password(None);
    referer.set_fragment(None);
    referer.as_str().parse().ok()
}

#[cfg(feature = "cookies")]
fn add_cookie_header(headers: &mut HeaderMap, cookie_store: &dyn cookie::CookieStore, url: &Url) {
    if let Some(header) = cookie_store.cookies(url) {
        headers.insert(crate::header::COOKIE, header);
    }
}

#[cfg(feature = "rustls-tls-native-roots")]
lazy_static! {
    static ref NATIVE_ROOTS: std::io::Result<RootCertStore> =
        rustls_native_certs::load_native_certs().map_err(|e| e.1);
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn execute_request_rejects_invald_urls() {
        let url_str = "hxxps://www.rust-lang.org/";
        let url = url::Url::parse(url_str).unwrap();
        let result = crate::get(url.clone()).await;

        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.is_builder());
        assert_eq!(url_str, err.url().unwrap().as_str());
    }
}
