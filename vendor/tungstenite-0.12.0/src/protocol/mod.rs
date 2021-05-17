//! Generic WebSocket message stream.

pub mod frame;

mod message;

pub use self::{frame::CloseFrame, message::Message};

use log::*;
use std::{
    collections::VecDeque,
    io::{ErrorKind as IoErrorKind, Read, Write},
    mem::replace,
};

use self::{
    frame::{
        coding::{CloseCode, Control as OpCtl, Data as OpData, OpCode},
        Frame, FrameCodec,
    },
    message::{IncompleteMessage, IncompleteMessageType},
};
use crate::{
    error::{Error, Result},
    util::NonBlockingResult,
};

/// Indicates a Client or Server role of the websocket
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// This socket is a server
    Server,
    /// This socket is a client
    Client,
}

/// The configuration for WebSocket connection.
#[derive(Debug, Clone, Copy)]
pub struct WebSocketConfig {
    /// The size of the send queue. You can use it to turn on/off the backpressure features. `None`
    /// means here that the size of the queue is unlimited. The default value is the unlimited
    /// queue.
    pub max_send_queue: Option<usize>,
    /// The maximum size of a message. `None` means no size limit. The default value is 64 MiB
    /// which should be reasonably big for all normal use-cases but small enough to prevent
    /// memory eating by a malicious user.
    pub max_message_size: Option<usize>,
    /// The maximum size of a single message frame. `None` means no size limit. The limit is for
    /// frame payload NOT including the frame header. The default value is 16 MiB which should
    /// be reasonably big for all normal use-cases but small enough to prevent memory eating
    /// by a malicious user.
    pub max_frame_size: Option<usize>,
    /// When set to `true`, the server will accept and handle unmasked frames
    /// from the client. According to the RFC 6455, the server must close the
    /// connection to the client in such cases, however it seems like there are
    /// some popular libraries that are sending unmasked frames, ignoring the RFC.
    /// By default this option is set to `false`, i.e. according to RFC 6455.
    pub accept_unmasked_frames: bool,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        WebSocketConfig {
            max_send_queue: None,
            max_message_size: Some(64 << 20),
            max_frame_size: Some(16 << 20),
            accept_unmasked_frames: false,
        }
    }
}

/// WebSocket input-output stream.
///
/// This is THE structure you want to create to be able to speak the WebSocket protocol.
/// It may be created by calling `connect`, `accept` or `client` functions.
#[derive(Debug)]
pub struct WebSocket<Stream> {
    /// The underlying socket.
    socket: Stream,
    /// The context for managing a WebSocket.
    context: WebSocketContext,
}

impl<Stream> WebSocket<Stream> {
    /// Convert a raw socket into a WebSocket without performing a handshake.
    ///
    /// Call this function if you're using Tungstenite as a part of a web framework
    /// or together with an existing one. If you need an initial handshake, use
    /// `connect()` or `accept()` functions of the crate to construct a websocket.
    pub fn from_raw_socket(stream: Stream, role: Role, config: Option<WebSocketConfig>) -> Self {
        WebSocket { socket: stream, context: WebSocketContext::new(role, config) }
    }

    /// Convert a raw socket into a WebSocket without performing a handshake.
    ///
    /// Call this function if you're using Tungstenite as a part of a web framework
    /// or together with an existing one. If you need an initial handshake, use
    /// `connect()` or `accept()` functions of the crate to construct a websocket.
    pub fn from_partially_read(
        stream: Stream,
        part: Vec<u8>,
        role: Role,
        config: Option<WebSocketConfig>,
    ) -> Self {
        WebSocket {
            socket: stream,
            context: WebSocketContext::from_partially_read(part, role, config),
        }
    }

    /// Returns a shared reference to the inner stream.
    pub fn get_ref(&self) -> &Stream {
        &self.socket
    }
    /// Returns a mutable reference to the inner stream.
    pub fn get_mut(&mut self) -> &mut Stream {
        &mut self.socket
    }

    /// Change the configuration.
    pub fn set_config(&mut self, set_func: impl FnOnce(&mut WebSocketConfig)) {
        self.context.set_config(set_func)
    }

    /// Read the configuration.
    pub fn get_config(&self) -> &WebSocketConfig {
        self.context.get_config()
    }

