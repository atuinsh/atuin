use std::error::Error;
use std::fmt::{self, Debug, Display, Formatter};

use crate::error::DatabaseError;
use crate::mysql::protocol::response::ErrPacket;
use smallvec::alloc::borrow::Cow;

/// An error returned from the MySQL database.
pub struct MySqlDatabaseError(pub(super) ErrPacket);

impl MySqlDatabaseError {
    /// The [SQLSTATE](https://dev.mysql.com/doc/refman/8.0/en/server-error-reference.html) code for this error.
    pub fn code(&self) -> Option<&str> {
        self.0.sql_state.as_deref()
    }

    /// The [number](https://dev.mysql.com/doc/refman/8.0/en/server-error-reference.html)
    /// for this error.
    ///
    /// MySQL tends to use SQLSTATE as a general error category, and the error number as a more
    /// granular indication of the error.
    pub fn number(&self) -> u16 {
        self.0.error_code
    }

    /// The human-readable error message.
    pub fn message(&self) -> &str {
        &self.0.error_message
    }
}

impl Debug for MySqlDatabaseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("MySqlDatabaseError")
            .field("code", &self.code())
            .field("number", &self.number())
            .field("message", &self.message())
            .finish()
    }
}

impl Display for MySqlDatabaseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(code) = &self.code() {
            write!(f, "{} ({}): {}", self.number(), code, self.message())
        } else {
            write!(f, "{}: {}", self.number(), self.message())
        }
    }
}

impl Error for MySqlDatabaseError {}

impl DatabaseError for MySqlDatabaseError {
    #[inline]
    fn message(&self) -> &str {
        self.message()
    }

    #[inline]
    fn code(&self) -> Option<Cow<'_, str>> {
        self.code().map(Cow::Borrowed)
    }

    #[doc(hidden)]
    fn as_error(&self) -> &(dyn Error + Send + Sync + 'static) {
        self
    }

    #[doc(hidden)]
    fn as_error_mut(&mut self) -> &mut (dyn Error + Send + Sync + 'static) {
        self
    }

    #[doc(hidden)]
    fn into_error(self: Box<Self>) -> Box<dyn Error + Send + Sync + 'static> {
        self
    }
}
