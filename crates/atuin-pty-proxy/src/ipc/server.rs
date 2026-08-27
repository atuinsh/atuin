use std::{
    io::{ErrorKind, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    thread::JoinHandle,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, warn};

use crate::{ipc::controller::IpcController, screen::ScreenSnapshot};

pub struct IpcServer {
    join_handle: JoinHandle<Result<(), IpcServerError>>,
}

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

        let handle = std::thread::spawn(move || IpcServerWorker::new(ctrl).work(listener));

        Ok(Self {
            join_handle: handle,
        })
    }

    pub fn join(self) -> Result<(), IpcServerError> {
        match self.join_handle.join() {
            Ok(result) => result,
            Err(_) => Ok(()),
        }
    }
}

struct IpcServerWorker {
    scratch_buf: Vec<u8>,
    controller: IpcController,
}

impl IpcServerWorker<C> {
    /// Clients after this timeout are considered corrupt and we no longer talk to them.
    const HEARTBEAT_TIMEOUT: Duration = Duration::from_millis(200);
    const READ_TIMEOUT: Duration = Duration::from_millis(100);
    const WRITE_TIMEOUT: Duration = Duration::from_millis(100);
    const MAX_MSG_LEN: u32 = 128 * 1024 * 1024;

    fn new(ctrl: C) -> Self {
        debug_assert!(Self::WRITE_TIMEOUT + Self::READ_TIMEOUT >= Self::HEARTBEAT_TIMEOUT);

        Self {
            scratch_buf: Vec::new(),
            controller: ctrl,
        }
    }

    fn work(mut self, listener: UnixListener) -> Result<(), IpcServerError> {
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
                Req::Hello(req) => Rep::Hello(self.controller.hello(req)),
                Req::DumpScreen(req) => Rep::DumpScreenRep(self.controller.dump_screen(req)),
                Req::Goodbye(req) => Rep::Goodbye(self.controller.goodbye(req)),
            };

            Self::write_reply(stream, rep)?;

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
        let mut len_bytes = [0u8; 4];
        if let Err(err) = stream.read_exact(&mut len_bytes) {
            return match err.kind() {
                ErrorKind::UnexpectedEof => Ok(None),
                _ => Err(err.into()),
            };
        }

        let len = u32::from_be_bytes(len_bytes);
        if len > Self::MAX_MSG_LEN {
            return Err(StreamServiceError::TooLarge(len as usize));
        }

        let len = len as usize;
        if scratch.len() < len {
            scratch.resize(len, 0);
        }
        let buf = &mut scratch[..len];
        stream.read_exact(buf)?;

        let req = postcard::from_bytes::<Req>(buf).map_err(StreamServiceError::Decode)?;
        Ok(Some(req))
    }

    fn write_reply(stream: &mut UnixStream, rep: Rep) -> Result<(), StreamServiceError> {
        let body = postcard::to_stdvec(&rep).map_err(StreamServiceError::Encode)?;
        let len =
            u32::try_from(body.len()).map_err(|_| StreamServiceError::TooLarge(body.len()))?;

        stream.write_all(&len.to_be_bytes())?;
        stream.write_all(&body)?;
        stream.flush()?;

        Ok(())
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
