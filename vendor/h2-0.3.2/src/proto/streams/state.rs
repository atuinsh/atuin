use std::io;

use crate::codec::UserError::*;
use crate::codec::{RecvError, UserError};
use crate::frame::{self, Reason};
use crate::proto::{self, PollReset};

use self::Inner::*;
use self::Peer::*;

/// Represents the state of an H2 stream
///
/// ```not_rust
///                              +--------+
///                      send PP |        | recv PP
///                     ,--------|  idle  |--------.
///                    /         |        |         \
///                   v          +--------+          v
///            +----------+          |           +----------+
///            |          |          | send H /  |          |
///     ,------| reserved |          | recv H    | reserved |------.
///     |      | (local)  |          |           | (remote) |      |
///     |      +----------+          v           +----------+      |
///     |          |             +--------+             |          |
///     |          |     recv ES |        | send ES     |          |
///     |   send H |     ,-------|  open  |-------.     | recv H   |
///     |          |    /        |        |        \    |          |
///     |          v   v         +--------+         v   v          |
///     |      +----------+          |           +----------+      |
///     |      |   half   |          |           |   half   |      |
///     |      |  closed  |          | send R /  |  closed  |      |
///     |      | (remote) |          | recv R    | (local)  |      |
///     |      +----------+          |           +----------+      |
///     |           |                |                 |           |
///     |           | send ES /      |       recv ES / |           |
///     |           | send R /       v        send R / |           |
///     |           | recv R     +--------+   recv R   |           |
///     | send R /  `----------->|        |<-----------'  send R / |
///     | recv R                 | closed |               recv R   |
///     `----------------------->|        |<----------------------'
///                              +--------+
///
///        send:   endpoint sends this frame
///        recv:   endpoint receives this frame
///
///        H:  HEADERS frame (with implied CONTINUATIONs)
///        PP: PUSH_PROMISE frame (with implied CONTINUATIONs)
///        ES: END_STREAM flag
///        R:  RST_STREAM frame
/// ```
#[derive(Debug, Clone)]
pub struct State {
    inner: Inner,
}

#[derive(Debug, Clone, Copy)]
enum Inner {
    Idle,
    // TODO: these states shouldn't count against concurrency limits:
    ReservedLocal,
    ReservedRemote,
    Open { local: Peer, remote: Peer },
    HalfClosedLocal(Peer), // TODO: explicitly name this value
    HalfClosedRemote(Peer),
    Closed(Cause),
}

#[derive(Debug, Copy, Clone)]
enum Peer {
    AwaitingHeaders,
    Streaming,
}

#[derive(Debug, Copy, Clone)]
enum Cause {
    EndStream,
    Proto(Reason),
    LocallyReset(Reason),
    Io,

    /// This indicates to the connection that a reset frame must be sent out
    /// once the send queue has been flushed.
    ///
    /// Examples of when this could happen:
    /// - User drops all references to a stream, so we want to CANCEL the it.
    /// - Header block size was too large, so we want to REFUSE, possibly
    ///   after sending a 431 response frame.
    Scheduled(Reason),
}

impl State {
    /// Opens the send-half of a stream if it is not already open.
    pub fn send_open(&mut self, eos: bool) -> Result<(), UserError> {
        let local = Streaming;

        self.inner = match self.inner {
            Idle => {
                if eos {
                    HalfClosedLocal(AwaitingHeaders)
                } else {
                    Open {
                        local,
                        remote: AwaitingHeaders,
                    }
                }
            }
            Open {
                local: AwaitingHeaders,
                remote,
            } => {
                if eos {
                    HalfClosedLocal(remote)
                } else {
                    Open { local, remote }
                }
            }
            HalfClosedRemote(AwaitingHeaders) | ReservedLocal => {
                if eos {
                    Closed(Cause::EndStream)
                } else {
                    HalfClosedRemote(local)
                }
            }
            _ => {
                // All other transitions result in a protocol error
                return Err(UnexpectedFrameType);
            }
        };

        Ok(())
    }

