use std::sync::mpsc::{self, SyncSender};

use crate::ipc::domain::*;
use crate::screen::{Msg, ScreenSnapshot};

/// This trait must be implemented by a controller which services each of these messages.
#[derive(Debug, Clone)]
pub struct IpcController {
    /// TODO(markovejnovic): World's biggest debt.
    screen_tx: SyncSender<Msg>,
}

impl IpcController {
    pub fn new(screen_tx: SyncSender<Msg>) -> Self {
        Self { screen_tx }
    }

    pub fn hello(_req: HelloReq) -> HelloRep {
        HelloRep {
            version: PROTOCOL_VERSION,
        }
    }

    pub fn dump_screen(&self, _req: DumpScreenReq) -> DumpScreenRep {
        DumpScreenRep {
            screen: self.get_screen(),
        }
    }

    pub fn goodbye(_req: GoodbyeReq) -> GoodbyeRep {
        GoodbyeRep {}
    }

    fn get_screen(&self) -> ScreenSnapshot {
        let (reply_tx, reply_rx) = mpsc::channel();

        if self.screen_tx.send(Msg::ScreenRequest(reply_tx)).is_err() {
            return ScreenSnapshot::default();
        }

        reply_rx.recv().unwrap_or_default()
    }
}
