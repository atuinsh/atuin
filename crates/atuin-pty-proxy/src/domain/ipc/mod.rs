pub mod wire;

use derive_more::{From, TryInto};
use serde::{Deserialize, Serialize};

use super::screen::ScreenSnapshot;

pub const PROTOCOL_VERSION: u32 = 1;

/// Associates a request type with the reply type the server sends back.

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HelloRequest {
    pub version: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResponse {
    pub version: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DumpScreenRequest;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpScreenResponse {
    pub screen: ScreenSnapshot,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GoodbyeRequest;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoodbyeResponse;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, From)]
pub enum AnyRequest {
    Hello(HelloRequest),
    DumpScreen(DumpScreenRequest),
    Goodbye(GoodbyeRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize, From, TryInto)]
pub enum AnyResponse {
    Hello(HelloResponse),
    DumpScreenResponse(DumpScreenResponse),
    Goodbye(GoodbyeResponse),
}
