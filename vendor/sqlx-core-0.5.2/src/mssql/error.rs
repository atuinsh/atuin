use std::error::Error as StdError;
use std::fmt::{self, Debug, Display, Formatter};

use crate::error::DatabaseError;
use crate::mssql::protocol::error::Error;

/// An error returned from the MSSQL database.
pub struct MssqlDatabaseError(pub(crate) Error);

impl Debug for MssqlDatabaseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("MssqlDatabaseError")
            .field("message", &self.0.message)
            .field("number", &self.0.number)
            .field("state", &self.0.state)
            .field("class", &self.0.class)
            .field("server", &self.0.server)
            .field("procedure", &self.0.procedure)
            .field("line", &self.0.line)
            .finish()
    }
}

impl Display for MssqlDatabaseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(self.message())
    }
}

impl StdError for MssqlDatabaseError {}

impl DatabaseError for MssqlDatabaseError {
    #[inline]
    fn message(&self) -> &str {
        &self.0.message
    }

    #[doc(hidden)]
    fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
        self
    }

    #[doc(hidden)]
    fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
        self
    }

    #[doc(hidden)]
    fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
        self
    }
}
