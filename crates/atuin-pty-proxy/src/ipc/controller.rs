use std::sync::mpsc::{self, SendError, SyncSender};

use crate::{
    ipc::domain::*,
    screen::{Msg, ScreenSnapshot},
};

/// This trait must be implemented by a controller which services each of these messages.
#[derive(Debug, Clone, Copy)]
pub struct IpcController {
    /// TODO(markovejnovic): World's biggest debt.
    screen_tx: SyncSender<Msg>,
}

impl IpcController {
    pub fn new(screen_tx: SyncSender<Msg>) -> Self {
        Self { screen_tx }
    }

    pub fn hello(&mut self, _req: HelloReq) -> HelloRep {
        HelloRep {}
    }

    pub fn dump_screen(&mut self, req: DumpScreenReq) -> DumpScreenRep {
        self.get_screen();
    }

    pub fn goodbye(&mut self, _req: GoodbyeReq) -> GoodbyeRep {
        GoodbyeRep {}
    }

    fn get_screen(&self) -> Result<ScreenSnapshot, SendError<Msg>> {
        let (reply_tx, reply_rx) = mpsc::channel();

        self.screen_tx.send(Msg::ScreenRequest(reply_tx))?;

        Ok(reply_rx.recv())
    }
}
