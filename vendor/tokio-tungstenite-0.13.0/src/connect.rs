//! Connection helper.
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};

use tungstenite::{
    client::uri_mode, handshake::client::Response, protocol::WebSocketConfig, Error,
};

use super::{client_async_with_config, IntoClientRequest, Request, WebSocketStream};

#[cfg(feature = "tls")]
pub(crate) mod encryption {
    use native_tls::TlsConnector as NativeTlsConnector;
    use tokio_native_tls::{TlsConnector as TokioTlsConnector, TlsStream};

    use tokio::io::{AsyncRead, AsyncWrite};

    use tungstenite::{stream::Mode, Error};

    use crate::stream::Stream as StreamSwitcher;

    /// A stream that might be protected with TLS.
    pub type MaybeTlsStream<S> = StreamSwitcher<S, TlsStream<S>>;

    pub type AutoStream<S> = MaybeTlsStream<S>;

    /// A TLS connector that can be used when establishing TLS connections.
    pub type TlsConnector = NativeTlsConnector;

    pub async fn wrap_stream<S>(
        socket: S,
        domain: String,
        mode: Mode,
        tls_connector: Option<TlsConnector>,
    ) -> Result<AutoStream<S>, Error>
    where
        S: 'static + AsyncRead + AsyncWrite + Send + Unpin,
    {
        match mode {
            Mode::Plain => Ok(StreamSwitcher::Plain(socket)),
            Mode::Tls => {
                let try_connector = tls_connector.map_or_else(|| TlsConnector::new(), |c| Ok(c));
                let connector = try_connector.map_err(Error::Tls)?;
                let stream = TokioTlsConnector::from(connector);
                let connected = stream.connect(&domain, socket).await;
                match connected {
                    Err(e) => Err(Error::Tls(e)),
                    Ok(s) => Ok(StreamSwitcher::Tls(s)),
                }
            }
        }
    }
}

#[cfg(feature = "tls")]
pub use self::encryption::MaybeTlsStream;

pub use self::encryption::TlsConnector;

#[cfg(not(feature = "tls"))]
pub(crate) mod encryption {
    use tokio::io::{AsyncRead, AsyncWrite};

    use tungstenite::{stream::Mode, Error};

    pub type AutoStream<S> = S;

    /// A TLS connector that can be used when establishing TLS connections.
    pub type TlsConnector = ();

    pub async fn wrap_stream<S>(
        socket: S,
        _domain: String,
        mode: Mode,
        _tls_connector: Option<TlsConnector>,
    ) -> Result<AutoStream<S>, Error>
    where
        S: 'static + AsyncRead + AsyncWrite + Send + Unpin,
    {
        match mode {
            Mode::Plain => Ok(socket),
            Mode::Tls => Err(Error::Url("TLS support not compiled in.".into())),
        }
    }
}

use self::encryption::{wrap_stream, AutoStream};

/// Get a domain from an URL.
#[inline]
fn domain(request: &Request) -> Result<String, Error> {
    match request.uri().host() {
        Some(d) => Ok(d.to_string()),
        None => Err(Error::Url("no host name in the url".into())),
    }
}

/// Creates a WebSocket handshake from a request and a stream,
/// upgrading the stream to TLS if required.
pub async fn client_async_tls<R, S>(
    request: R,
    stream: S,
) -> Result<(WebSocketStream<AutoStream<S>>, Response), Error>
where
    R: IntoClientRequest + Unpin,
    S: 'static + AsyncRead + AsyncWrite + Send + Unpin,
    AutoStream<S>: Unpin,
{
    client_async_tls_with_config(request, stream, None, None).await
}

/// The same as `client_async_tls()` but the one can specify a websocket configuration,
/// and an optional TLS connector. If no connector is specified, the default one will
/// be created.
///
/// Please refer to `client_async_tls()` for more details.
pub async fn client_async_tls_with_config<R, S>(
    request: R,
    stream: S,
    config: Option<WebSocketConfig>,
    tls_connector: Option<TlsConnector>,
) -> Result<(WebSocketStream<AutoStream<S>>, Response), Error>
where
    R: IntoClientRequest + Unpin,
    S: 'static + AsyncRead + AsyncWrite + Send + Unpin,
    AutoStream<S>: Unpin,
{
    let request = request.into_client_request()?;

    let domain = domain(&request)?;

    // Make sure we check domain and mode first. URL must be valid.
    let mode = uri_mode(&request.uri())?;

    let stream = wrap_stream(stream, domain, mode, tls_connector).await?;
    client_async_with_config(request, stream, config).await
}

/// Connect to a given URL.
pub async fn connect_async<R>(
    request: R,
) -> Result<(WebSocketStream<AutoStream<TcpStream>>, Response), Error>
where
    R: IntoClientRequest + Unpin,
{
    connect_async_with_config(request, None).await
}

/// The same as `connect_async()` but the one can specify a websocket configuration.
/// Please refer to `connect_async()` for more details.
pub async fn connect_async_with_config<R>(
    request: R,
    config: Option<WebSocketConfig>,
) -> Result<(WebSocketStream<AutoStream<TcpStream>>, Response), Error>
where
    R: IntoClientRequest + Unpin,
{
    let request = request.into_client_request()?;

    let domain = domain(&request)?;
    let port = request
        .uri()
        .port_u16()
        .or_else(|| match request.uri().scheme_str() {
            Some("wss") => Some(443),
            Some("ws") => Some(80),
            _ => None,
        })
        .ok_or_else(|| Error::Url("Url scheme not supported".into()))?;

    let addr = format!("{}:{}", domain, port);
    let try_socket = TcpStream::connect(addr).await;
    let socket = try_socket.map_err(Error::Io)?;
    client_async_tls_with_config(request, socket, config, None).await
}
