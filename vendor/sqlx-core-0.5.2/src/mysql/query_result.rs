use std::iter::{Extend, IntoIterator};

#[derive(Debug, Default)]
pub struct MySqlQueryResult {
    pub(super) rows_affected: u64,
    pub(super) last_insert_id: u64,
}

impl MySqlQueryResult {
    pub fn last_insert_id(&self) -> u64 {
        self.last_insert_id
    }

    pub fn rows_affected(&self) -> u64 {
        self.rows_affected
    }
}

impl Extend<MySqlQueryResult> for MySqlQueryResult {
    fn extend<T: IntoIterator<Item = MySqlQueryResult>>(&mut self, iter: T) {
        for elem in iter {
            self.rows_affected += elem.rows_affected;
            self.last_insert_id = elem.last_insert_id;
        }
    }
}

#[cfg(feature = "any")]
impl From<MySqlQueryResult> for crate::any::AnyQueryResult {
    fn from(done: MySqlQueryResult) -> Self {
        crate::any::AnyQueryResult {
            rows_affected: done.rows_affected,
            last_insert_id: Some(done.last_insert_id as i64),
        }
    }
}
