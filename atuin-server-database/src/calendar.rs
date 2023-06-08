// Calendar data

use serde::{Deserialize, Serialize};

pub enum TimePeriod {
    YEAR,
    MONTH,
    DAY,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TimePeriodInfo {
    pub count: u64,

    // TODO: Use this for merkle tree magic
    pub hash: String,
}
