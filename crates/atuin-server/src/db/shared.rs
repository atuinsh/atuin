use atuin_domain::record::{EncryptedData, Record, RecordStatus};

use super::wrappers::RecordSeriesPoint;
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
pub(super) fn build_status(points: impl IntoIterator<Item = RecordSeriesPoint>) -> RecordStatus {
    let mut status = RecordStatus::new();
    for point in points {
        status.set_raw(point.series, point.idx);
    }
    status
}
