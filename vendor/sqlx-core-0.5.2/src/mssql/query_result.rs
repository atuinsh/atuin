use std::iter::{Extend, IntoIterator};

#[derive(Debug, Default)]
pub struct MssqlQueryResult {
    pub(super) rows_affected: u64,
}

impl MssqlQueryResult {
    pub fn rows_affected(&self) -> u64 {
        self.rows_affected
    }
}

impl Extend<MssqlQueryResult> for MssqlQueryResult {
    fn extend<T: IntoIterator<Item = MssqlQueryResult>>(&mut self, iter: T) {
        for elem in iter {
            self.rows_affected += elem.rows_affected;
        }
    }
}

#[cfg(feature = "any")]
impl From<MssqlQueryResult> for crate::any::AnyQueryResult {
    fn from(done: MssqlQueryResult) -> Self {
        crate::any::AnyQueryResult {
            rows_affected: done.rows_affected,
            last_insert_id: None,
        }
    }
}