    /// Check if it is possible to read messages.
    ///
    /// Reading is impossible after receiving `Message::Close`. It is still possible after
    /// sending close frame since the peer still may send some data before confirming close.
    pub fn can_read(&self) -> bool {
        self.context.can_read()
    }

    /// Check if it is possible to write messages.
    ///
    /// Writing gets impossible immediately after sending or receiving `Message::Close`.
    pub fn can_write(&self) -> bool {
        self.context.can_write()
    }
}

impl<Stream: Read + Write> WebSocket<Stream> {
    /// Read a message from stream, if possible.
    ///
    /// This will queue responses to ping and close messages to be sent. It will call
    /// `write_pending` before trying to read in order to make sure that those responses
    /// make progress even if you never call `write_pending`. That does mean that they
    /// get sent out earliest on the next call to `read_message`, `write_message` or `write_pending`.
    ///
    /// ## Closing the connection
    /// When the remote endpoint decides to close the connection this will return
    /// the close message with an optional close frame.
    ///
    /// You should continue calling `read_message`, `write_message` or `write_pending` to drive
    /// the reply to the close frame until [Error::ConnectionClosed] is returned. Once that happens
    /// it is safe to drop the underlying connection.
    pub fn read_message(&mut self) -> Result<Message> {
        self.context.read_message(&mut self.socket)
    }

    /// Send a message to stream, if possible.
    ///
    /// WebSocket will buffer a configurable number of messages at a time, except to reply to Ping
    /// requests. A Pong reply will jump the queue because the
    /// [websocket RFC](https://tools.ietf.org/html/rfc6455#section-5.5.2) specifies it should be sent
    /// as soon as is practical.
    ///
    /// Note that upon receiving a ping message, tungstenite cues a pong reply automatically.
    /// When you call either `read_message`, `write_message` or `write_pending` next it will try to send
    /// that pong out if the underlying connection can take more data. This means you should not
    /// respond to ping frames manually.
    ///
    /// You can however send pong frames manually in order to indicate a unidirectional heartbeat
    /// as described in [RFC 6455](https://tools.ietf.org/html/rfc6455#section-5.5.3). Note that
    /// if `read_message` returns a ping, you should call `write_pending` until it doesn't return
    /// WouldBlock before passing a pong to `write_message`, otherwise the response to the
    /// ping will not be sent, but rather replaced by your custom pong message.
    ///
    /// ## Errors
    /// - If the WebSocket's send queue is full, `SendQueueFull` will be returned
    /// along with the passed message. Otherwise, the message is queued and Ok(()) is returned.
    /// - If the connection is closed and should be dropped, this will return [Error::ConnectionClosed].
    /// - If you try again after [Error::ConnectionClosed] was returned either from here or from `read_message`,
    ///   [Error::AlreadyClosed] will be returned. This indicates a program error on your part.
    /// - [Error::Io] is returned if the underlying connection returns an error
    ///   (consider these fatal except for WouldBlock).
    /// - [Error::Capacity] if your message size is bigger than the configured max message size.
    pub fn write_message(&mut self, message: Message) -> Result<()> {
        self.context.write_message(&mut self.socket, message)
    }

    /// Flush the pending send queue.
    pub fn write_pending(&mut self) -> Result<()> {
        self.context.write_pending(&mut self.socket)
    }

    /// Close the connection.
    ///
    /// This function guarantees that the close frame will be queued.
    /// There is no need to call it again. Calling this function is
    /// the same as calling `write_message(Message::Close(..))`.
    ///
    /// After queing the close frame you should continue calling `read_message` or
    /// `write_pending` to drive the close handshake to completion.
    ///
    /// The websocket RFC defines that the underlying connection should be closed
    /// by the server. Tungstenite takes care of this asymmetry for you.
    ///
    /// When the close handshake is finished (we have both sent and received
    /// a close message), `read_message` or `write_pending` will return
    /// [Error::ConnectionClosed] if this endpoint is the server.
    ///
    /// If this endpoint is a client, [Error::ConnectionClosed] will only be
    /// returned after the server has closed the underlying connection.
    ///
    /// It is thus safe to drop the underlying connection as soon as [Error::ConnectionClosed]
    /// is returned from `read_message` or `write_pending`.
    pub fn close(&mut self, code: Option<CloseFrame>) -> Result<()> {
        self.context.close(&mut self.socket, code)
    }
}

