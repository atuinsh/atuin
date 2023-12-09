// Calendar data

use serde::{Deserialize, Serialize};
use time::Month;

pub enum TimePeriod {
    Year,
    Month { year: i32 },
    Day { year: i32, month: Month },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TimePeriodInfo {
    pub count: u64,

    // TODO: Use this for merkle tree magic
    pub hash: String,
}