    /// Opens the receive-half of the stream when a HEADERS frame is received.
    ///
    /// Returns true if this transitions the state to Open.
    pub fn recv_open(&mut self, frame: &frame::Headers) -> Result<bool, RecvError> {
        let mut initial = false;
        let eos = frame.is_end_stream();

        self.inner = match self.inner {
            Idle => {
                initial = true;

                if eos {
                    HalfClosedRemote(AwaitingHeaders)
                } else {
                    Open {
                        local: AwaitingHeaders,
                        remote: if frame.is_informational() {
                            tracing::trace!("skipping 1xx response headers");
                            AwaitingHeaders
                        } else {
                            Streaming
                        },
                    }
                }
            }
            ReservedRemote => {
                initial = true;

                if eos {
                    Closed(Cause::EndStream)
                } else if frame.is_informational() {
                    tracing::trace!("skipping 1xx response headers");
                    ReservedRemote
                } else {
                    HalfClosedLocal(Streaming)
                }
            }
            Open {
                local,
                remote: AwaitingHeaders,
            } => {
                if eos {
                    HalfClosedRemote(local)
                } else {
                    Open {
                        local,
                        remote: if frame.is_informational() {
                            tracing::trace!("skipping 1xx response headers");
                            AwaitingHeaders
                        } else {
                            Streaming
                        },
                    }
                }
            }
            HalfClosedLocal(AwaitingHeaders) => {
                if eos {
                    Closed(Cause::EndStream)
                } else if frame.is_informational() {
                    tracing::trace!("skipping 1xx response headers");
                    HalfClosedLocal(AwaitingHeaders)
                } else {
                    HalfClosedLocal(Streaming)
                }
            }
            state => {
                // All other transitions result in a protocol error
                proto_err!(conn: "recv_open: in unexpected state {:?}", state);
                return Err(RecvError::Connection(Reason::PROTOCOL_ERROR));
            }
        };

        Ok(initial)
    }

    /// Transition from Idle -> ReservedRemote
    pub fn reserve_remote(&mut self) -> Result<(), RecvError> {
        match self.inner {
            Idle => {
                self.inner = ReservedRemote;
                Ok(())
            }
            state => {
                proto_err!(conn: "reserve_remote: in unexpected state {:?}", state);
                Err(RecvError::Connection(Reason::PROTOCOL_ERROR))
            }
        }
    }

    /// Transition from Idle -> ReservedLocal
    pub fn reserve_local(&mut self) -> Result<(), UserError> {
        match self.inner {
            Idle => {
                self.inner = ReservedLocal;
                Ok(())
            }
            _ => Err(UserError::UnexpectedFrameType),
        }
    }

    /// Indicates that the remote side will not send more data to the local.
    pub fn recv_close(&mut self) -> Result<(), RecvError> {
        match self.inner {
            Open { local, .. } => {
                // The remote side will continue to receive data.
                tracing::trace!("recv_close: Open => HalfClosedRemote({:?})", local);
                self.inner = HalfClosedRemote(local);
                Ok(())
            }
            HalfClosedLocal(..) => {
                tracing::trace!("recv_close: HalfClosedLocal => Closed");
                self.inner = Closed(Cause::EndStream);
                Ok(())
            }
            state => {
                proto_err!(conn: "recv_close: in unexpected state {:?}", state);
                Err(RecvError::Connection(Reason::PROTOCOL_ERROR))
            }
        }
    }

    /// The remote explicitly sent a RST_STREAM.
    ///
    /// # Arguments
    /// - `reason`: the reason field of the received RST_STREAM frame.
    /// - `queued`: true if this stream has frames in the pending send queue.
    pub fn recv_reset(&mut self, reason: Reason, queued: bool) {
        match self.inner {
            // If the stream is already in a `Closed` state, do nothing,
            // provided that there are no frames still in the send queue.
            Closed(..) if !queued => {}
            // A notionally `Closed` stream may still have queued frames in
            // the following cases:
            //
            // - if the cause is `Cause::Scheduled(..)` (i.e. we have not
            //   actually closed the stream yet).
            // - if the cause is `Cause::EndStream`: we transition to this
            //   state when an EOS frame is *enqueued* (so that it's invalid
            //   to enqueue more frames), not when the EOS frame is *sent*;
            //   therefore, there may still be frames ahead of the EOS frame
            //   in the send queue.
            //
            // In either of these cases, we want to overwrite the stream's
            // previous state with the received RST_STREAM, so that the queue
            // will be cleared by `Prioritize::pop_frame`.
            state => {
                tracing::trace!(
                    "recv_reset; reason={:?}; state={:?}; queued={:?}",
                    reason,
                    state,
                    queued
                );
                self.inner = Closed(Cause::Proto(reason));
            }
        }
    }

    /// We noticed a protocol error.
    pub fn recv_err(&mut self, err: &proto::Error) {
        use crate::proto::Error::*;

        match self.inner {
            Closed(..) => {}
            _ => {
                tracing::trace!("recv_err; err={:?}", err);
                self.inner = Closed(match *err {
                    Proto(reason) => Cause::LocallyReset(reason),
                    Io(..) => Cause::Io,
                });
            }
        }
    }

    pub fn recv_eof(&mut self) {
        match self.inner {
            Closed(..) => {}
            s => {
                tracing::trace!("recv_eof; state={:?}", s);
                self.inner = Closed(Cause::Io);
            }
        }
    }