/// A context for managing WebSocket stream.
#[derive(Debug)]
pub struct WebSocketContext {
    /// Server or client?
    role: Role,
    /// encoder/decoder of frame.
    frame: FrameCodec,
    /// The state of processing, either "active" or "closing".
    state: WebSocketState,
    /// Receive: an incomplete message being processed.
    incomplete: Option<IncompleteMessage>,
    /// Send: a data send queue.
    send_queue: VecDeque<Frame>,
    /// Send: an OOB pong message.
    pong: Option<Frame>,
    /// The configuration for the websocket session.
    config: WebSocketConfig,
}

impl WebSocketContext {
    /// Create a WebSocket context that manages a post-handshake stream.
    pub fn new(role: Role, config: Option<WebSocketConfig>) -> Self {
        WebSocketContext {
            role,
            frame: FrameCodec::new(),
            state: WebSocketState::Active,
            incomplete: None,
            send_queue: VecDeque::new(),
            pong: None,
            config: config.unwrap_or_else(WebSocketConfig::default),
        }
    }

    /// Create a WebSocket context that manages an post-handshake stream.
    pub fn from_partially_read(part: Vec<u8>, role: Role, config: Option<WebSocketConfig>) -> Self {
        WebSocketContext {
            frame: FrameCodec::from_partially_read(part),
            ..WebSocketContext::new(role, config)
        }
    }

    /// Change the configuration.
    pub fn set_config(&mut self, set_func: impl FnOnce(&mut WebSocketConfig)) {
        set_func(&mut self.config)
    }

    /// Read the configuration.
    pub fn get_config(&self) -> &WebSocketConfig {
        &self.config
    }

    /// Check if it is possible to read messages.
    ///
    /// Reading is impossible after receiving `Message::Close`. It is still possible after
    /// sending close frame since the peer still may send some data before confirming close.
    pub fn can_read(&self) -> bool {
        self.state.can_read()
    }

    /// Check if it is possible to write messages.
    ///
    /// Writing gets impossible immediately after sending or receiving `Message::Close`.
    pub fn can_write(&self) -> bool {
        self.state.is_active()
    }

    /// Read a message from the provided stream, if possible.
    ///
    /// This function sends pong and close responses automatically.
    /// However, it never blocks on write.
    pub fn read_message<Stream>(&mut self, stream: &mut Stream) -> Result<Message>
    where
        Stream: Read + Write,
    {
        // Do not read from already closed connections.
        self.state.check_active()?;

        loop {
            // Since we may get ping or close, we need to reply to the messages even during read.
            // Thus we call write_pending() but ignore its blocking.
            self.write_pending(stream).no_block()?;
            // If we get here, either write blocks or we have nothing to write.
            // Thus if read blocks, just let it return WouldBlock.
            if let Some(message) = self.read_message_frame(stream)? {
                trace!("Received message {}", message);
                return Ok(message);
            }
        }
    }

    /// Send a message to the provided stream, if possible.
    ///
    /// WebSocket will buffer a configurable number of messages at a time, except to reply to Ping
    /// and Close requests. If the WebSocket's send queue is full, `SendQueueFull` will be returned
    /// along with the passed message. Otherwise, the message is queued and Ok(()) is returned.
    ///
    /// Note that only the last pong frame is stored to be sent, and only the
    /// most recent pong frame is sent if multiple pong frames are queued.
    pub fn write_message<Stream>(&mut self, stream: &mut Stream, message: Message) -> Result<()>
    where
        Stream: Read + Write,
    {
        // When terminated, return AlreadyClosed.
        self.state.check_active()?;

        // Do not write after sending a close frame.
        if !self.state.is_active() {
            return Err(Error::Protocol("Sending after closing is not allowed".into()));
        }

        if let Some(max_send_queue) = self.config.max_send_queue {
            if self.send_queue.len() >= max_send_queue {
                // Try to make some room for the new message.
                // Do not return here if write would block, ignore WouldBlock silently
                // since we must queue the message anyway.
                self.write_pending(stream).no_block()?;
            }

            if self.send_queue.len() >= max_send_queue {
                return Err(Error::SendQueueFull(message));
            }
        }

        let frame = match message {
            Message::Text(data) => Frame::message(data.into(), OpCode::Data(OpData::Text), true),
            Message::Binary(data) => Frame::message(data, OpCode::Data(OpData::Binary), true),
            Message::Ping(data) => Frame::ping(data),
            Message::Pong(data) => {
                self.pong = Some(Frame::pong(data));
                return self.write_pending(stream);
            }
            Message::Close(code) => return self.close(stream, code),
        };

        self.send_queue.push_back(frame);
        self.write_pending(stream)
    }

