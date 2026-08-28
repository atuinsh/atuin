use atuin_domain::record::{
    EncryptedData, HostId, Record, RecordSeriesKey, RecordStatus, RecordTag,
};

use super::{DbError, DbResult};

/// Apply the `next_records` contract: a `NotFound` becomes an empty result, any
/// other error propagates.
pub(super) fn shape_next_records(
    records: DbResult<Vec<Record<EncryptedData>>>,
) -> DbResult<Vec<Record<EncryptedData>>> {
    match records {
        Ok(records) => Ok(records),
        Err(DbError::NotFound) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Assemble a [`RecordStatus`] from `(host, tag, max_idx)` rows.
pub(super) fn build_status(rows: impl IntoIterator<Item = (HostId, String, i64)>) -> RecordStatus {
    let mut status = RecordStatus::new();
    for (host, tag, idx) in rows {
        status.set_raw(RecordSeriesKey::new(host, RecordTag::from(tag)), idx as u64);
    }
    status
}
