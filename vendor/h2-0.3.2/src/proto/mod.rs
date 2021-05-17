mod connection;
mod error;
mod go_away;
mod peer;
mod ping_pong;
mod settings;
mod streams;

pub(crate) use self::connection::{Config, Connection};
pub(crate) use self::error::Error;
pub(crate) use self::peer::{Dyn as DynPeer, Peer};
pub(crate) use self::ping_pong::UserPings;
pub(crate) use self::streams::{DynStreams, OpaqueStreamRef, StreamRef, Streams};
pub(crate) use self::streams::{Open, PollReset, Prioritized};

use crate::codec::Codec;

use self::go_away::GoAway;
use self::ping_pong::PingPong;
use self::settings::Settings;

use crate::frame::{self, Frame};

use bytes::Buf;

use tokio::io::AsyncWrite;

pub type PingPayload = [u8; 8];

pub type WindowSize = u32;

// Constants
pub const MAX_WINDOW_SIZE: WindowSize = (1 << 31) - 1;
pub const DEFAULT_RESET_STREAM_MAX: usize = 10;
pub const DEFAULT_RESET_STREAM_SECS: u64 = 30;