    /// Flush the pending send queue.
    pub fn write_pending<Stream>(&mut self, stream: &mut Stream) -> Result<()>
    where
        Stream: Read + Write,
    {
        // First, make sure we have no pending frame sending.
        self.frame.write_pending(stream)?;

        // Upon receipt of a Ping frame, an endpoint MUST send a Pong frame in
        // response, unless it already received a Close frame. It SHOULD
        // respond with Pong frame as soon as is practical. (RFC 6455)
        if let Some(pong) = self.pong.take() {
            trace!("Sending pong reply");
            self.send_one_frame(stream, pong)?;
        }
        // If we have any unsent frames, send them.
        trace!("Frames still in queue: {}", self.send_queue.len());
        while let Some(data) = self.send_queue.pop_front() {
            self.send_one_frame(stream, data)?;
        }

        // If we get to this point, the send queue is empty and the underlying socket is still
        // willing to take more data.

        // If we're closing and there is nothing to send anymore, we should close the connection.
        if self.role == Role::Server && !self.state.can_read() {
            // The underlying TCP connection, in most normal cases, SHOULD be closed
            // first by the server, so that it holds the TIME_WAIT state and not the
            // client (as this would prevent it from re-opening the connection for 2
            // maximum segment lifetimes (2MSL), while there is no corresponding
            // server impact as a TIME_WAIT connection is immediately reopened upon
            // a new SYN with a higher seq number). (RFC 6455)
            self.state = WebSocketState::Terminated;
            Err(Error::ConnectionClosed)
        } else {
            Ok(())
        }
    }

    /// Close the connection.
    ///
    /// This function guarantees that the close frame will be queued.
    /// There is no need to call it again. Calling this function is
    /// the same as calling `write(Message::Close(..))`.
    pub fn close<Stream>(&mut self, stream: &mut Stream, code: Option<CloseFrame>) -> Result<()>
    where
        Stream: Read + Write,
    {
        if let WebSocketState::Active = self.state {
            self.state = WebSocketState::ClosedByUs;
            let frame = Frame::close(code);
            self.send_queue.push_back(frame);
        } else {
            // Already closed, nothing to do.
        }
        self.write_pending(stream)
    }

