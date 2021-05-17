use crate::codec::{RecvError, UserError};
use crate::frame::{Reason, StreamId};
use crate::{client, frame, proto, server};

use crate::frame::DEFAULT_INITIAL_WINDOW_SIZE;
use crate::proto::*;

use bytes::{Buf, Bytes};
use futures_core::Stream;
use std::io;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

/// An H2 connection
#[derive(Debug)]
pub(crate) struct Connection<T, P, B: Buf = Bytes>
where
    P: Peer,
{
    /// Read / write frame values
    codec: Codec<T, Prioritized<B>>,

    inner: ConnectionInner<P, B>,
}

// Extracted part of `Connection` which does not depend on `T`. Reduces the amount of duplicated
// method instantiations.
#[derive(Debug)]
struct ConnectionInner<P, B: Buf = Bytes>
where
    P: Peer,
{
    /// Tracks the connection level state transitions.
    state: State,

    /// An error to report back once complete.
    ///
    /// This exists separately from State in order to support
    /// graceful shutdown.
    error: Option<Reason>,

    /// Pending GOAWAY frames to write.
    go_away: GoAway,

    /// Ping/pong handler
    ping_pong: PingPong,

    /// Connection settings
    settings: Settings,

    /// Stream state handler
    streams: Streams<B, P>,

    /// A `tracing` span tracking the lifetime of the connection.
    span: tracing::Span,

    /// Client or server
    _phantom: PhantomData<P>,
}

struct DynConnection<'a, B: Buf = Bytes> {
    state: &'a mut State,

    go_away: &'a mut GoAway,

    streams: DynStreams<'a, B>,

    error: &'a mut Option<Reason>,

    ping_pong: &'a mut PingPong,
}

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub next_stream_id: StreamId,
    pub initial_max_send_streams: usize,
    pub reset_stream_duration: Duration,
    pub reset_stream_max: usize,
    pub settings: frame::Settings,
}

#[derive(Debug)]
enum State {
    /// Currently open in a sane state
    Open,

    /// The codec must be flushed
    Closing(Reason),

    /// In a closed state
    Closed(Reason),
}

