use crate::any::connection::AnyConnectionKind;
use crate::any::options::{AnyConnectOptions, AnyConnectOptionsKind};
use crate::any::AnyConnection;
use crate::connection::Connection;
use crate::error::Error;

impl AnyConnection {
    pub(crate) async fn establish(options: &AnyConnectOptions) -> Result<Self, Error> {
        match &options.0 {
            #[cfg(feature = "mysql")]
            AnyConnectOptionsKind::MySql(options) => {
                crate::mysql::MySqlConnection::connect_with(options)
                    .await
                    .map(AnyConnectionKind::MySql)
            }

            #[cfg(feature = "postgres")]
            AnyConnectOptionsKind::Postgres(options) => {
                crate::postgres::PgConnection::connect_with(options)
                    .await
                    .map(AnyConnectionKind::Postgres)
            }

            #[cfg(feature = "sqlite")]
            AnyConnectOptionsKind::Sqlite(options) => {
                crate::sqlite::SqliteConnection::connect_with(options)
                    .await
                    .map(AnyConnectionKind::Sqlite)
            }

            #[cfg(feature = "mssql")]
            AnyConnectOptionsKind::Mssql(options) => {
                crate::mssql::MssqlConnection::connect_with(options)
                    .await
                    .map(AnyConnectionKind::Mssql)
            }
        }
        .map(AnyConnection)
    }
}
