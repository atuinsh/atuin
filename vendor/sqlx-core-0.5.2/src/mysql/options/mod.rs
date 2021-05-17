use std::path::{Path, PathBuf};

mod connect;
mod parse;
mod ssl_mode;

use crate::{connection::LogSettings, net::CertificateInput};
pub use ssl_mode::MySqlSslMode;

/// Options and flags which can be used to configure a MySQL connection.
///
/// A value of `MySqlConnectOptions` can be parsed from a connection URI,
/// as described by [MySQL](https://dev.mysql.com/doc/connector-j/8.0/en/connector-j-reference-jdbc-url-format.html).
///
/// The generic format of the connection URL:
///
/// ```text
/// mysql://[host][/database][?properties]
/// ```
///
/// ## Properties
///
/// |Parameter|Default|Description|
/// |---------|-------|-----------|
/// | `ssl-mode` | `PREFERRED` | Determines whether or with what priority a secure SSL TCP/IP connection will be negotiated. See [`MySqlSslMode`]. |
/// | `ssl-ca` | `None` | Sets the name of a file containing a list of trusted SSL Certificate Authorities. |
/// | `statement-cache-capacity` | `100` | The maximum number of prepared statements stored in the cache. Set to `0` to disable. |
/// | `socket` | `None` | Path to the unix domain socket, which will be used instead of TCP if set. |
///
/// # Example
///
/// ```rust,no_run
/// # use sqlx_core::error::Error;
/// # use sqlx_core::connection::{Connection, ConnectOptions};
/// # use sqlx_core::mysql::{MySqlConnectOptions, MySqlConnection, MySqlSslMode};
/// #
/// # fn main() {
/// # #[cfg(feature = "_rt-async-std")]
/// # sqlx_rt::async_std::task::block_on::<_, Result<(), Error>>(async move {
/// // URI connection string
/// let conn = MySqlConnection::connect("mysql://root:password@localhost/db").await?;
///
/// // Manually-constructed options
/// let conn = MySqlConnectOptions::new()
///     .host("localhost")
///     .username("root")
///     .password("password")
///     .database("db")
///     .connect().await?;
/// # Ok(())
/// # }).unwrap();
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct MySqlConnectOptions {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) socket: Option<PathBuf>,
    pub(crate) username: String,
    pub(crate) password: Option<String>,
    pub(crate) database: Option<String>,
    pub(crate) ssl_mode: MySqlSslMode,
    pub(crate) ssl_ca: Option<CertificateInput>,
    pub(crate) statement_cache_capacity: usize,
    pub(crate) charset: String,
    pub(crate) collation: Option<String>,
    pub(crate) log_settings: LogSettings,
}

impl Default for MySqlConnectOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl MySqlConnectOptions {
    /// Creates a new, default set of options ready for configuration
    pub fn new() -> Self {
        Self {
            port: 3306,
            host: String::from("localhost"),
            socket: None,
            username: String::from("root"),
            password: None,
            database: None,
            charset: String::from("utf8mb4"),
            collation: None,
            ssl_mode: MySqlSslMode::Preferred,
            ssl_ca: None,
            statement_cache_capacity: 100,
            log_settings: Default::default(),
        }
    }

    /// Sets the name of the host to connect to.
    ///
    /// The default behavior when the host is not specified,
    /// is to connect to localhost.
    pub fn host(mut self, host: &str) -> Self {
        self.host = host.to_owned();
        self
    }

    /// Sets the port to connect to at the server host.
    ///
    /// The default port for MySQL is `3306`.
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Pass a path to a Unix socket. This changes the connection stream from
    /// TCP to UDS.
    ///
    /// By default set to `None`.
    pub fn socket(mut self, path: impl AsRef<Path>) -> Self {
        self.socket = Some(path.as_ref().to_path_buf());
        self
    }

    /// Sets the username to connect as.
    pub fn username(mut self, username: &str) -> Self {
        self.username = username.to_owned();
        self
    }

    /// Sets the password to connect with.
    pub fn password(mut self, password: &str) -> Self {
        self.password = Some(password.to_owned());
        self
    }

    /// Sets the database name.
    pub fn database(mut self, database: &str) -> Self {
        self.database = Some(database.to_owned());
        self
    }

    /// Sets whether or with what priority a secure SSL TCP/IP connection will be negotiated
    /// with the server.
    ///
    /// By default, the SSL mode is [`Preferred`](MySqlSslMode::Preferred), and the client will
    /// first attempt an SSL connection but fallback to a non-SSL connection on failure.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::mysql::{MySqlSslMode, MySqlConnectOptions};
    /// let options = MySqlConnectOptions::new()
    ///     .ssl_mode(MySqlSslMode::Required);
    /// ```
    pub fn ssl_mode(mut self, mode: MySqlSslMode) -> Self {
        self.ssl_mode = mode;
        self
    }

    /// Sets the name of a file containing a list of trusted SSL Certificate Authorities.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::mysql::{MySqlSslMode, MySqlConnectOptions};
    /// let options = MySqlConnectOptions::new()
    ///     .ssl_mode(MySqlSslMode::VerifyCa)
    ///     .ssl_ca("path/to/ca.crt");
    /// ```
    pub fn ssl_ca(mut self, file_name: impl AsRef<Path>) -> Self {
        self.ssl_ca = Some(CertificateInput::File(file_name.as_ref().to_owned()));
        self
    }

    /// Sets PEM encoded list of trusted SSL Certificate Authorities.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::mysql::{MySqlSslMode, MySqlConnectOptions};
    /// let options = MySqlConnectOptions::new()
    ///     .ssl_mode(MySqlSslMode::VerifyCa)
    ///     .ssl_ca_from_pem(vec![]);
    /// ```
    pub fn ssl_ca_from_pem(mut self, pem_certificate: Vec<u8>) -> Self {
        self.ssl_ca = Some(CertificateInput::Inline(pem_certificate));
        self
    }

    /// Sets the capacity of the connection's statement cache in a number of stored
    /// distinct statements. Caching is handled using LRU, meaning when the
    /// amount of queries hits the defined limit, the oldest statement will get
    /// dropped.
    ///
    /// The default cache capacity is 100 statements.
    pub fn statement_cache_capacity(mut self, capacity: usize) -> Self {
        self.statement_cache_capacity = capacity;
        self
    }

    /// Sets the character set for the connection.
    ///
    /// The default character set is `utf8mb4`. This is supported from MySQL 5.5.3.
    /// If you need to connect to an older version, we recommend you to change this to `utf8`.
    pub fn charset(mut self, charset: &str) -> Self {
        self.charset = charset.to_owned();
        self
    }

    /// Sets the collation for the connection.
    ///
    /// The default collation is derived from the `charset`. Normally, you should only have to set
    /// the `charset`.
    pub fn collation(mut self, collation: &str) -> Self {
        self.collation = Some(collation.to_owned());
        self
    }
}