impl<T, P, B> Connection<T, P, B>
where
    T: AsyncRead + AsyncWrite + Unpin,
    P: Peer,
    B: Buf,
{
    pub fn new(codec: Codec<T, Prioritized<B>>, config: Config) -> Connection<T, P, B> {
        fn streams_config(config: &Config) -> streams::Config {
            streams::Config {
                local_init_window_sz: config
                    .settings
                    .initial_window_size()
                    .unwrap_or(DEFAULT_INITIAL_WINDOW_SIZE),
                initial_max_send_streams: config.initial_max_send_streams,
                local_next_stream_id: config.next_stream_id,
                local_push_enabled: config.settings.is_push_enabled().unwrap_or(true),
                local_reset_duration: config.reset_stream_duration,
                local_reset_max: config.reset_stream_max,
                remote_init_window_sz: DEFAULT_INITIAL_WINDOW_SIZE,
                remote_max_initiated: config
                    .settings
                    .max_concurrent_streams()
                    .map(|max| max as usize),
            }
        }
        let streams = Streams::new(streams_config(&config));
        Connection {
            codec,
            inner: ConnectionInner {
                state: State::Open,
                error: None,
                go_away: GoAway::new(),
                ping_pong: PingPong::new(),
                settings: Settings::new(config.settings),
                streams,
                span: tracing::debug_span!("Connection", peer = %P::NAME),
                _phantom: PhantomData,
            },
        }
    }

    /// connection flow control
    pub(crate) fn set_target_window_size(&mut self, size: WindowSize) {
        self.inner.streams.set_target_connection_window_size(size);
    }

    /// Send a new SETTINGS frame with an updated initial window size.
    pub(crate) fn set_initial_window_size(&mut self, size: WindowSize) -> Result<(), UserError> {
        let mut settings = frame::Settings::default();
        settings.set_initial_window_size(Some(size));
        self.inner.settings.send_settings(settings)
    }

    /// Returns the maximum number of concurrent streams that may be initiated
    /// by this peer.
    pub(crate) fn max_send_streams(&self) -> usize {
        self.inner.streams.max_send_streams()
    }

    /// Returns the maximum number of concurrent streams that may be initiated
    /// by the remote peer.
    pub(crate) fn max_recv_streams(&self) -> usize {
        self.inner.streams.max_recv_streams()
    }

    /// Returns `Ready` when the connection is ready to receive a frame.
    ///
    /// Returns `RecvError` as this may raise errors that are caused by delayed
    /// processing of received frames.
    fn poll_ready(&mut self, cx: &mut Context) -> Poll<Result<(), RecvError>> {
        let _e = self.inner.span.enter();
        let span = tracing::trace_span!("poll_ready");
        let _e = span.enter();
        // The order of these calls don't really matter too much
        ready!(self.inner.ping_pong.send_pending_pong(cx, &mut self.codec))?;
        ready!(self.inner.ping_pong.send_pending_ping(cx, &mut self.codec))?;
        ready!(self
            .inner
            .settings
            .poll_send(cx, &mut self.codec, &mut self.inner.streams))?;
        ready!(self.inner.streams.send_pending_refusal(cx, &mut self.codec))?;

        Poll::Ready(Ok(()))
    }

    /// Send any pending GOAWAY frames.
    ///
    /// This will return `Some(reason)` if the connection should be closed
    /// afterwards. If this is a graceful shutdown, this returns `None`.
    fn poll_go_away(&mut self, cx: &mut Context) -> Poll<Option<io::Result<Reason>>> {
        self.inner.go_away.send_pending_go_away(cx, &mut self.codec)
    }

    pub fn go_away_from_user(&mut self, e: Reason) {
        self.inner.as_dyn().go_away_from_user(e)
    }

    fn take_error(&mut self, ours: Reason) -> Poll<Result<(), proto::Error>> {
        let reason = if let Some(theirs) = self.inner.error.take() {
            match (ours, theirs) {
                // If either side reported an error, return that
                // to the user.
                (Reason::NO_ERROR, err) | (err, Reason::NO_ERROR) => err,
                // If both sides reported an error, give their
                // error back to th user. We assume our error
                // was a consequence of their error, and less
                // important.
                (_, theirs) => theirs,
            }
        } else {
            ours
        };

        if reason == Reason::NO_ERROR {
            Poll::Ready(Ok(()))
        } else {
            Poll::Ready(Err(proto::Error::Proto(reason)))
        }
    }

    /// Closes the connection by transitioning to a GOAWAY state
    /// iff there are no streams or references
    pub fn maybe_close_connection_if_no_streams(&mut self) {
        // If we poll() and realize that there are no streams or references
        // then we can close the connection by transitioning to GOAWAY
        if !self.inner.streams.has_streams_or_other_references() {
            self.inner.as_dyn().go_away_now(Reason::NO_ERROR);
        }
    }

    pub(crate) fn take_user_pings(&mut self) -> Option<UserPings> {
        self.inner.ping_pong.take_user_pings()
    }

    /// Advances the internal state of the connection.
    pub fn poll(&mut self, cx: &mut Context) -> Poll<Result<(), proto::Error>> {
        // XXX(eliza): cloning the span is unfortunately necessary here in
        // order to placate the borrow checker — `self` is mutably borrowed by
        // `poll2`, which means that we can't borrow `self.span` to enter it.
        // The clone is just an atomic ref bump.
        let span = self.inner.span.clone();
        let _e = span.enter();
        let span = tracing::trace_span!("poll");
        let _e = span.enter();

        loop {
            tracing::trace!(connection.state = ?self.inner.state);
            // TODO: probably clean up this glob of code
            match self.inner.state {
                // When open, continue to poll a frame
                State::Open => {
                    let result = match self.poll2(cx) {
                        Poll::Ready(result) => result,
                        // The connection is not ready to make progress
                        Poll::Pending => {
                            // Ensure all window updates have been sent.
                            //
                            // This will also handle flushing `self.codec`
                            ready!(self.inner.streams.poll_complete(cx, &mut self.codec))?;

                            if (self.inner.error.is_some()
                                || self.inner.go_away.should_close_on_idle())
                                && !self.inner.streams.has_streams()
                            {
                                self.inner.as_dyn().go_away_now(Reason::NO_ERROR);
                                continue;
                            }

                            return Poll::Pending;
                        }
                    };

                    self.inner.as_dyn().handle_poll2_result(result)?
                }
                State::Closing(reason) => {
                    tracing::trace!("connection closing after flush");
                    // Flush/shutdown the codec
                    ready!(self.codec.shutdown(cx))?;

                    // Transition the state to error
                    self.inner.state = State::Closed(reason);
                }
                State::Closed(reason) => return self.take_error(reason),
            }
        }
    }

    fn poll2(&mut self, cx: &mut Context) -> Poll<Result<(), RecvError>> {
        // This happens outside of the loop to prevent needing to do a clock
        // check and then comparison of the queue possibly multiple times a
        // second (and thus, the clock wouldn't have changed enough to matter).
        self.clear_expired_reset_streams();

        loop {
            // First, ensure that the `Connection` is able to receive a frame
            //
            // The order here matters:
            // - poll_go_away may buffer a graceful shutdown GOAWAY frame
            // - If it has, we've also added a PING to be sent in poll_ready
            if let Some(reason) = ready!(self.poll_go_away(cx)?) {
                if self.inner.go_away.should_close_now() {
                    if self.inner.go_away.is_user_initiated() {
                        // A user initiated abrupt shutdown shouldn't return
                        // the same error back to the user.
                        return Poll::Ready(Ok(()));
                    } else {
                        return Poll::Ready(Err(RecvError::Connection(reason)));
                    }
                }
                // Only NO_ERROR should be waiting for idle
                debug_assert_eq!(
                    reason,
                    Reason::NO_ERROR,
                    "graceful GOAWAY should be NO_ERROR"
                );
            }
            ready!(self.poll_ready(cx))?;

            match self
                .inner
                .as_dyn()
                .recv_frame(ready!(Pin::new(&mut self.codec).poll_next(cx)?))?
            {
                ReceivedFrame::Settings(frame) => {
                    self.inner.settings.recv_settings(
                        frame,
                        &mut self.codec,
                        &mut self.inner.streams,
                    )?;
                }
                ReceivedFrame::Continue => (),
                ReceivedFrame::Done => {
                    return Poll::Ready(Ok(()));
                }
            }
        }
    }

    fn clear_expired_reset_streams(&mut self) {
        self.inner.streams.clear_expired_reset_streams();
    }
}

