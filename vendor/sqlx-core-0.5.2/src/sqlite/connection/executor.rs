use crate::common::StatementCache;
use crate::describe::Describe;
use crate::error::Error;
use crate::executor::{Execute, Executor};
use crate::logger::QueryLogger;
use crate::sqlite::connection::describe::describe;
use crate::sqlite::statement::{StatementHandle, VirtualStatement};
use crate::sqlite::{
    Sqlite, SqliteArguments, SqliteConnection, SqliteQueryResult, SqliteRow, SqliteStatement,
    SqliteTypeInfo,
};
use either::Either;
use futures_core::future::BoxFuture;
use futures_core::stream::BoxStream;
use libsqlite3_sys::sqlite3_last_insert_rowid;
use std::borrow::Cow;
use std::sync::Arc;

fn prepare<'a>(
    statements: &'a mut StatementCache<VirtualStatement>,
    statement: &'a mut Option<VirtualStatement>,
    query: &str,
    persistent: bool,
) -> Result<&'a mut VirtualStatement, Error> {
    if !persistent || !statements.is_enabled() {
        *statement = Some(VirtualStatement::new(query, false)?);
        return Ok(statement.as_mut().unwrap());
    }

    let exists = statements.contains_key(query);

    if !exists {
        let statement = VirtualStatement::new(query, true)?;
        statements.insert(query, statement);
    }

    let statement = statements.get_mut(query).unwrap();

    if exists {
        // as this statement has been executed before, we reset before continuing
        // this also causes any rows that are from the statement to be inflated
        statement.reset();
    }

    Ok(statement)
}

fn bind(
    statement: &StatementHandle,
    arguments: &Option<SqliteArguments<'_>>,
    offset: usize,
) -> Result<usize, Error> {
    let mut n = 0;

    if let Some(arguments) = arguments {
        n += arguments.bind(statement, offset)?;
    }

    Ok(n)
}

/// A structure holding sqlite statement handle and resetting the
/// statement when it is dropped.
struct StatementResetter {
    handle: StatementHandle,
}

impl StatementResetter {
    fn new(handle: StatementHandle) -> Self {
        Self { handle }
    }
}

impl Drop for StatementResetter {
    fn drop(&mut self) {
        self.handle.reset();
    }
}

impl<'c> Executor<'c> for &'c mut SqliteConnection {
    type Database = Sqlite;

    fn fetch_many<'e, 'q: 'e, E: 'q>(
        self,
        mut query: E,
    ) -> BoxStream<'e, Result<Either<SqliteQueryResult, SqliteRow>, Error>>
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        let sql = query.sql();
        let mut logger = QueryLogger::new(sql, self.log_settings.clone());
        let arguments = query.take_arguments();
        let persistent = query.persistent() && arguments.is_some();

        Box::pin(try_stream! {
            let SqliteConnection {
                handle: ref mut conn,
                ref mut statements,
                ref mut statement,
                ref mut worker,
                ..
            } = self;

            // prepare statement object (or checkout from cache)
            let stmt = prepare(statements, statement, sql, persistent)?;

            // keep track of how many arguments we have bound
            let mut num_arguments = 0;

            while let Some((stmt, columns, column_names, last_row_values)) = stmt.prepare(conn)? {
                // Prepare to reset raw SQLite statement when the handle
                // is dropped. `StatementResetter` will reliably reset the
                // statement even if the stream returned from `fetch_many`
                // is dropped early.
                let _resetter = StatementResetter::new(*stmt);

                // bind values to the statement
                num_arguments += bind(stmt, &arguments, num_arguments)?;

                loop {
                    // save the rows from the _current_ position on the statement
                    // and send them to the still-live row object
                    SqliteRow::inflate_if_needed(stmt, &*columns, last_row_values.take());

                    // invoke [sqlite3_step] on the dedicated worker thread
                    // this will move us forward one row or finish the statement
                    let s = worker.step(*stmt).await?;

                    match s {
                        Either::Left(changes) => {
                            let last_insert_rowid = unsafe {
                                sqlite3_last_insert_rowid(conn.as_ptr())
                            };

                            let done = SqliteQueryResult {
                                changes,
                                last_insert_rowid,
                            };

                            r#yield!(Either::Left(done));

                            break;
                        }

                        Either::Right(()) => {
                            let (row, weak_values_ref) = SqliteRow::current(
                                *stmt,
                                columns,
                                column_names
                            );

                            let v = Either::Right(row);
                            *last_row_values = Some(weak_values_ref);

                            logger.increment_rows();

                            r#yield!(v);
                        }
                    }
                }
            }

            Ok(())
        })
    }

    fn fetch_optional<'e, 'q: 'e, E: 'q>(
        self,
        mut query: E,
    ) -> BoxFuture<'e, Result<Option<SqliteRow>, Error>>
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        let sql = query.sql();
        let mut logger = QueryLogger::new(sql, self.log_settings.clone());
        let arguments = query.take_arguments();
        let persistent = query.persistent() && arguments.is_some();

        Box::pin(async move {
            let SqliteConnection {
                handle: ref mut conn,
                ref mut statements,
                ref mut statement,
                ref mut worker,
                ..
            } = self;

            // prepare statement object (or checkout from cache)
            let virtual_stmt = prepare(statements, statement, sql, persistent)?;

            // keep track of how many arguments we have bound
            let mut num_arguments = 0;

            while let Some((stmt, columns, column_names, last_row_values)) =
                virtual_stmt.prepare(conn)?
            {
                // bind values to the statement
                num_arguments += bind(stmt, &arguments, num_arguments)?;

                // save the rows from the _current_ position on the statement
                // and send them to the still-live row object
                SqliteRow::inflate_if_needed(stmt, &*columns, last_row_values.take());

                // invoke [sqlite3_step] on the dedicated worker thread
                // this will move us forward one row or finish the statement
                match worker.step(*stmt).await? {
                    Either::Left(_) => (),

                    Either::Right(()) => {
                        let (row, weak_values_ref) =
                            SqliteRow::current(*stmt, columns, column_names);

                        *last_row_values = Some(weak_values_ref);

                        logger.increment_rows();

                        virtual_stmt.reset();
                        return Ok(Some(row));
                    }
                }
            }
            Ok(None)
        })
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        _parameters: &[SqliteTypeInfo],
    ) -> BoxFuture<'e, Result<SqliteStatement<'q>, Error>>
    where
        'c: 'e,
    {
        Box::pin(async move {
            let SqliteConnection {
                handle: ref mut conn,
                ref mut statements,
                ref mut statement,
                ..
            } = self;

            // prepare statement object (or checkout from cache)
            let statement = prepare(statements, statement, sql, true)?;

            let mut parameters = 0;
            let mut columns = None;
            let mut column_names = None;

            while let Some((statement, columns_, column_names_, _)) = statement.prepare(conn)? {
                parameters += statement.bind_parameter_count();

                // the first non-empty statement is chosen as the statement we pull columns from
                if !columns_.is_empty() && columns.is_none() {
                    columns = Some(Arc::clone(columns_));
                    column_names = Some(Arc::clone(column_names_));
                }
            }

            Ok(SqliteStatement {
                sql: Cow::Borrowed(sql),
                columns: columns.unwrap_or_default(),
                column_names: column_names.unwrap_or_default(),
                parameters,
            })
        })
    }

    #[doc(hidden)]
    fn describe<'e, 'q: 'e>(self, sql: &'q str) -> BoxFuture<'e, Result<Describe<Sqlite>, Error>>
    where
        'c: 'e,
    {
        describe(self, sql)
    }
}
