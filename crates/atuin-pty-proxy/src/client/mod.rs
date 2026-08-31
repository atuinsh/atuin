use crate::domain::ipc::{
    AnyRequest, AnyResponse, DumpScreenRequest, DumpScreenResponse, GoodbyeRequest,
    GoodbyeResponse, HelloRequest, HelloResponse,
};

mod ipc;

pub use ipc::{IpcClient, IpcConnectError, IpcConnection, IpcError};

pub trait Request: Into<AnyRequest> {
    type Response: TryFrom<AnyResponse>;
}

impl Request for HelloRequest {
    type Response = HelloResponse;
}

impl Request for DumpScreenRequest {
    type Response = DumpScreenResponse;
}

impl Request for GoodbyeRequest {
    type Response = GoodbyeResponse;
}
