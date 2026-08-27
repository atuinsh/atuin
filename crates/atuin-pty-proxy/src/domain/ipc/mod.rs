pub mod wire;

use derive_more::{From, TryInto};
use serde::{Deserialize, Serialize};

use super::screen::ScreenSnapshot;

pub const PROTOCOL_VERSION: u32 = 1;

/// Associates a request type with the reply type the server sends back.

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HelloReq {
    pub version: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloRep {
    pub version: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DumpScreenReq;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpScreenRep {
    pub screen: ScreenSnapshot,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GoodbyeReq;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoodbyeRep;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, From)]
pub enum AnyRequest {
    Hello(HelloReq),
    DumpScreen(DumpScreenReq),
    Goodbye(GoodbyeReq),
}

#[derive(Debug, Clone, Serialize, Deserialize, From, TryInto)]
pub enum AnyResponse {
    Hello(HelloRep),
    DumpScreenRep(DumpScreenRep),
    Goodbye(GoodbyeRep),
}
