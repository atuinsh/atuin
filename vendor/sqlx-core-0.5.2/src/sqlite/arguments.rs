use crate::arguments::Arguments;
use crate::encode::{Encode, IsNull};
use crate::error::Error;
use crate::sqlite::statement::StatementHandle;
use crate::sqlite::Sqlite;
use atoi::atoi;
use libsqlite3_sys::SQLITE_OK;
use std::borrow::Cow;

#[derive(Debug, Clone)]
pub enum SqliteArgumentValue<'q> {
    Null,
    Text(Cow<'q, str>),
    Blob(Cow<'q, [u8]>),
    Double(f64),
    Int(i32),
    Int64(i64),
}

#[derive(Default, Debug, Clone)]
pub struct SqliteArguments<'q> {
    pub(crate) values: Vec<SqliteArgumentValue<'q>>,
}

impl<'q> SqliteArguments<'q> {
    pub(crate) fn add<T>(&mut self, value: T)
    where
        T: Encode<'q, Sqlite>,
    {
        if let IsNull::Yes = value.encode(&mut self.values) {
            self.values.push(SqliteArgumentValue::Null);
        }
    }
}

impl<'q> Arguments<'q> for SqliteArguments<'q> {
    type Database = Sqlite;

    fn reserve(&mut self, len: usize, _size_hint: usize) {
        self.values.reserve(len);
    }

    fn add<T>(&mut self, value: T)
    where
        T: Encode<'q, Self::Database>,
    {
        self.add(value)
    }
}

impl SqliteArguments<'_> {
    pub(super) fn bind(&self, handle: &StatementHandle, offset: usize) -> Result<usize, Error> {
        let mut arg_i = offset;
        // for handle in &statement.handles {

        let cnt = handle.bind_parameter_count();

        for param_i in 1..=cnt {
            // figure out the index of this bind parameter into our argument tuple
            let n: usize = if let Some(name) = handle.bind_parameter_name(param_i) {
                if let Some(name) = name.strip_prefix('?') {
                    // parameter should have the form ?NNN
                    atoi(name.as_bytes()).expect("parameter of the form ?NNN")
                } else if let Some(name) = name.strip_prefix('$') {
                    // parameter should have the form $NNN
                    atoi(name.as_bytes()).ok_or_else(|| {
                        err_protocol!(
                            "parameters with non-integer names are not currently supported: {}",
                            name
                        )
                    })?
                } else {
                    return Err(err_protocol!("unsupported SQL parameter format: {}", name));
                }
            } else {
                arg_i += 1;
                arg_i
            };

            if n > self.values.len() {
                // SQLite treats unbound variables as NULL
                // we reproduce this here
                // If you are reading this and think this should be an error, open an issue and we can
                // discuss configuring this somehow
                // Note that the query macros have a different way of enforcing
                // argument arity
                break;
            }

            self.values[n - 1].bind(handle, param_i)?;
        }

        Ok(arg_i - offset)
    }
}

impl SqliteArgumentValue<'_> {
    fn bind(&self, handle: &StatementHandle, i: usize) -> Result<(), Error> {
        use SqliteArgumentValue::*;

        let status = match self {
            Text(v) => handle.bind_text(i, v),
            Blob(v) => handle.bind_blob(i, v),
            Int(v) => handle.bind_int(i, *v),
            Int64(v) => handle.bind_int64(i, *v),
            Double(v) => handle.bind_double(i, *v),
            Null => handle.bind_null(i),
        };

        if status != SQLITE_OK {
            return Err(handle.last_error().into());
        }

        Ok(())
    }
}
