use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use thiserror::Error;
use tracing::{error, warn};

use crate::ipc::controller::IpcController;
use crate::ipc::domain::{Rep, Req};
use crate::ipc::wire::{self, FrameError};

pub struct IpcServer;

#[derive(Debug, Error)]
pub enum IpcSpawnError {
    #[error("unexpected io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum IpcServerError {
    #[error("critical listener io error: {0}")]
    Listener(std::io::Error),

    #[error("failed to configure a stream: {0}")]
    StreamControl(std::io::Error),
}

#[derive(Debug, Error)]
enum StreamServiceError {
    #[error("connection with peer reset: {0}")]
    ConnectionReset(std::io::Error),

    #[error("peer exceeded the heartbeat timeout: {0}")]
    Timeout(std::io::Error),

    #[error("peer sent an oversized message: {0} bytes")]
    TooLarge(usize),

    #[error("failed to decode message from peer: {0}")]
    Decode(postcard::Error),

    #[error("failed to encode reply: {0}")]
    Encode(postcard::Error),

    #[error("unexpected io error: {0}")]
    Io(std::io::Error),
}

impl IpcServer {
    /// Spawn the IPC server on a separate thread.
    pub fn spawn(sock_path: &Path, ctrl: IpcController) -> Result<Self, IpcSpawnError> {
        let listener = UnixListener::bind(sock_path)?;

        std::thread::spawn(move || IpcServerWorker::new(ctrl).work(&listener));

        Ok(Self)
    }
}

struct IpcServerWorker {
    scratch_buf: Vec<u8>,
    controller: IpcController,
}

impl IpcServerWorker {
    /// Clients after this timeout are considered corrupt and we no longer talk to them.
    const HEARTBEAT_TIMEOUT: Duration = Duration::from_millis(200);
    const READ_TIMEOUT: Duration = Duration::from_millis(100);
    const WRITE_TIMEOUT: Duration = Duration::from_millis(100);

    fn new(ctrl: IpcController) -> Self {
        debug_assert!(Self::WRITE_TIMEOUT + Self::READ_TIMEOUT >= Self::HEARTBEAT_TIMEOUT);

        Self {
            scratch_buf: Vec::new(),
            controller: ctrl,
        }
    }

    fn work(mut self, listener: &UnixListener) -> Result<(), IpcServerError> {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(err) => match err.kind() {
                    ErrorKind::ConnectionAborted | ErrorKind::Interrupted => {
                        error!(%err, "connection with peer dropped.");
                        continue;
                    }
                    _ => {
                        error!(%err, "critical error. cannot proceed. aborting pty-proxy...");
                        return Err(IpcServerError::Listener(err));
                    }
                },
            };

            if let Err(err) = stream.set_read_timeout(Some(Self::READ_TIMEOUT)) {
                return Err(IpcServerError::StreamControl(err));
            }

            if let Err(err) = stream.set_write_timeout(Some(Self::WRITE_TIMEOUT)) {
                return Err(IpcServerError::StreamControl(err));
            }

            if let Err(err) = self.service_stream(&mut stream) {
                warn!(%err, "dropping ipc client connection");
            }
        }

        Ok(())
    }

    fn service_stream(&mut self, stream: &mut UnixStream) -> Result<(), StreamServiceError> {
        // The following protocol is very naive, but might just be good enough.
        //
        // The client needs to respond to us within 200ms. If the client does not respond within
        // 200ms, we consider this client to be detached (likely due to the client's bugs), so
        // we stop our connection, in order to be able to service others.
        //
        // Really, pty-proxy only ever talks to one client -- the daemon, and the case of
        // needing to serve another client is if the daemon restarts. The daemon may, indeed,
        // crash and restart on a stream we are just servicing, so this logic is correct and
        // good to have.
        loop {
            let Some(req) = Self::read_request(stream, &mut self.scratch_buf)? else {
                return Ok(());
            };

            let rep = match req {
                Req::Hello(req) => Rep::Hello(IpcController::hello(req)),
                Req::DumpScreen(req) => Rep::DumpScreenRep(self.controller.dump_screen(req)),
                Req::Goodbye(req) => Rep::Goodbye(IpcController::goodbye(req)),
            };

            Self::write_reply(stream, &rep)?;

            if matches!(req, Req::Goodbye(_)) {
                return Ok(());
            }
        }
    }

    /// Reads a request fro the server and parses it into a [`Req`] structure.
    ///
    /// Returns [`None`] if the client aborted the connection normally.
    fn read_request(
        stream: &mut UnixStream,
        scratch: &mut Vec<u8>,
    ) -> Result<Option<Req>, StreamServiceError> {
        let mut len_bytes = [0u8; wire::LEN_PREFIX_BYTES];
        if let Err(err) = stream.read_exact(&mut len_bytes) {
            return match err.kind() {
                ErrorKind::UnexpectedEof => Ok(None),
                _ => Err(err.into()),
            };
        }

        let len = wire::parse_len(len_bytes)?;
        if scratch.len() < len {
            scratch.resize(len, 0);
        }
        let buf = &mut scratch[..len];
        stream.read_exact(buf)?;

        let req = wire::decode_body::<Req>(buf)?;
        Ok(Some(req))
    }

    fn write_reply(stream: &mut UnixStream, rep: &Rep) -> Result<(), StreamServiceError> {
        let framed = wire::encode_frame(rep)?;
        stream.write_all(&framed)?;
        stream.flush()?;

        Ok(())
    }
}

impl From<FrameError> for StreamServiceError {
    fn from(err: FrameError) -> Self {
        match err {
            FrameError::TooLarge(len) => Self::TooLarge(len),
            FrameError::Encode(err) => Self::Encode(err),
            FrameError::Decode(err) => Self::Decode(err),
        }
    }
}

impl From<std::io::Error> for StreamServiceError {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            ErrorKind::ConnectionReset => Self::ConnectionReset(err),
            ErrorKind::WouldBlock | ErrorKind::TimedOut => Self::Timeout(err),
            _ => Self::Io(err),
        }
    }
}