impl<P, B> ConnectionInner<P, B>
where
    P: Peer,
    B: Buf,
{
    fn as_dyn(&mut self) -> DynConnection<'_, B> {
        let ConnectionInner {
            state,
            go_away,
            streams,
            error,
            ping_pong,
            ..
        } = self;
        let streams = streams.as_dyn();
        DynConnection {
            state,
            go_away,
            streams,
            error,
            ping_pong,
        }
    }
}

impl<B> DynConnection<'_, B>
where
    B: Buf,
{
    fn go_away(&mut self, id: StreamId, e: Reason) {
        let frame = frame::GoAway::new(id, e);
        self.streams.send_go_away(id);
        self.go_away.go_away(frame);
    }

    fn go_away_now(&mut self, e: Reason) {
        let last_processed_id = self.streams.last_processed_id();
        let frame = frame::GoAway::new(last_processed_id, e);
        self.go_away.go_away_now(frame);
    }

    fn go_away_from_user(&mut self, e: Reason) {
        let last_processed_id = self.streams.last_processed_id();
        let frame = frame::GoAway::new(last_processed_id, e);
        self.go_away.go_away_from_user(frame);

        // Notify all streams of reason we're abruptly closing.
        self.streams.recv_err(&proto::Error::Proto(e));
    }

    fn handle_poll2_result(&mut self, result: Result<(), RecvError>) -> Result<(), Error> {
        use crate::codec::RecvError::*;
        match result {
            // The connection has shutdown normally
            Ok(()) => {
                *self.state = State::Closing(Reason::NO_ERROR);
                Ok(())
            }
            // Attempting to read a frame resulted in a connection level
            // error. This is handled by setting a GOAWAY frame followed by
            // terminating the connection.
            Err(Connection(e)) => {
                tracing::debug!(error = ?e, "Connection::poll; connection error");

                // We may have already sent a GOAWAY for this error,
                // if so, don't send another, just flush and close up.
                if let Some(reason) = self.go_away.going_away_reason() {
                    if reason == e {
                        tracing::trace!("    -> already going away");
                        *self.state = State::Closing(e);
                        return Ok(());
                    }
                }

                // Reset all active streams
                self.streams.recv_err(&e.into());
                self.go_away_now(e);
                Ok(())
            }
            // Attempting to read a frame resulted in a stream level error.
            // This is handled by resetting the frame then trying to read
            // another frame.
            Err(Stream { id, reason }) => {
                tracing::trace!(?id, ?reason, "stream error");
                self.streams.send_reset(id, reason);
                Ok(())
            }
            // Attempting to read a frame resulted in an I/O error. All
            // active streams must be reset.
            //
            // TODO: Are I/O errors recoverable?
            Err(Io(e)) => {
                tracing::debug!(error = ?e, "Connection::poll; IO error");
                let e = e.into();

                // Reset all active streams
                self.streams.recv_err(&e);

                // Return the error
                Err(e)
            }
        }
    }

    fn recv_frame(&mut self, frame: Option<Frame>) -> Result<ReceivedFrame, RecvError> {
        use crate::frame::Frame::*;
        match frame {
            Some(Headers(frame)) => {
                tracing::trace!(?frame, "recv HEADERS");
                self.streams.recv_headers(frame)?;
            }
            Some(Data(frame)) => {
                tracing::trace!(?frame, "recv DATA");
                self.streams.recv_data(frame)?;
            }
            Some(Reset(frame)) => {
                tracing::trace!(?frame, "recv RST_STREAM");
                self.streams.recv_reset(frame)?;
            }
            Some(PushPromise(frame)) => {
                tracing::trace!(?frame, "recv PUSH_PROMISE");
                self.streams.recv_push_promise(frame)?;
            }
            Some(Settings(frame)) => {
                tracing::trace!(?frame, "recv SETTINGS");
                return Ok(ReceivedFrame::Settings(frame));
            }
            Some(GoAway(frame)) => {
                tracing::trace!(?frame, "recv GOAWAY");
                // This should prevent starting new streams,
                // but should allow continuing to process current streams
                // until they are all EOS. Once they are, State should
                // transition to GoAway.
                self.streams.recv_go_away(&frame)?;
                *self.error = Some(frame.reason());
            }
            Some(Ping(frame)) => {
                tracing::trace!(?frame, "recv PING");
                let status = self.ping_pong.recv_ping(frame);
                if status.is_shutdown() {
                    assert!(
                        self.go_away.is_going_away(),
                        "received unexpected shutdown ping"
                    );

                    let last_processed_id = self.streams.last_processed_id();
                    self.go_away(last_processed_id, Reason::NO_ERROR);
                }
            }
            Some(WindowUpdate(frame)) => {
                tracing::trace!(?frame, "recv WINDOW_UPDATE");
                self.streams.recv_window_update(frame)?;
            }
            Some(Priority(frame)) => {
                tracing::trace!(?frame, "recv PRIORITY");
                // TODO: handle
            }
            None => {
                tracing::trace!("codec closed");
                self.streams.recv_eof(false).expect("mutex poisoned");
                return Ok(ReceivedFrame::Done);
            }
        }
        Ok(ReceivedFrame::Continue)
    }
}

