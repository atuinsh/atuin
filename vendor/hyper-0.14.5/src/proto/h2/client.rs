use std::error::Error as StdError;
#[cfg(feature = "runtime")]
use std::time::Duration;

use futures_channel::{mpsc, oneshot};
use futures_util::future::{self, Either, FutureExt as _, TryFutureExt as _};
use futures_util::stream::StreamExt as _;
use h2::client::{Builder, SendRequest};
use tokio::io::{AsyncRead, AsyncWrite};

use super::{decode_content_length, ping, PipeToSendStream, SendBuf};
use crate::body::HttpBody;
use crate::common::{task, exec::Exec, Future, Never, Pin, Poll};
use crate::headers;
use crate::proto::Dispatched;
use crate::{Body, Request, Response};

type ClientRx<B> = crate::client::dispatch::Receiver<Request<B>, Response<Body>>;

///// An mpsc channel is used to help notify the `Connection` task when *all*
///// other handles to it have been dropped, so that it can shutdown.
type ConnDropRef = mpsc::Sender<Never>;

///// A oneshot channel watches the `Connection` task, and when it completes,
///// the "dispatch" task will be notified and can shutdown sooner.
type ConnEof = oneshot::Receiver<Never>;

// Our defaults are chosen for the "majority" case, which usually are not
// resource constrained, and so the spec default of 64kb can be too limiting
// for performance.
const DEFAULT_CONN_WINDOW: u32 = 1024 * 1024 * 5; // 5mb
const DEFAULT_STREAM_WINDOW: u32 = 1024 * 1024 * 2; // 2mb
const DEFAULT_MAX_FRAME_SIZE: u32 = 1024 * 16; // 16kb

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub(crate) adaptive_window: bool,
    pub(crate) initial_conn_window_size: u32,
    pub(crate) initial_stream_window_size: u32,
    pub(crate) max_frame_size: u32,
    #[cfg(feature = "runtime")]
    pub(crate) keep_alive_interval: Option<Duration>,
    #[cfg(feature = "runtime")]
    pub(crate) keep_alive_timeout: Duration,
    #[cfg(feature = "runtime")]
    pub(crate) keep_alive_while_idle: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            adaptive_window: false,
            initial_conn_window_size: DEFAULT_CONN_WINDOW,
            initial_stream_window_size: DEFAULT_STREAM_WINDOW,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            #[cfg(feature = "runtime")]
            keep_alive_interval: None,
            #[cfg(feature = "runtime")]
            keep_alive_timeout: Duration::from_secs(20),
            #[cfg(feature = "runtime")]
            keep_alive_while_idle: false,
        }
    }
}

fn new_builder(config: &Config) -> Builder {
    let mut builder = Builder::default();
    builder
        .initial_window_size(config.initial_stream_window_size)
        .initial_connection_window_size(config.initial_conn_window_size)
        .max_frame_size(config.max_frame_size)
        .enable_push(false);
    builder
}

fn new_ping_config(config: &Config) -> ping::Config {
    ping::Config {
        bdp_initial_window: if config.adaptive_window {
            Some(config.initial_stream_window_size)
        } else {
            None
        },
        #[cfg(feature = "runtime")]
        keep_alive_interval: config.keep_alive_interval,
        #[cfg(feature = "runtime")]
        keep_alive_timeout: config.keep_alive_timeout,
        #[cfg(feature = "runtime")]
        keep_alive_while_idle: config.keep_alive_while_idle,
    }
}

