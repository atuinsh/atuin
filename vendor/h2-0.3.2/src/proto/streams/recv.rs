use super::*;
use crate::codec::{RecvError, UserError};
use crate::frame::{PushPromiseHeaderError, Reason, DEFAULT_INITIAL_WINDOW_SIZE};
use crate::{frame, proto};
use std::task::Context;

use http::{HeaderMap, Request, Response};

use std::io;
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(super) struct Recv {
    /// Initial window size of remote initiated streams
    init_window_sz: WindowSize,

    /// Connection level flow control governing received data
    flow: FlowControl,

    /// Amount of connection window capacity currently used by outstanding streams.
    in_flight_data: WindowSize,

    /// The lowest stream ID that is still idle
    next_stream_id: Result<StreamId, StreamIdOverflow>,

    /// The stream ID of the last processed stream
    last_processed_id: StreamId,

    /// Any streams with a higher ID are ignored.
    ///
    /// This starts as MAX, but is lowered when a GOAWAY is received.
    ///
    /// > After sending a GOAWAY frame, the sender can discard frames for
    /// > streams initiated by the receiver with identifiers higher than
    /// > the identified last stream.
    max_stream_id: StreamId,

    /// Streams that have pending window updates
    pending_window_updates: store::Queue<stream::NextWindowUpdate>,

    /// New streams to be accepted
    pending_accept: store::Queue<stream::NextAccept>,

    /// Locally reset streams that should be reaped when they expire
    pending_reset_expired: store::Queue<stream::NextResetExpire>,

    /// How long locally reset streams should ignore received frames
    reset_duration: Duration,

    /// Holds frames that are waiting to be read
    buffer: Buffer<Event>,

    /// Refused StreamId, this represents a frame that must be sent out.
    refused: Option<StreamId>,

    /// If push promises are allowed to be received.
    is_push_enabled: bool,
}

#[derive(Debug)]
pub(super) enum Event {
    Headers(peer::PollMessage),
    Data(Bytes),
    Trailers(HeaderMap),
}

#[derive(Debug)]
pub(super) enum RecvHeaderBlockError<T> {
    Oversize(T),
    State(RecvError),
}

#[derive(Debug)]
pub(crate) enum Open {
    PushPromise,
    Headers,
}

#[derive(Debug, Clone, Copy)]
struct Indices {
    head: store::Key,
    tail: store::Key,
}

impl Recv {
    pub fn new(peer: peer::Dyn, config: &Config) -> Self {
        let next_stream_id = if peer.is_server() { 1 } else { 2 };

        let mut flow = FlowControl::new();

        // connections always have the default window size, regardless of
        // settings
        flow.inc_window(DEFAULT_INITIAL_WINDOW_SIZE)
            .expect("invalid initial remote window size");
        flow.assign_capacity(DEFAULT_INITIAL_WINDOW_SIZE);

        Recv {
            init_window_sz: config.local_init_window_sz,
            flow,
            in_flight_data: 0 as WindowSize,
            next_stream_id: Ok(next_stream_id.into()),
            pending_window_updates: store::Queue::new(),
            last_processed_id: StreamId::ZERO,
            max_stream_id: StreamId::MAX,
            pending_accept: store::Queue::new(),
            pending_reset_expired: store::Queue::new(),
            reset_duration: config.local_reset_duration,
            buffer: Buffer::new(),
            refused: None,
            is_push_enabled: config.local_push_enabled,
        }
    }

    /// Returns the initial receive window size
    pub fn init_window_sz(&self) -> WindowSize {
        self.init_window_sz
    }

    /// Returns the ID of the last processed stream
    pub fn last_processed_id(&self) -> StreamId {
        self.last_processed_id
    }

    /// Update state reflecting a new, remotely opened stream
    ///
    /// Returns the stream state if successful. `None` if refused
    pub fn open(
        &mut self,
        id: StreamId,
        mode: Open,
        counts: &mut Counts,
    ) -> Result<Option<StreamId>, RecvError> {
        assert!(self.refused.is_none());

        counts.peer().ensure_can_open(id, mode)?;

        let next_id = self.next_stream_id()?;
        if id < next_id {
            proto_err!(conn: "id ({:?}) < next_id ({:?})", id, next_id);
            return Err(RecvError::Connection(Reason::PROTOCOL_ERROR));
        }

        self.next_stream_id = id.next_id();

        if !counts.can_inc_num_recv_streams() {
            self.refused = Some(id);
            return Ok(None);
        }

        Ok(Some(id))
    }