    /// Try to decode one message frame. May return None.
    fn read_message_frame<Stream>(&mut self, stream: &mut Stream) -> Result<Option<Message>>
    where
        Stream: Read + Write,
    {
        if let Some(mut frame) = self
            .frame
            .read_frame(stream, self.config.max_frame_size)
            .check_connection_reset(self.state)?
        {
            if !self.state.can_read() {
                return Err(Error::Protocol(
                    "Remote sent frame after having sent a Close Frame".into(),
                ));
            }
            // MUST be 0 unless an extension is negotiated that defines meanings
            // for non-zero values.  If a nonzero value is received and none of
            // the negotiated extensions defines the meaning of such a nonzero
            // value, the receiving endpoint MUST _Fail the WebSocket
            // Connection_.
            {
                let hdr = frame.header();
                if hdr.rsv1 || hdr.rsv2 || hdr.rsv3 {
                    return Err(Error::Protocol("Reserved bits are non-zero".into()));
                }
            }

            match self.role {
                Role::Server => {
                    if frame.is_masked() {
                        // A server MUST remove masking for data frames received from a client
                        // as described in Section 5.3. (RFC 6455)
                        frame.apply_mask()
                    } else if !self.config.accept_unmasked_frames {
                        // The server MUST close the connection upon receiving a
                        // frame that is not masked. (RFC 6455)
                        // The only exception here is if the user explicitly accepts given
                        // stream by setting WebSocketConfig.accept_unmasked_frames to true
                        return Err(Error::Protocol(
                            "Received an unmasked frame from client".into(),
                        ));
                    }
                }
                Role::Client => {
                    if frame.is_masked() {
                        // A client MUST close a connection if it detects a masked frame. (RFC 6455)
                        return Err(Error::Protocol("Received a masked frame from server".into()));
                    }
                }
            }

            match frame.header().opcode {
                OpCode::Control(ctl) => {
                    match ctl {
                        // All control frames MUST have a payload length of 125 bytes or less
                        // and MUST NOT be fragmented. (RFC 6455)
                        _ if !frame.header().is_final => {
                            Err(Error::Protocol("Fragmented control frame".into()))
                        }
                        _ if frame.payload().len() > 125 => {
                            Err(Error::Protocol("Control frame too big".into()))
                        }
                        OpCtl::Close => Ok(self.do_close(frame.into_close()?).map(Message::Close)),
                        OpCtl::Reserved(i) => {
                            Err(Error::Protocol(format!("Unknown control frame type {}", i).into()))
                        }
                        OpCtl::Ping => {
                            let data = frame.into_data();
                            // No ping processing after we sent a close frame.
                            if self.state.is_active() {
                                self.pong = Some(Frame::pong(data.clone()));
                            }
                            Ok(Some(Message::Ping(data)))
                        }
                        OpCtl::Pong => Ok(Some(Message::Pong(frame.into_data()))),
                    }
                }

                OpCode::Data(data) => {
                    let fin = frame.header().is_final;
                    match data {
                        OpData::Continue => {
                            if let Some(ref mut msg) = self.incomplete {
                                msg.extend(frame.into_data(), self.config.max_message_size)?;
                            } else {
                                return Err(Error::Protocol(
                                    "Continue frame but nothing to continue".into(),
                                ));
                            }
                            if fin {
                                Ok(Some(self.incomplete.take().unwrap().complete()?))
                            } else {
                                Ok(None)
                            }
                        }
                        c if self.incomplete.is_some() => Err(Error::Protocol(
                            format!("Received {} while waiting for more fragments", c).into(),
                        )),
                        OpData::Text | OpData::Binary => {
                            let msg = {
                                let message_type = match data {
                                    OpData::Text => IncompleteMessageType::Text,
                                    OpData::Binary => IncompleteMessageType::Binary,
                                    _ => panic!("Bug: message is not text nor binary"),
                                };
                                let mut m = IncompleteMessage::new(message_type);
                                m.extend(frame.into_data(), self.config.max_message_size)?;
                                m
                            };
                            if fin {
                                Ok(Some(msg.complete()?))
                            } else {
                                self.incomplete = Some(msg);
                                Ok(None)
                            }
                        }
                        OpData::Reserved(i) => {
                            Err(Error::Protocol(format!("Unknown data frame type {}", i).into()))
                        }
                    }
                }
            } // match opcode
        } else {
            // Connection closed by peer
            match replace(&mut self.state, WebSocketState::Terminated) {
                WebSocketState::ClosedByPeer | WebSocketState::CloseAcknowledged => {
                    Err(Error::ConnectionClosed)
                }
                _ => Err(Error::Protocol("Connection reset without closing handshake".into())),
            }
        }
    }

    /// Received a close frame. Tells if we need to return a close frame to the user.
    #[allow(clippy::option_option)]
    fn do_close<'t>(&mut self, close: Option<CloseFrame<'t>>) -> Option<Option<CloseFrame<'t>>> {
        debug!("Received close frame: {:?}", close);
        match self.state {
            WebSocketState::Active => {
                let close_code = close.as_ref().map(|f| f.code);
                self.state = WebSocketState::ClosedByPeer;
                let reply = if let Some(code) = close_code {
                    if code.is_allowed() {
                        Frame::close(Some(CloseFrame {
                            code: CloseCode::Normal,
                            reason: "".into(),
                        }))
                    } else {
                        Frame::close(Some(CloseFrame {
                            code: CloseCode::Protocol,
                            reason: "Protocol violation".into(),
                        }))
                    }
                } else {
                    Frame::close(None)
                };
                debug!("Replying to close with {:?}", reply);
                self.send_queue.push_back(reply);

                Some(close)
            }
            WebSocketState::ClosedByPeer | WebSocketState::CloseAcknowledged => {
                // It is already closed, just ignore.
                None
            }
            WebSocketState::ClosedByUs => {
                // We received a reply.
                self.state = WebSocketState::CloseAcknowledged;
                Some(close)
            }
            WebSocketState::Terminated => unreachable!(),
        }
    }

    /// Send a single pending frame.
    fn send_one_frame<Stream>(&mut self, stream: &mut Stream, mut frame: Frame) -> Result<()>
    where
        Stream: Read + Write,
    {
        match self.role {
            Role::Server => {}
            Role::Client => {
                // 5.  If the data is being sent by the client, the frame(s) MUST be
                // masked as defined in Section 5.3. (RFC 6455)
                frame.set_random_mask();
            }
        }

        trace!("Sending frame: {:?}", frame);
        self.frame.write_frame(stream, frame).check_connection_reset(self.state)
    }
}

