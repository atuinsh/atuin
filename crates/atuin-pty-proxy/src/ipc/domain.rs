use derive_more::{From, TryInto};
use serde::{Deserialize, Serialize};

use crate::screen::ScreenSnapshot;

pub const PROTOCOL_VERSION: u32 = 1;

/// Associates a request type with the reply type the server sends back.
#[cfg(feature = "client")]
pub trait IsRequest: Into<Req> {
    type Rep: TryFrom<Rep>;
}

#[cfg(feature = "client")]
impl IsRequest for HelloReq {
    type Rep = HelloRep;
}

#[cfg(feature = "client")]
impl IsRequest for DumpScreenReq {
    type Rep = DumpScreenRep;
}

#[cfg(feature = "client")]
impl IsRequest for GoodbyeReq {
    type Rep = GoodbyeRep;
}

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
pub enum Req {
    Hello(HelloReq),
    DumpScreen(DumpScreenReq),
    Goodbye(GoodbyeReq),
}

#[derive(Debug, Clone, Serialize, Deserialize, From, TryInto)]
pub enum Rep {
    Hello(HelloRep),
    DumpScreenRep(DumpScreenRep),
    Goodbye(GoodbyeRep),
}
