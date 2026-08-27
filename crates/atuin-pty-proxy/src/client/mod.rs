use crate::domain::ipc::{
    AnyRequest, AnyResponse, DumpScreenRep, DumpScreenReq, GoodbyeRep, GoodbyeReq, HelloRep,
    HelloReq,
};

mod ipc;

pub use ipc::{IpcClient, IpcConnectError, IpcConnection, IpcError};

pub trait Request: Into<AnyRequest> {
    type Rep: TryFrom<AnyResponse>;
}

impl Request for HelloReq {
    type Rep = HelloRep;
}

impl Request for DumpScreenReq {
    type Rep = DumpScreenRep;
}

impl Request for GoodbyeReq {
    type Rep = GoodbyeRep;
}