/// The current connection state.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum WebSocketState {
    /// The connection is active.
    Active,
    /// We initiated a close handshake.
    ClosedByUs,
    /// The peer initiated a close handshake.
    ClosedByPeer,
    /// The peer replied to our close handshake.
    CloseAcknowledged,
    /// The connection does not exist anymore.
    Terminated,
}

impl WebSocketState {
    /// Tell if we're allowed to process normal messages.
    fn is_active(self) -> bool {
        matches!(self, WebSocketState::Active)
    }

    /// Tell if we should process incoming data. Note that if we send a close frame
    /// but the remote hasn't confirmed, they might have sent data before they receive our
    /// close frame, so we should still pass those to client code, hence ClosedByUs is valid.
    fn can_read(self) -> bool {
        matches!(self, WebSocketState::Active | WebSocketState::ClosedByUs)
    }

    /// Check if the state is active, return error if not.
    fn check_active(self) -> Result<()> {
        match self {
            WebSocketState::Terminated => Err(Error::AlreadyClosed),
            _ => Ok(()),
        }
    }
}

/// Translate "Connection reset by peer" into `ConnectionClosed` if appropriate.
trait CheckConnectionReset {
    fn check_connection_reset(self, state: WebSocketState) -> Self;
}

impl<T> CheckConnectionReset for Result<T> {
    fn check_connection_reset(self, state: WebSocketState) -> Self {
        match self {
            Err(Error::Io(io_error)) => Err({
                if !state.can_read() && io_error.kind() == IoErrorKind::ConnectionReset {
                    Error::ConnectionClosed
                } else {
                    Error::Io(io_error)
                }
            }),
            x => x,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Message, Role, WebSocket, WebSocketConfig};

    use std::{io, io::Cursor};

    struct WriteMoc<Stream>(Stream);

    impl<Stream> io::Write for WriteMoc<Stream> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<Stream: io::Read> io::Read for WriteMoc<Stream> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    #[test]
    fn receive_messages() {
        let incoming = Cursor::new(vec![
            0x89, 0x02, 0x01, 0x02, 0x8a, 0x01, 0x03, 0x01, 0x07, 0x48, 0x65, 0x6c, 0x6c, 0x6f,
            0x2c, 0x20, 0x80, 0x06, 0x57, 0x6f, 0x72, 0x6c, 0x64, 0x21, 0x82, 0x03, 0x01, 0x02,
            0x03,
        ]);
        let mut socket = WebSocket::from_raw_socket(WriteMoc(incoming), Role::Client, None);
        assert_eq!(socket.read_message().unwrap(), Message::Ping(vec![1, 2]));
        assert_eq!(socket.read_message().unwrap(), Message::Pong(vec![3]));
        assert_eq!(socket.read_message().unwrap(), Message::Text("Hello, World!".into()));
        assert_eq!(socket.read_message().unwrap(), Message::Binary(vec![0x01, 0x02, 0x03]));
    }

    #[test]
    fn size_limiting_text_fragmented() {
        let incoming = Cursor::new(vec![
            0x01, 0x07, 0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x2c, 0x20, 0x80, 0x06, 0x57, 0x6f, 0x72,
            0x6c, 0x64, 0x21,
        ]);
        let limit = WebSocketConfig { max_message_size: Some(10), ..WebSocketConfig::default() };
        let mut socket = WebSocket::from_raw_socket(WriteMoc(incoming), Role::Client, Some(limit));
        assert_eq!(
            socket.read_message().unwrap_err().to_string(),
            "Space limit exceeded: Message too big: 7 + 6 > 10"
        );
    }

    #[test]
    fn size_limiting_binary() {
        let incoming = Cursor::new(vec![0x82, 0x03, 0x01, 0x02, 0x03]);
        let limit = WebSocketConfig { max_message_size: Some(2), ..WebSocketConfig::default() };
        let mut socket = WebSocket::from_raw_socket(WriteMoc(incoming), Role::Client, Some(limit));
        assert_eq!(
            socket.read_message().unwrap_err().to_string(),
            "Space limit exceeded: Message too big: 0 + 3 > 2"
        );
    }
}
