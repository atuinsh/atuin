#![allow(non_snake_case, non_camel_case_types)]

pub use self::error::*;

use std::default::Default;
use std::mem;

mod error;

pub fn SQLITE_STATIC() -> sqlite3_destructor_type {
    None
}

pub fn SQLITE_TRANSIENT() -> sqlite3_destructor_type {
    Some(unsafe { mem::transmute(-1isize) })
}

/// Run-Time Limit Categories
#[repr(i32)]
#[non_exhaustive]
#[allow(clippy::upper_case_acronyms)]
pub enum Limit {
    /// The maximum size of any string or BLOB or table row, in bytes.
    SQLITE_LIMIT_LENGTH = SQLITE_LIMIT_LENGTH,
    /// The maximum length of an SQL statement, in bytes.
    SQLITE_LIMIT_SQL_LENGTH = SQLITE_LIMIT_SQL_LENGTH,
    /// The maximum number of columns in a table definition or in the result set
    /// of a SELECT or the maximum number of columns in an index or in an
    /// ORDER BY or GROUP BY clause.
    SQLITE_LIMIT_COLUMN = SQLITE_LIMIT_COLUMN,
    /// The maximum depth of the parse tree on any expression.
    SQLITE_LIMIT_EXPR_DEPTH = SQLITE_LIMIT_EXPR_DEPTH,
    /// The maximum number of terms in a compound SELECT statement.
    SQLITE_LIMIT_COMPOUND_SELECT = SQLITE_LIMIT_COMPOUND_SELECT,
    /// The maximum number of instructions in a virtual machine program used to
    /// implement an SQL statement.
    SQLITE_LIMIT_VDBE_OP = SQLITE_LIMIT_VDBE_OP,
    /// The maximum number of arguments on a function.
    SQLITE_LIMIT_FUNCTION_ARG = SQLITE_LIMIT_FUNCTION_ARG,
    /// The maximum number of attached databases.
    SQLITE_LIMIT_ATTACHED = SQLITE_LIMIT_ATTACHED,
    /// The maximum length of the pattern argument to the LIKE or GLOB
    /// operators.
    SQLITE_LIMIT_LIKE_PATTERN_LENGTH = SQLITE_LIMIT_LIKE_PATTERN_LENGTH,
    /// The maximum index number of any parameter in an SQL statement.
    SQLITE_LIMIT_VARIABLE_NUMBER = SQLITE_LIMIT_VARIABLE_NUMBER,
    /// The maximum depth of recursion for triggers.
    SQLITE_LIMIT_TRIGGER_DEPTH = 10,
    /// The maximum number of auxiliary worker threads that a single prepared
    /// statement may start.
    SQLITE_LIMIT_WORKER_THREADS = 11,
}

#[allow(clippy::all)]
mod bindings {
    include!(concat!(env!("OUT_DIR"), "/bindgen.rs"));
}
pub use bindings::*;

pub type sqlite3_index_constraint = sqlite3_index_info_sqlite3_index_constraint;
pub type sqlite3_index_constraint_usage = sqlite3_index_info_sqlite3_index_constraint_usage;

impl Default for sqlite3_vtab {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl Default for sqlite3_vtab_cursor {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}
