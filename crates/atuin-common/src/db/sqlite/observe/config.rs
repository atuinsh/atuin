use std::num::NonZeroU32;
use std::time::Duration;

use strum_macros::Display;
use typed_builder::TypedBuilder;

use crate::futures::Backoff;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Display)]
pub enum Replay {
    #[default]
    FromNow,
    All,
}

#[derive(Debug, Clone, Copy, TypedBuilder)]
pub struct ObserveConfig {
    #[builder(default)]
    pub replay: Replay,
    #[builder(default = Duration::from_millis(250))]
    pub poll_interval: Duration,
    #[builder(default = Backoff::Exponential {
        initial: Duration::from_millis(50),
        max: Duration::from_secs(5),
        factor: NonZeroU32::new(2).unwrap(),
    })]
    pub reconnect: Backoff,
}
