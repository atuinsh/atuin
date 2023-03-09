// Calendar data

use serde::{Deserialize, Serialize};

pub enum TimePeriod {
    Year,
    Month,
    Day,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TimePeriodInfo {
    pub count: u64,

    // TODO: Use this for merkle tree magic
    pub hash: String,
}