    /// Transition the stream state based on receiving headers
    ///
    /// The caller ensures that the frame represents headers and not trailers.
    pub fn recv_headers(
        &mut self,
        frame: frame::Headers,
        stream: &mut store::Ptr,
        counts: &mut Counts,
    ) -> Result<(), RecvHeaderBlockError<Option<frame::Headers>>> {
        tracing::trace!("opening stream; init_window={}", self.init_window_sz);
        let is_initial = stream.state.recv_open(&frame)?;

        if is_initial {
            // TODO: be smarter about this logic
            if frame.stream_id() > self.last_processed_id {
                self.last_processed_id = frame.stream_id();
            }

            // Increment the number of concurrent streams
            counts.inc_num_recv_streams(stream);
        }

        if !stream.content_length.is_head() {
            use super::stream::ContentLength;
            use http::header;

            if let Some(content_length) = frame.fields().get(header::CONTENT_LENGTH) {
                let content_length = match frame::parse_u64(content_length.as_bytes()) {
                    Ok(v) => v,
                    Err(()) => {
                        proto_err!(stream: "could not parse content-length; stream={:?}", stream.id);
                        return Err(RecvError::Stream {
                            id: stream.id,
                            reason: Reason::PROTOCOL_ERROR,
                        }
                        .into());
                    }
                };

                stream.content_length = ContentLength::Remaining(content_length);
            }
        }

        if frame.is_over_size() {
            // A frame is over size if the decoded header block was bigger than
            // SETTINGS_MAX_HEADER_LIST_SIZE.
            //
            // > A server that receives a larger header block than it is willing
            // > to handle can send an HTTP 431 (Request Header Fields Too
            // > Large) status code [RFC6585]. A client can discard responses
            // > that it cannot process.
            //
            // So, if peer is a server, we'll send a 431. In either case,
            // an error is recorded, which will send a REFUSED_STREAM,
            // since we don't want any of the data frames either.
            tracing::debug!(
                "stream error REQUEST_HEADER_FIELDS_TOO_LARGE -- \
                 recv_headers: frame is over size; stream={:?}",
                stream.id
            );
            return if counts.peer().is_server() && is_initial {
                let mut res = frame::Headers::new(
                    stream.id,
                    frame::Pseudo::response(::http::StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE),
                    HeaderMap::new(),
                );
                res.set_end_stream();
                Err(RecvHeaderBlockError::Oversize(Some(res)))
            } else {
                Err(RecvHeaderBlockError::Oversize(None))
            };
        }

        let stream_id = frame.stream_id();
        let (pseudo, fields) = frame.into_parts();
        if !pseudo.is_informational() {
            let message = counts
                .peer()
                .convert_poll_message(pseudo, fields, stream_id)?;

            // Push the frame onto the stream's recv buffer
            stream
                .pending_recv
                .push_back(&mut self.buffer, Event::Headers(message));
            stream.notify_recv();
        }

        // Only servers can receive a headers frame that initiates the stream.
        // This is verified in `Streams` before calling this function.
        if counts.peer().is_server() {
            self.pending_accept.push(stream);
        }

        Ok(())
    }

    /// Called by the server to get the request
    ///
    /// TODO: Should this fn return `Result`?
    pub fn take_request(&mut self, stream: &mut store::Ptr) -> Request<()> {
        use super::peer::PollMessage::*;

        match stream.pending_recv.pop_front(&mut self.buffer) {
            Some(Event::Headers(Server(request))) => request,
            _ => panic!(),
        }
    }

    /// Called by the client to get pushed response
    pub fn poll_pushed(
        &mut self,
        cx: &Context,
        stream: &mut store::Ptr,
    ) -> Poll<Option<Result<(Request<()>, store::Key), proto::Error>>> {
        use super::peer::PollMessage::*;

        let mut ppp = stream.pending_push_promises.take();
        let pushed = ppp.pop(stream.store_mut()).map(|mut pushed| {
            match pushed.pending_recv.pop_front(&mut self.buffer) {
                Some(Event::Headers(Server(headers))) => (headers, pushed.key()),
                // When frames are pushed into the queue, it is verified that
                // the first frame is a HEADERS frame.
                _ => panic!("Headers not set on pushed stream"),
            }
        });
        stream.pending_push_promises = ppp;
        if let Some(p) = pushed {
            Poll::Ready(Some(Ok(p)))
        } else {
            let is_open = stream.state.ensure_recv_open()?;

            if is_open {
                stream.recv_task = Some(cx.waker().clone());
                Poll::Pending
            } else {
                Poll::Ready(None)
            }
        }
    }

