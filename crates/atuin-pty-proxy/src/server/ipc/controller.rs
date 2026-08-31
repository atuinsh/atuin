//! The main controller responsible for servicing the [`crate::ipc::IpcServer`].
//!
//! A controller is a piece of code which receives typed request messages, does something with them,
//! and then returns typed response messages.
//!
//! This controller is injected into [`crate::ipc::IpcServer::spawn`] and is responsible for
//! servicing client requests.
use std::sync::mpsc::{self, SyncSender};

use crate::domain::ipc::{
    DumpScreenRequest, DumpScreenResponse, GoodbyeRequest, GoodbyeResponse, HelloRequest,
    HelloResponse, PROTOCOL_VERSION,
};
use crate::domain::screen::ScreenSnapshot;
use crate::server::screen::Msg;

#[derive(Debug, Clone)]
pub struct IpcController {
    /// A channel to the `screen.rs` thread to request terminal data from it.
    ///
    /// Historically we had this multi-threaded architecture -- `screen.rs` spawns a thread which
    /// collects data from the screen, parses it, understands osc133 codes, etc.
    ///
    /// We "request" the frame from that thread by sending a message over this channel, telling it
    /// "hey i want data -- here's a channel you can write to".
    ///
    /// This is all so very allocation heavy, obviously, but it is the pattern as it was before.
    screen_tx: SyncSender<Msg>,
}

impl IpcController {
    pub fn new(screen_tx: SyncSender<Msg>) -> Self {
        Self { screen_tx }
    }

    pub fn hello(&self, _req: HelloRequest) -> HelloResponse {
        let _ = self;

        HelloResponse {
            version: PROTOCOL_VERSION,
        }
    }

    pub fn dump_screen(&self, _req: DumpScreenRequest) -> DumpScreenResponse {
        DumpScreenResponse {
            screen: self.get_screen(),
        }
    }

    pub fn goodbye(&self, _req: GoodbyeRequest) -> GoodbyeResponse {
        let _ = self;

        GoodbyeResponse {}
    }

    fn get_screen(&self) -> ScreenSnapshot {
        let (reply_tx, reply_rx) = mpsc::channel();

        if self.screen_tx.send(Msg::ScreenRequest(reply_tx)).is_err() {
            return ScreenSnapshot::default();
        }

        reply_rx.recv().unwrap_or_default()
    }
}
