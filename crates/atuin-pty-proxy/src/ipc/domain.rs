use serde::{Deserialize, Serialize};

use crate::screen::ScreenSnapshot;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HelloReq {
    pub version: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HelloRep {
    pub version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DumpScreenReq;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpScreenRep {
    pub screen: ScreenSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GoodbyeReq;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GoodbyeRep;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Req {
    Hello(HelloReq),
    DumpScreen(DumpScreenReq),
    Goodbye(GoodbyeReq),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Rep {
    Hello(HelloRep),
    DumpScreenRep(DumpScreenRep),
    Goodbye(GoodbyeRep),
}