pub(crate) async fn handshake<T, B>(
    io: T,
    req_rx: ClientRx<B>,
    config: &Config,
    exec: Exec,
) -> crate::Result<ClientTask<B>>
where
    T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    B: HttpBody,
    B::Data: Send + 'static,
{
    let (h2_tx, mut conn) = new_builder(config)
        .handshake::<_, SendBuf<B::Data>>(io)
        .await
        .map_err(crate::Error::new_h2)?;

    // An mpsc channel is used entirely to detect when the
    // 'Client' has been dropped. This is to get around a bug
    // in h2 where dropping all SendRequests won't notify a
    // parked Connection.
    let (conn_drop_ref, rx) = mpsc::channel(1);
    let (cancel_tx, conn_eof) = oneshot::channel();

    let conn_drop_rx = rx.into_future().map(|(item, _rx)| {
        if let Some(never) = item {
            match never {}
        }
    });

    let ping_config = new_ping_config(&config);

    let (conn, ping) = if ping_config.is_enabled() {
        let pp = conn.ping_pong().expect("conn.ping_pong");
        let (recorder, mut ponger) = ping::channel(pp, ping_config);

        let conn = future::poll_fn(move |cx| {
            match ponger.poll(cx) {
                Poll::Ready(ping::Ponged::SizeUpdate(wnd)) => {
                    conn.set_target_window_size(wnd);
                    conn.set_initial_window_size(wnd)?;
                }
                #[cfg(feature = "runtime")]
                Poll::Ready(ping::Ponged::KeepAliveTimedOut) => {
                    debug!("connection keep-alive timed out");
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => {}
            }

            Pin::new(&mut conn).poll(cx)
        });
        (Either::Left(conn), recorder)
    } else {
        (Either::Right(conn), ping::disabled())
    };
    let conn = conn.map_err(|e| debug!("connection error: {}", e));

    exec.execute(conn_task(conn, conn_drop_rx, cancel_tx));

    Ok(ClientTask {
        ping,
        conn_drop_ref,
        conn_eof,
        executor: exec,
        h2_tx,
        req_rx,
    })
}

async fn conn_task<C, D>(conn: C, drop_rx: D, cancel_tx: oneshot::Sender<Never>)
where
    C: Future + Unpin,
    D: Future<Output = ()> + Unpin,
{
    match future::select(conn, drop_rx).await {
        Either::Left(_) => {
            // ok or err, the `conn` has finished
        }
        Either::Right(((), conn)) => {
            // mpsc has been dropped, hopefully polling
            // the connection some more should start shutdown
            // and then close
            trace!("send_request dropped, starting conn shutdown");
            drop(cancel_tx);
            let _ = conn.await;
        }
    }
}

pub(crate) struct ClientTask<B>
where
    B: HttpBody,
{
    ping: ping::Recorder,
    conn_drop_ref: ConnDropRef,
    conn_eof: ConnEof,
    executor: Exec,
    h2_tx: SendRequest<SendBuf<B::Data>>,
    req_rx: ClientRx<B>,
}

impl<B> Future for ClientTask<B>
where
    B: HttpBody + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    type Output = crate::Result<Dispatched>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        loop {
            match ready!(self.h2_tx.poll_ready(cx)) {
                Ok(()) => (),
                Err(err) => {
                    self.ping.ensure_not_timed_out()?;
                    return if err.reason() == Some(::h2::Reason::NO_ERROR) {
                        trace!("connection gracefully shutdown");
                        Poll::Ready(Ok(Dispatched::Shutdown))
                    } else {
                        Poll::Ready(Err(crate::Error::new_h2(err)))
                    };
                }
            };

            match self.req_rx.poll_recv(cx) {
                Poll::Ready(Some((req, cb))) => {
                    // check that future hasn't been canceled already
                    if cb.is_canceled() {
                        trace!("request callback is canceled");
                        continue;
                    }
                    let (head, body) = req.into_parts();
                    let mut req = ::http::Request::from_parts(head, ());
                    super::strip_connection_headers(req.headers_mut(), true);
                    if let Some(len) = body.size_hint().exact() {
                        if len != 0 || headers::method_has_defined_payload_semantics(req.method()) {
                            headers::set_content_length_if_missing(req.headers_mut(), len);
                        }
                    }
                    let eos = body.is_end_stream();
                    let (fut, body_tx) = match self.h2_tx.send_request(req, eos) {
                        Ok(ok) => ok,
                        Err(err) => {
                            debug!("client send request error: {}", err);
                            cb.send(Err((crate::Error::new_h2(err), None)));
                            continue;
                        }
                    };

                    let ping = self.ping.clone();
                    if !eos {
                        let mut pipe = Box::pin(PipeToSendStream::new(body, body_tx)).map(|res| {
                            if let Err(e) = res {
                                debug!("client request body error: {}", e);
                            }
                        });

                        // eagerly see if the body pipe is ready and
                        // can thus skip allocating in the executor
                        match Pin::new(&mut pipe).poll(cx) {
                            Poll::Ready(_) => (),
                            Poll::Pending => {
                                let conn_drop_ref = self.conn_drop_ref.clone();
                                // keep the ping recorder's knowledge of an
                                // "open stream" alive while this body is
                                // still sending...
                                let ping = ping.clone();
                                let pipe = pipe.map(move |x| {
                                    drop(conn_drop_ref);
                                    drop(ping);
                                    x
                                });
                                self.executor.execute(pipe);
                            }
                        }
                    }

                    let fut = fut.map(move |result| match result {
                        Ok(res) => {
                            // record that we got the response headers
                            ping.record_non_data();

                            let content_length = decode_content_length(res.headers());
                            let res = res.map(|stream| {
                                let ping = ping.for_stream(&stream);
                                crate::Body::h2(stream, content_length, ping)
                            });
                            Ok(res)
                        }
                        Err(err) => {
                            ping.ensure_not_timed_out().map_err(|e| (e, None))?;

                            debug!("client response error: {}", err);
                            Err((crate::Error::new_h2(err), None))
                        }
                    });
                    self.executor.execute(cb.send_when(fut));
                    continue;
                }

                Poll::Ready(None) => {
                    trace!("client::dispatch::Sender dropped");
                    return Poll::Ready(Ok(Dispatched::Shutdown));
                }

                Poll::Pending => match ready!(Pin::new(&mut self.conn_eof).poll(cx)) {
                    Ok(never) => match never {},
                    Err(_conn_is_eof) => {
                        trace!("connection task is closed, closing dispatch task");
                        return Poll::Ready(Ok(Dispatched::Shutdown));
                    }
                },
            }
        }
    }
}