enum ReceivedFrame {
    Settings(frame::Settings),
    Continue,
    Done,
}

impl<T, B> Connection<T, client::Peer, B>
where
    T: AsyncRead + AsyncWrite,
    B: Buf,
{
    pub(crate) fn streams(&self) -> &Streams<B, client::Peer> {
        &self.inner.streams
    }
}

impl<T, B> Connection<T, server::Peer, B>
where
    T: AsyncRead + AsyncWrite + Unpin,
    B: Buf,
{
    pub fn next_incoming(&mut self) -> Option<StreamRef<B>> {
        self.inner.streams.next_incoming()
    }

    // Graceful shutdown only makes sense for server peers.
    pub fn go_away_gracefully(&mut self) {
        if self.inner.go_away.is_going_away() {
            // No reason to start a new one.
            return;
        }

        // According to http://httpwg.org/specs/rfc7540.html#GOAWAY:
        //
        // > A server that is attempting to gracefully shut down a connection
        // > SHOULD send an initial GOAWAY frame with the last stream
        // > identifier set to 2^31-1 and a NO_ERROR code. This signals to the
        // > client that a shutdown is imminent and that initiating further
        // > requests is prohibited. After allowing time for any in-flight
        // > stream creation (at least one round-trip time), the server can
        // > send another GOAWAY frame with an updated last stream identifier.
        // > This ensures that a connection can be cleanly shut down without
        // > losing requests.
        self.inner.as_dyn().go_away(StreamId::MAX, Reason::NO_ERROR);

        // We take the advice of waiting 1 RTT literally, and wait
        // for a pong before proceeding.
        self.inner.ping_pong.ping_shutdown();
    }
}

impl<T, P, B> Drop for Connection<T, P, B>
where
    P: Peer,
    B: Buf,
{
    fn drop(&mut self) {
        // Ignore errors as this indicates that the mutex is poisoned.
        let _ = self.inner.streams.recv_eof(true);
    }
}