    /// Indicates that the local side will not send more data to the local.
    pub fn send_close(&mut self) {
        match self.inner {
            Open { remote, .. } => {
                // The remote side will continue to receive data.
                tracing::trace!("send_close: Open => HalfClosedLocal({:?})", remote);
                self.inner = HalfClosedLocal(remote);
            }
            HalfClosedRemote(..) => {
                tracing::trace!("send_close: HalfClosedRemote => Closed");
                self.inner = Closed(Cause::EndStream);
            }
            state => panic!("send_close: unexpected state {:?}", state),
        }
    }

    /// Set the stream state to reset locally.
    pub fn set_reset(&mut self, reason: Reason) {
        self.inner = Closed(Cause::LocallyReset(reason));
    }

    /// Set the stream state to a scheduled reset.
    pub fn set_scheduled_reset(&mut self, reason: Reason) {
        debug_assert!(!self.is_closed());
        self.inner = Closed(Cause::Scheduled(reason));
    }

    pub fn get_scheduled_reset(&self) -> Option<Reason> {
        match self.inner {
            Closed(Cause::Scheduled(reason)) => Some(reason),
            _ => None,
        }
    }

    pub fn is_scheduled_reset(&self) -> bool {
        match self.inner {
            Closed(Cause::Scheduled(..)) => true,
            _ => false,
        }
    }

    pub fn is_local_reset(&self) -> bool {
        match self.inner {
            Closed(Cause::LocallyReset(_)) => true,
            Closed(Cause::Scheduled(..)) => true,
            _ => false,
        }
    }

    /// Returns true if the stream is already reset.
    pub fn is_reset(&self) -> bool {
        match self.inner {
            Closed(Cause::EndStream) => false,
            Closed(_) => true,
            _ => false,
        }
    }

    pub fn is_send_streaming(&self) -> bool {
        match self.inner {
            Open {
                local: Streaming, ..
            } => true,
            HalfClosedRemote(Streaming) => true,
            _ => false,
        }
    }

    /// Returns true when the stream is in a state to receive headers
    pub fn is_recv_headers(&self) -> bool {
        match self.inner {
            Idle => true,
            Open {
                remote: AwaitingHeaders,
                ..
            } => true,
            HalfClosedLocal(AwaitingHeaders) => true,
            ReservedRemote => true,
            _ => false,
        }
    }

    pub fn is_recv_streaming(&self) -> bool {
        match self.inner {
            Open {
                remote: Streaming, ..
            } => true,
            HalfClosedLocal(Streaming) => true,
            _ => false,
        }
    }

    pub fn is_closed(&self) -> bool {
        match self.inner {
            Closed(_) => true,
            _ => false,
        }
    }

    pub fn is_recv_closed(&self) -> bool {
        match self.inner {
            Closed(..) | HalfClosedRemote(..) | ReservedLocal => true,
            _ => false,
        }
    }

    pub fn is_send_closed(&self) -> bool {
        match self.inner {
            Closed(..) | HalfClosedLocal(..) | ReservedRemote => true,
            _ => false,
        }
    }

    pub fn is_idle(&self) -> bool {
        match self.inner {
            Idle => true,
            _ => false,
        }
    }

    pub fn ensure_recv_open(&self) -> Result<bool, proto::Error> {
        // TODO: Is this correct?
        match self.inner {
            Closed(Cause::Proto(reason))
            | Closed(Cause::LocallyReset(reason))
            | Closed(Cause::Scheduled(reason)) => Err(proto::Error::Proto(reason)),
            Closed(Cause::Io) => Err(proto::Error::Io(io::ErrorKind::BrokenPipe.into())),
            Closed(Cause::EndStream) | HalfClosedRemote(..) | ReservedLocal => Ok(false),
            _ => Ok(true),
        }
    }

    /// Returns a reason if the stream has been reset.
    pub(super) fn ensure_reason(&self, mode: PollReset) -> Result<Option<Reason>, crate::Error> {
        match self.inner {
            Closed(Cause::Proto(reason))
            | Closed(Cause::LocallyReset(reason))
            | Closed(Cause::Scheduled(reason)) => Ok(Some(reason)),
            Closed(Cause::Io) => Err(proto::Error::Io(io::ErrorKind::BrokenPipe.into()).into()),
            Open {
                local: Streaming, ..
            }
            | HalfClosedRemote(Streaming) => match mode {
                PollReset::AwaitingHeaders => Err(UserError::PollResetAfterSendResponse.into()),
                PollReset::Streaming => Ok(None),
            },
            _ => Ok(None),
        }
    }
}

impl Default for State {
    fn default() -> State {
        State { inner: Inner::Idle }
    }
}

impl Default for Peer {
    fn default() -> Self {
        AwaitingHeaders
    }
}
