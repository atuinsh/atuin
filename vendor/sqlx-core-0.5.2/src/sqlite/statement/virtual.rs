#![allow(clippy::rc_buffer)]

use crate::error::Error;
use crate::ext::ustr::UStr;
use crate::sqlite::connection::ConnectionHandle;
use crate::sqlite::statement::StatementHandle;
use crate::sqlite::{SqliteColumn, SqliteError, SqliteRow, SqliteValue};
use crate::HashMap;
use bytes::{Buf, Bytes};
use libsqlite3_sys::{
    sqlite3, sqlite3_clear_bindings, sqlite3_finalize, sqlite3_prepare_v3, sqlite3_reset,
    sqlite3_stmt, SQLITE_MISUSE, SQLITE_OK, SQLITE_PREPARE_PERSISTENT,
};
use smallvec::SmallVec;
use std::i32;
use std::os::raw::c_char;
use std::ptr::{null, null_mut, NonNull};
use std::sync::{atomic::AtomicPtr, Arc, Weak};

// A virtual statement consists of *zero* or more raw SQLite3 statements. We chop up a SQL statement
// on `;` to support multiple statements in one query.

#[derive(Debug)]
pub(crate) struct VirtualStatement {
    persistent: bool,
    index: usize,

    // tail of the most recently prepared SQL statement within this container
    tail: Bytes,

    // underlying sqlite handles for each inner statement
    // a SQL query string in SQLite is broken up into N statements
    // we use a [`SmallVec`] to optimize for the most likely case of a single statement
    pub(crate) handles: SmallVec<[StatementHandle; 1]>,

    // each set of columns
    pub(crate) columns: SmallVec<[Arc<Vec<SqliteColumn>>; 1]>,

    // each set of column names
    pub(crate) column_names: SmallVec<[Arc<HashMap<UStr, usize>>; 1]>,

    // weak reference to the previous row from this connection
    // we use the notice of a successful upgrade of this reference as an indicator that the
    // row is still around, in which we then inflate the row such that we can let SQLite
    // clobber the memory allocation for the row
    pub(crate) last_row_values: SmallVec<[Option<Weak<AtomicPtr<SqliteValue>>>; 1]>,
}

fn prepare(
    conn: *mut sqlite3,
    query: &mut Bytes,
    persistent: bool,
) -> Result<Option<StatementHandle>, Error> {
    let mut flags = 0;

    if persistent {
        // SQLITE_PREPARE_PERSISTENT
        //  The SQLITE_PREPARE_PERSISTENT flag is a hint to the query
        //  planner that the prepared statement will be retained for a long time
        //  and probably reused many times.
        flags |= SQLITE_PREPARE_PERSISTENT;
    }

    while !query.is_empty() {
        let mut statement_handle: *mut sqlite3_stmt = null_mut();
        let mut tail: *const c_char = null();

        let query_ptr = query.as_ptr() as *const c_char;
        let query_len = query.len() as i32;

        // <https://www.sqlite.org/c3ref/prepare.html>
        let status = unsafe {
            sqlite3_prepare_v3(
                conn,
                query_ptr,
                query_len,
                flags as u32,
                &mut statement_handle,
                &mut tail,
            )
        };

        if status != SQLITE_OK {
            return Err(SqliteError::new(conn).into());
        }

        // tail should point to the first byte past the end of the first SQL
        // statement in zSql. these routines only compile the first statement,
        // so tail is left pointing to what remains un-compiled.

        let n = (tail as usize) - (query_ptr as usize);
        query.advance(n);

        if let Some(handle) = NonNull::new(statement_handle) {
            return Ok(Some(StatementHandle(handle)));
        }
    }

    Ok(None)
}

impl VirtualStatement {
    pub(crate) fn new(mut query: &str, persistent: bool) -> Result<Self, Error> {
        query = query.trim();

        if query.len() > i32::MAX as usize {
            return Err(err_protocol!(
                "query string must be smaller than {} bytes",
                i32::MAX
            ));
        }

        Ok(Self {
            persistent,
            tail: Bytes::from(String::from(query)),
            handles: SmallVec::with_capacity(1),
            index: 0,
            columns: SmallVec::with_capacity(1),
            column_names: SmallVec::with_capacity(1),
            last_row_values: SmallVec::with_capacity(1),
        })
    }

    pub(crate) fn prepare(
        &mut self,
        conn: &mut ConnectionHandle,
    ) -> Result<
        Option<(
            &StatementHandle,
            &mut Arc<Vec<SqliteColumn>>,
            &Arc<HashMap<UStr, usize>>,
            &mut Option<Weak<AtomicPtr<SqliteValue>>>,
        )>,
        Error,
    > {
        while self.handles.len() == self.index {
            if self.tail.is_empty() {
                return Ok(None);
            }

            if let Some(statement) = prepare(conn.as_ptr(), &mut self.tail, self.persistent)? {
                let num = statement.column_count();

                let mut columns = Vec::with_capacity(num);
                let mut column_names = HashMap::with_capacity(num);

                for i in 0..num {
                    let name: UStr = statement.column_name(i).to_owned().into();
                    let type_info = statement
                        .column_decltype(i)
                        .unwrap_or_else(|| statement.column_type_info(i));

                    columns.push(SqliteColumn {
                        ordinal: i,
                        name: name.clone(),
                        type_info,
                    });

                    column_names.insert(name, i);
                }

                self.handles.push(statement);
                self.columns.push(Arc::new(columns));
                self.column_names.push(Arc::new(column_names));
                self.last_row_values.push(None);
            }
        }

        let index = self.index;
        self.index += 1;

        Ok(Some((
            &self.handles[index],
            &mut self.columns[index],
            &self.column_names[index],
            &mut self.last_row_values[index],
        )))
    }

    pub(crate) fn reset(&mut self) {
        self.index = 0;

        for (i, handle) in self.handles.iter().enumerate() {
            SqliteRow::inflate_if_needed(&handle, &self.columns[i], self.last_row_values[i].take());

            unsafe {
                // Reset A Prepared Statement Object
                // https://www.sqlite.org/c3ref/reset.html
                // https://www.sqlite.org/c3ref/clear_bindings.html
                sqlite3_reset(handle.0.as_ptr());
                sqlite3_clear_bindings(handle.0.as_ptr());
            }
        }
    }
}

impl Drop for VirtualStatement {
    fn drop(&mut self) {
        for (i, handle) in self.handles.drain(..).enumerate() {
            SqliteRow::inflate_if_needed(&handle, &self.columns[i], self.last_row_values[i].take());

            unsafe {
                // https://sqlite.org/c3ref/finalize.html
                let status = sqlite3_finalize(handle.0.as_ptr());
                if status == SQLITE_MISUSE {
                    // Panic in case of detected misuse of SQLite API.
                    //
                    // sqlite3_finalize returns it at least in the
                    // case of detected double free, i.e. calling
                    // sqlite3_finalize on already finalized
                    // statement.
                    panic!("Detected sqlite3_finalize misuse.");
                }
            }
        }
    }
}