    /// Called by the client to get the response
    pub fn poll_response(
        &mut self,
        cx: &Context,
        stream: &mut store::Ptr,
    ) -> Poll<Result<Response<()>, proto::Error>> {
        use super::peer::PollMessage::*;

        // If the buffer is not empty, then the first frame must be a HEADERS
        // frame or the user violated the contract.
        match stream.pending_recv.pop_front(&mut self.buffer) {
            Some(Event::Headers(Client(response))) => Poll::Ready(Ok(response)),
            Some(_) => panic!("poll_response called after response returned"),
            None => {
                stream.state.ensure_recv_open()?;

                stream.recv_task = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    /// Transition the stream based on receiving trailers
    pub fn recv_trailers(
        &mut self,
        frame: frame::Headers,
        stream: &mut store::Ptr,
    ) -> Result<(), RecvError> {
        // Transition the state
        stream.state.recv_close()?;

        if stream.ensure_content_length_zero().is_err() {
            proto_err!(stream: "recv_trailers: content-length is not zero; stream={:?};",  stream.id);
            return Err(RecvError::Stream {
                id: stream.id,
                reason: Reason::PROTOCOL_ERROR,
            });
        }

        let trailers = frame.into_fields();

        // Push the frame onto the stream's recv buffer
        stream
            .pending_recv
            .push_back(&mut self.buffer, Event::Trailers(trailers));
        stream.notify_recv();

        Ok(())
    }

    /// Releases capacity of the connection
    pub fn release_connection_capacity(&mut self, capacity: WindowSize, task: &mut Option<Waker>) {
        tracing::trace!(
            "release_connection_capacity; size={}, connection in_flight_data={}",
            capacity,
            self.in_flight_data,
        );

        // Decrement in-flight data
        self.in_flight_data -= capacity;

        // Assign capacity to connection
        self.flow.assign_capacity(capacity);

        if self.flow.unclaimed_capacity().is_some() {
            if let Some(task) = task.take() {
                task.wake();
            }
        }
    }

    /// Releases capacity back to the connection & stream
    pub fn release_capacity(
        &mut self,
        capacity: WindowSize,
        stream: &mut store::Ptr,
        task: &mut Option<Waker>,
    ) -> Result<(), UserError> {
        tracing::trace!("release_capacity; size={}", capacity);

        if capacity > stream.in_flight_recv_data {
            return Err(UserError::ReleaseCapacityTooBig);
        }

        self.release_connection_capacity(capacity, task);

        // Decrement in-flight data
        stream.in_flight_recv_data -= capacity;

        // Assign capacity to stream
        stream.recv_flow.assign_capacity(capacity);

        if stream.recv_flow.unclaimed_capacity().is_some() {
            // Queue the stream for sending the WINDOW_UPDATE frame.
            self.pending_window_updates.push(stream);

            if let Some(task) = task.take() {
                task.wake();
            }
        }

        Ok(())
    }

    /// Release any unclaimed capacity for a closed stream.
    pub fn release_closed_capacity(&mut self, stream: &mut store::Ptr, task: &mut Option<Waker>) {
        debug_assert_eq!(stream.ref_count, 0);

        if stream.in_flight_recv_data == 0 {
            return;
        }

        tracing::trace!(
            "auto-release closed stream ({:?}) capacity: {:?}",
            stream.id,
            stream.in_flight_recv_data,
        );

        self.release_connection_capacity(stream.in_flight_recv_data, task);
        stream.in_flight_recv_data = 0;

        self.clear_recv_buffer(stream);
    }

    /// Set the "target" connection window size.
    ///
    /// By default, all new connections start with 64kb of window size. As
    /// streams used and release capacity, we will send WINDOW_UPDATEs for the
    /// connection to bring it back up to the initial "target".
    ///
    /// Setting a target means that we will try to tell the peer about
    /// WINDOW_UPDATEs so the peer knows it has about `target` window to use
    /// for the whole connection.
    ///
    /// The `task` is an optional parked task for the `Connection` that might
    /// be blocked on needing more window capacity.
    pub fn set_target_connection_window(&mut self, target: WindowSize, task: &mut Option<Waker>) {
        tracing::trace!(
            "set_target_connection_window; target={}; available={}, reserved={}",
            target,
            self.flow.available(),
            self.in_flight_data,
        );

        // The current target connection window is our `available` plus any
        // in-flight data reserved by streams.
        //
        // Update the flow controller with the difference between the new
        // target and the current target.
        let current = (self.flow.available() + self.in_flight_data).checked_size();
        if target > current {
            self.flow.assign_capacity(target - current);
        } else {
            self.flow.claim_capacity(current - target);
        }

        // If changing the target capacity means we gained a bunch of capacity,
        // enough that we went over the update threshold, then schedule sending
        // a connection WINDOW_UPDATE.
        if self.flow.unclaimed_capacity().is_some() {
            if let Some(task) = task.take() {
                task.wake();
            }
        }
    }

    pub(crate) fn apply_local_settings(
        &mut self,
        settings: &frame::Settings,
        store: &mut Store,
    ) -> Result<(), RecvError> {
        let target = if let Some(val) = settings.initial_window_size() {
            val
        } else {
            return Ok(());
        };

        let old_sz = self.init_window_sz;
        self.init_window_sz = target;

        tracing::trace!("update_initial_window_size; new={}; old={}", target, old_sz,);

        // Per RFC 7540 §6.9.2:
        //
        // In addition to changing the flow-control window for streams that are
        // not yet active, a SETTINGS frame can alter the initial flow-control
        // window size for streams with active flow-control windows (that is,
        // streams in the "open" or "half-closed (remote)" state). When the
        // value of SETTINGS_INITIAL_WINDOW_SIZE changes, a receiver MUST adjust
        // the size of all stream flow-control windows that it maintains by the
        // difference between the new value and the old value.
        //
        // A change to `SETTINGS_INITIAL_WINDOW_SIZE` can cause the available
        // space in a flow-control window to become negative. A sender MUST
        // track the negative flow-control window and MUST NOT send new
        // flow-controlled frames until it receives WINDOW_UPDATE frames that
        // cause the flow-control window to become positive.

        if target < old_sz {
            // We must decrease the (local) window on every open stream.
            let dec = old_sz - target;
            tracing::trace!("decrementing all windows; dec={}", dec);

            store.for_each(|mut stream| {
                stream.recv_flow.dec_recv_window(dec);
                Ok(())
            })
        } else if target > old_sz {
            // We must increase the (local) window on every open stream.
            let inc = target - old_sz;
            tracing::trace!("incrementing all windows; inc={}", inc);
            store.for_each(|mut stream| {
                // XXX: Shouldn't the peer have already noticed our
                // overflow and sent us a GOAWAY?
                stream
                    .recv_flow
                    .inc_window(inc)
                    .map_err(RecvError::Connection)?;
                stream.recv_flow.assign_capacity(inc);
                Ok(())
            })
        } else {
            // size is the same... so do nothing
            Ok(())
        }
    }

    pub fn is_end_stream(&self, stream: &store::Ptr) -> bool {
        if !stream.state.is_recv_closed() {
            return false;
        }

        stream.pending_recv.is_empty()
    }

    pub fn recv_data(
        &mut self,
        frame: frame::Data,
        stream: &mut store::Ptr,
    ) -> Result<(), RecvError> {
        let sz = frame.payload().len();

        // This should have been enforced at the codec::FramedRead layer, so
        // this is just a sanity check.
        assert!(sz <= MAX_WINDOW_SIZE as usize);

        let sz = sz as WindowSize;

        let is_ignoring_frame = stream.state.is_local_reset();

        if !is_ignoring_frame && !stream.state.is_recv_streaming() {
            // TODO: There are cases where this can be a stream error of
            // STREAM_CLOSED instead...

            // Receiving a DATA frame when not expecting one is a protocol
            // error.
            proto_err!(conn: "unexpected DATA frame; stream={:?}", stream.id);
            return Err(RecvError::Connection(Reason::PROTOCOL_ERROR));
        }

        tracing::trace!(
            "recv_data; size={}; connection={}; stream={}",
            sz,
            self.flow.window_size(),
            stream.recv_flow.window_size()
        );

        if is_ignoring_frame {
            tracing::trace!(
                "recv_data; frame ignored on locally reset {:?} for some time",
                stream.id,
            );
            return self.ignore_data(sz);
        }

        // Ensure that there is enough capacity on the connection before acting
        // on the stream.
        self.consume_connection_window(sz)?;

        if stream.recv_flow.window_size() < sz {
            // http://httpwg.org/specs/rfc7540.html#WINDOW_UPDATE
            // > A receiver MAY respond with a stream error (Section 5.4.2) or
            // > connection error (Section 5.4.1) of type FLOW_CONTROL_ERROR if
            // > it is unable to accept a frame.
            //
            // So, for violating the **stream** window, we can send either a
            // stream or connection error. We've opted to send a stream
            // error.
            return Err(RecvError::Stream {
                id: stream.id,
                reason: Reason::FLOW_CONTROL_ERROR,
            });
        }

        if stream.dec_content_length(frame.payload().len()).is_err() {
            proto_err!(stream:
                "recv_data: content-length overflow; stream={:?}; len={:?}",
                stream.id,
                frame.payload().len(),
            );
            return Err(RecvError::Stream {
                id: stream.id,
                reason: Reason::PROTOCOL_ERROR,
            });
        }

        if frame.is_end_stream() {
            if stream.ensure_content_length_zero().is_err() {
                proto_err!(stream:
                    "recv_data: content-length underflow; stream={:?}; len={:?}",
                    stream.id,
                    frame.payload().len(),
                );
                return Err(RecvError::Stream {
                    id: stream.id,
                    reason: Reason::PROTOCOL_ERROR,
                });
            }

            if stream.state.recv_close().is_err() {
                proto_err!(conn: "recv_data: failed to transition to closed state; stream={:?}", stream.id);
                return Err(RecvError::Connection(Reason::PROTOCOL_ERROR));
            }
        }

        // Update stream level flow control
        stream.recv_flow.send_data(sz);

        // Track the data as in-flight
        stream.in_flight_recv_data += sz;

        let event = Event::Data(frame.into_payload());

        // Push the frame onto the recv buffer
        stream.pending_recv.push_back(&mut self.buffer, event);
        stream.notify_recv();

        Ok(())
    }

    pub fn ignore_data(&mut self, sz: WindowSize) -> Result<(), RecvError> {
        // Ensure that there is enough capacity on the connection...
        self.consume_connection_window(sz)?;

        // Since we are ignoring this frame,
        // we aren't returning the frame to the user. That means they
        // have no way to release the capacity back to the connection. So
        // we have to release it automatically.
        //
        // This call doesn't send a WINDOW_UPDATE immediately, just marks
        // the capacity as available to be reclaimed. When the available
        // capacity meets a threshold, a WINDOW_UPDATE is then sent.
        self.release_connection_capacity(sz, &mut None);
        Ok(())
    }

    pub fn consume_connection_window(&mut self, sz: WindowSize) -> Result<(), RecvError> {
        if self.flow.window_size() < sz {
            tracing::debug!(
                "connection error FLOW_CONTROL_ERROR -- window_size ({:?}) < sz ({:?});",
                self.flow.window_size(),
                sz,
            );
            return Err(RecvError::Connection(Reason::FLOW_CONTROL_ERROR));
        }

        // Update connection level flow control
        self.flow.send_data(sz);

        // Track the data as in-flight
        self.in_flight_data += sz;
        Ok(())
    }

    pub fn recv_push_promise(
        &mut self,
        frame: frame::PushPromise,
        stream: &mut store::Ptr,
    ) -> Result<(), RecvError> {
        stream.state.reserve_remote()?;
        if frame.is_over_size() {
            // A frame is over size if the decoded header block was bigger than
            // SETTINGS_MAX_HEADER_LIST_SIZE.
            //
            // > A server that receives a larger header block than it is willing
            // > to handle can send an HTTP 431 (Request Header Fields Too
            // > Large) status code [RFC6585]. A client can discard responses
            // > that it cannot process.
            //
            // So, if peer is a server, we'll send a 431. In either case,
            // an error is recorded, which will send a REFUSED_STREAM,
            // since we don't want any of the data frames either.
            tracing::debug!(
                "stream error REFUSED_STREAM -- recv_push_promise: \
                 headers frame is over size; promised_id={:?};",
                frame.promised_id(),
            );
            return Err(RecvError::Stream {
                id: frame.promised_id(),
                reason: Reason::REFUSED_STREAM,
            });
        }

        let promised_id = frame.promised_id();
        let (pseudo, fields) = frame.into_parts();
        let req = crate::server::Peer::convert_poll_message(pseudo, fields, promised_id)?;

        if let Err(e) = frame::PushPromise::validate_request(&req) {
            use PushPromiseHeaderError::*;
            match e {
                NotSafeAndCacheable => proto_err!(
                    stream:
                    "recv_push_promise: method {} is not safe and cacheable; promised_id={:?}",
                    req.method(),
                    promised_id,
                ),
                InvalidContentLength(e) => proto_err!(
                    stream:
                    "recv_push_promise; promised request has invalid content-length {:?}; promised_id={:?}",
                    e,
                    promised_id,
                ),
            }
            return Err(RecvError::Stream {
                id: promised_id,
                reason: Reason::PROTOCOL_ERROR,
            });
        }

        use super::peer::PollMessage::*;
        stream
            .pending_recv
            .push_back(&mut self.buffer, Event::Headers(Server(req)));
        stream.notify_recv();
        Ok(())
    }

    /// Ensures that `id` is not in the `Idle` state.
    pub fn ensure_not_idle(&self, id: StreamId) -> Result<(), Reason> {
        if let Ok(next) = self.next_stream_id {
            if id >= next {
                tracing::debug!(
                    "stream ID implicitly closed, PROTOCOL_ERROR; stream={:?}",
                    id
                );
                return Err(Reason::PROTOCOL_ERROR);
            }
        }
        // if next_stream_id is overflowed, that's ok.

        Ok(())
    }

    /// Handle remote sending an explicit RST_STREAM.
    pub fn recv_reset(&mut self, frame: frame::Reset, stream: &mut Stream) {
        // Notify the stream
        stream
            .state
            .recv_reset(frame.reason(), stream.is_pending_send);

        stream.notify_send();
        stream.notify_recv();
    }

    /// Handle a received error
    pub fn recv_err(&mut self, err: &proto::Error, stream: &mut Stream) {
        // Receive an error
        stream.state.recv_err(err);

        // If a receiver is waiting, notify it
        stream.notify_send();
        stream.notify_recv();
    }

    pub fn go_away(&mut self, last_processed_id: StreamId) {
        assert!(self.max_stream_id >= last_processed_id);
        self.max_stream_id = last_processed_id;
    }

    pub fn recv_eof(&mut self, stream: &mut Stream) {
        stream.state.recv_eof();
        stream.notify_send();
        stream.notify_recv();
    }

    pub(super) fn clear_recv_buffer(&mut self, stream: &mut Stream) {
        while let Some(_) = stream.pending_recv.pop_front(&mut self.buffer) {
            // drop it
        }
    }

    /// Get the max ID of streams we can receive.
    ///
    /// This gets lowered if we send a GOAWAY frame.
    pub fn max_stream_id(&self) -> StreamId {
        self.max_stream_id
    }

    pub fn next_stream_id(&self) -> Result<StreamId, RecvError> {
        if let Ok(id) = self.next_stream_id {
            Ok(id)
        } else {
            Err(RecvError::Connection(Reason::PROTOCOL_ERROR))
        }
    }

    pub fn may_have_created_stream(&self, id: StreamId) -> bool {
        if let Ok(next_id) = self.next_stream_id {
            // Peer::is_local_init should have been called beforehand
            debug_assert_eq!(id.is_server_initiated(), next_id.is_server_initiated(),);
            id < next_id
        } else {
            true
        }
    }

    /// Returns true if the remote peer can reserve a stream with the given ID.
    pub fn ensure_can_reserve(&self) -> Result<(), RecvError> {
        if !self.is_push_enabled {
            proto_err!(conn: "recv_push_promise: push is disabled");
            return Err(RecvError::Connection(Reason::PROTOCOL_ERROR));
        }

        Ok(())
    }

    /// Add a locally reset stream to queue to be eventually reaped.
    pub fn enqueue_reset_expiration(&mut self, stream: &mut store::Ptr, counts: &mut Counts) {
        if !stream.state.is_local_reset() || stream.is_pending_reset_expiration() {
            return;
        }

        tracing::trace!("enqueue_reset_expiration; {:?}", stream.id);

        if !counts.can_inc_num_reset_streams() {
            // try to evict 1 stream if possible
            // if max allow is 0, this won't be able to evict,
            // and then we'll just bail after
            if let Some(evicted) = self.pending_reset_expired.pop(stream.store_mut()) {
                counts.transition_after(evicted, true);
            }
        }

        if counts.can_inc_num_reset_streams() {
            counts.inc_num_reset_streams();
            self.pending_reset_expired.push(stream);
        }
    }

    /// Send any pending refusals.
    pub fn send_pending_refusal<T, B>(
        &mut self,
        cx: &mut Context,
        dst: &mut Codec<T, Prioritized<B>>,
    ) -> Poll<io::Result<()>>
    where
        T: AsyncWrite + Unpin,
        B: Buf,
    {
        if let Some(stream_id) = self.refused {
            ready!(dst.poll_ready(cx))?;

            // Create the RST_STREAM frame
            let frame = frame::Reset::new(stream_id, Reason::REFUSED_STREAM);

            // Buffer the frame
            dst.buffer(frame.into()).expect("invalid RST_STREAM frame");
        }

        self.refused = None;

        Poll::Ready(Ok(()))
    }

    pub fn clear_expired_reset_streams(&mut self, store: &mut Store, counts: &mut Counts) {
        let now = Instant::now();
        let reset_duration = self.reset_duration;
        while let Some(stream) = self.pending_reset_expired.pop_if(store, |stream| {
            let reset_at = stream.reset_at.expect("reset_at must be set if in queue");
            now - reset_at > reset_duration
        }) {
            counts.transition_after(stream, true);
        }
    }

    pub fn clear_queues(
        &mut self,
        clear_pending_accept: bool,
        store: &mut Store,
        counts: &mut Counts,
    ) {
        self.clear_stream_window_update_queue(store, counts);
        self.clear_all_reset_streams(store, counts);

        if clear_pending_accept {
            self.clear_all_pending_accept(store, counts);
        }
    }

    fn clear_stream_window_update_queue(&mut self, store: &mut Store, counts: &mut Counts) {
        while let Some(stream) = self.pending_window_updates.pop(store) {
            counts.transition(stream, |_, stream| {
                tracing::trace!("clear_stream_window_update_queue; stream={:?}", stream.id);
            })
        }
    }

    /// Called on EOF
    fn clear_all_reset_streams(&mut self, store: &mut Store, counts: &mut Counts) {
        while let Some(stream) = self.pending_reset_expired.pop(store) {
            counts.transition_after(stream, true);
        }
    }

    fn clear_all_pending_accept(&mut self, store: &mut Store, counts: &mut Counts) {
        while let Some(stream) = self.pending_accept.pop(store) {
            counts.transition_after(stream, false);
        }
    }

    pub fn poll_complete<T, B>(
        &mut self,
        cx: &mut Context,
        store: &mut Store,
        counts: &mut Counts,
        dst: &mut Codec<T, Prioritized<B>>,
    ) -> Poll<io::Result<()>>
    where
        T: AsyncWrite + Unpin,
        B: Buf,
    {
        // Send any pending connection level window updates
        ready!(self.send_connection_window_update(cx, dst))?;

        // Send any pending stream level window updates
        ready!(self.send_stream_window_updates(cx, store, counts, dst))?;

        Poll::Ready(Ok(()))
    }

    /// Send connection level window update
    fn send_connection_window_update<T, B>(
        &mut self,
        cx: &mut Context,
        dst: &mut Codec<T, Prioritized<B>>,
    ) -> Poll<io::Result<()>>
    where
        T: AsyncWrite + Unpin,
        B: Buf,
    {
        if let Some(incr) = self.flow.unclaimed_capacity() {
            let frame = frame::WindowUpdate::new(StreamId::zero(), incr);

            // Ensure the codec has capacity
            ready!(dst.poll_ready(cx))?;

            // Buffer the WINDOW_UPDATE frame
            dst.buffer(frame.into())
                .expect("invalid WINDOW_UPDATE frame");

            // Update flow control
            self.flow
                .inc_window(incr)
                .expect("unexpected flow control state");
        }

        Poll::Ready(Ok(()))
    }

    /// Send stream level window update
    pub fn send_stream_window_updates<T, B>(
        &mut self,
        cx: &mut Context,
        store: &mut Store,
        counts: &mut Counts,
        dst: &mut Codec<T, Prioritized<B>>,
    ) -> Poll<io::Result<()>>
    where
        T: AsyncWrite + Unpin,
        B: Buf,
    {
        loop {
            // Ensure the codec has capacity
            ready!(dst.poll_ready(cx))?;

            // Get the next stream
            let stream = match self.pending_window_updates.pop(store) {
                Some(stream) => stream,
                None => return Poll::Ready(Ok(())),
            };

            counts.transition(stream, |_, stream| {
                tracing::trace!("pending_window_updates -- pop; stream={:?}", stream.id);
                debug_assert!(!stream.is_pending_window_update);

                if !stream.state.is_recv_streaming() {
                    // No need to send window updates on the stream if the stream is
                    // no longer receiving data.
                    //
                    // TODO: is this correct? We could possibly send a window
                    // update on a ReservedRemote stream if we already know
                    // we want to stream the data faster...
                    return;
                }

                // TODO: de-dup
                if let Some(incr) = stream.recv_flow.unclaimed_capacity() {
                    // Create the WINDOW_UPDATE frame
                    let frame = frame::WindowUpdate::new(stream.id, incr);

                    // Buffer it
                    dst.buffer(frame.into())
                        .expect("invalid WINDOW_UPDATE frame");

                    // Update flow control
                    stream
                        .recv_flow
                        .inc_window(incr)
                        .expect("unexpected flow control state");
                }
            })
        }
    }

    pub fn next_incoming(&mut self, store: &mut Store) -> Option<store::Key> {
        self.pending_accept.pop(store).map(|ptr| ptr.key())
    }

    pub fn poll_data(
        &mut self,
        cx: &Context,
        stream: &mut Stream,
    ) -> Poll<Option<Result<Bytes, proto::Error>>> {
        // TODO: Return error when the stream is reset
        match stream.pending_recv.pop_front(&mut self.buffer) {
            Some(Event::Data(payload)) => Poll::Ready(Some(Ok(payload))),
            Some(event) => {
                // Frame is trailer
                stream.pending_recv.push_front(&mut self.buffer, event);

                // Notify the recv task. This is done just in case
                // `poll_trailers` was called.
                //
                // It is very likely that `notify_recv` will just be a no-op (as
                // the task will be None), so this isn't really much of a
                // performance concern. It also means we don't have to track
                // state to see if `poll_trailers` was called before `poll_data`
                // returned `None`.
                stream.notify_recv();

                // No more data frames
                Poll::Ready(None)
            }
            None => self.schedule_recv(cx, stream),
        }
    }

    pub fn poll_trailers(
        &mut self,
        cx: &Context,
        stream: &mut Stream,
    ) -> Poll<Option<Result<HeaderMap, proto::Error>>> {
        match stream.pending_recv.pop_front(&mut self.buffer) {
            Some(Event::Trailers(trailers)) => Poll::Ready(Some(Ok(trailers))),
            Some(event) => {
                // Frame is not trailers.. not ready to poll trailers yet.
                stream.pending_recv.push_front(&mut self.buffer, event);

                Poll::Pending
            }
            None => self.schedule_recv(cx, stream),
        }
    }

    fn schedule_recv<T>(
        &mut self,
        cx: &Context,
        stream: &mut Stream,
    ) -> Poll<Option<Result<T, proto::Error>>> {
        if stream.state.ensure_recv_open()? {
            // Request to get notified once more frames arrive
            stream.recv_task = Some(cx.waker().clone());
            Poll::Pending
        } else {
            // No more frames will be received
            Poll::Ready(None)
        }
    }
}

// ===== impl Open =====

impl Open {
    pub fn is_push_promise(&self) -> bool {
        use self::Open::*;

        match *self {
            PushPromise => true,
            _ => false,
        }
    }
}

// ===== impl RecvHeaderBlockError =====

impl<T> From<RecvError> for RecvHeaderBlockError<T> {
    fn from(err: RecvError) -> Self {
        RecvHeaderBlockError::State(err)
    }
}
