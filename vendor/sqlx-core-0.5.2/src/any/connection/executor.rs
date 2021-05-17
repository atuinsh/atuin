use crate::any::connection::AnyConnectionKind;
use crate::any::{
    Any, AnyColumn, AnyConnection, AnyQueryResult, AnyRow, AnyStatement, AnyTypeInfo,
};
use crate::database::Database;
use crate::describe::Describe;
use crate::error::Error;
use crate::executor::{Execute, Executor};
use either::Either;
use futures_core::future::BoxFuture;
use futures_core::stream::BoxStream;
use futures_util::{StreamExt, TryStreamExt};

impl<'c> Executor<'c> for &'c mut AnyConnection {
    type Database = Any;

    fn fetch_many<'e, 'q: 'e, E: 'q>(
        self,
        mut query: E,
    ) -> BoxStream<'e, Result<Either<AnyQueryResult, AnyRow>, Error>>
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        let arguments = query.take_arguments();
        let query = query.sql();

        match &mut self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn
                .fetch_many((query, arguments.map(Into::into)))
                .map_ok(|v| v.map_right(Into::into).map_left(Into::into))
                .boxed(),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn
                .fetch_many((query, arguments.map(Into::into)))
                .map_ok(|v| v.map_right(Into::into).map_left(Into::into))
                .boxed(),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn
                .fetch_many((query, arguments.map(Into::into)))
                .map_ok(|v| v.map_right(Into::into).map_left(Into::into))
                .boxed(),

            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(conn) => conn
                .fetch_many((query, arguments.map(Into::into)))
                .map_ok(|v| v.map_right(Into::into).map_left(Into::into))
                .boxed(),
        }
    }

    fn fetch_optional<'e, 'q: 'e, E: 'q>(
        self,
        mut query: E,
    ) -> BoxFuture<'e, Result<Option<AnyRow>, Error>>
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        let arguments = query.take_arguments();
        let query = query.sql();

        Box::pin(async move {
            Ok(match &mut self.0 {
                #[cfg(feature = "postgres")]
                AnyConnectionKind::Postgres(conn) => conn
                    .fetch_optional((query, arguments.map(Into::into)))
                    .await?
                    .map(Into::into),

                #[cfg(feature = "mysql")]
                AnyConnectionKind::MySql(conn) => conn
                    .fetch_optional((query, arguments.map(Into::into)))
                    .await?
                    .map(Into::into),

                #[cfg(feature = "sqlite")]
                AnyConnectionKind::Sqlite(conn) => conn
                    .fetch_optional((query, arguments.map(Into::into)))
                    .await?
                    .map(Into::into),

                #[cfg(feature = "mssql")]
                AnyConnectionKind::Mssql(conn) => conn
                    .fetch_optional((query, arguments.map(Into::into)))
                    .await?
                    .map(Into::into),
            })
        })
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        _parameters: &[AnyTypeInfo],
    ) -> BoxFuture<'e, Result<AnyStatement<'q>, Error>>
    where
        'c: 'e,
    {
        Box::pin(async move {
            Ok(match &mut self.0 {
                // To match other databases here, we explicitly ignore the parameter types
                #[cfg(feature = "postgres")]
                AnyConnectionKind::Postgres(conn) => conn.prepare(sql).await.map(Into::into)?,

                #[cfg(feature = "mysql")]
                AnyConnectionKind::MySql(conn) => conn.prepare(sql).await.map(Into::into)?,

                #[cfg(feature = "sqlite")]
                AnyConnectionKind::Sqlite(conn) => conn.prepare(sql).await.map(Into::into)?,

                #[cfg(feature = "mssql")]
                AnyConnectionKind::Mssql(conn) => conn.prepare(sql).await.map(Into::into)?,
            })
        })
    }

    fn describe<'e, 'q: 'e>(
        self,
        sql: &'q str,
    ) -> BoxFuture<'e, Result<Describe<Self::Database>, Error>>
    where
        'c: 'e,
    {
        Box::pin(async move {
            Ok(match &mut self.0 {
                #[cfg(feature = "postgres")]
                AnyConnectionKind::Postgres(conn) => conn.describe(sql).await.map(map_describe)?,

                #[cfg(feature = "mysql")]
                AnyConnectionKind::MySql(conn) => conn.describe(sql).await.map(map_describe)?,

                #[cfg(feature = "sqlite")]
                AnyConnectionKind::Sqlite(conn) => conn.describe(sql).await.map(map_describe)?,

                #[cfg(feature = "mssql")]
                AnyConnectionKind::Mssql(conn) => conn.describe(sql).await.map(map_describe)?,
            })
        })
    }
}

fn map_describe<DB: Database>(info: Describe<DB>) -> Describe<Any>
where
    AnyTypeInfo: From<DB::TypeInfo>,
    AnyColumn: From<DB::Column>,
{
    let parameters = match info.parameters {
        None => None,
        Some(Either::Right(num)) => Some(Either::Right(num)),
        Some(Either::Left(params)) => {
            Some(Either::Left(params.into_iter().map(Into::into).collect()))
        }
    };

    Describe {
        parameters,
        nullable: info.nullable,
        columns: info.columns.into_iter().map(Into::into).collect(),
    }
}
