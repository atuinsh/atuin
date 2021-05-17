use std::env::var;
use std::path::{Path, PathBuf};

mod connect;
mod parse;
mod pgpass;
mod ssl_mode;
use crate::{connection::LogSettings, net::CertificateInput};
pub use ssl_mode::PgSslMode;

/// Options and flags which can be used to configure a PostgreSQL connection.
///
/// A value of `PgConnectOptions` can be parsed from a connection URI,
/// as described by [libpq](https://www.postgresql.org/docs/current/libpq-connect.html#LIBPQ-CONNSTRING).
///
/// The general form for a connection URI is:
///
/// ```text
/// postgresql://[user[:password]@][host][:port][/dbname][?param1=value1&...]
/// ```
///
/// ## Parameters
///
/// |Parameter|Default|Description|
/// |---------|-------|-----------|
/// | `sslmode` | `prefer` | Determines whether or with what priority a secure SSL TCP/IP connection will be negotiated. See [`PgSslMode`]. |
/// | `sslrootcert` | `None` | Sets the name of a file containing a list of trusted SSL Certificate Authorities. |
/// | `statement-cache-capacity` | `100` | The maximum number of prepared statements stored in the cache. Set to `0` to disable. |
/// | `host` | `None` | Path to the directory containing a PostgreSQL unix domain socket, which will be used instead of TCP if set. |
/// | `hostaddr` | `None` | Same as `host`, but only accepts IP addresses. |
/// | `application-name` | `None` | The name will be displayed in the pg_stat_activity view and included in CSV log entries. |
/// | `user` | result of `whoami` | PostgreSQL user name to connect as. |
/// | `password` | `None` | Password to be used if the server demands password authentication. |
/// | `port` | `5432` | Port number to connect to at the server host, or socket file name extension for Unix-domain connections. |
/// | `dbname` | `None` | The database name. |
///
/// The URI scheme designator can be either `postgresql://` or `postgres://`.
/// Each of the URI parts is optional.
///
/// ```text
/// postgresql://
/// postgresql://localhost
/// postgresql://localhost:5433
/// postgresql://localhost/mydb
/// postgresql://user@localhost
/// postgresql://user:secret@localhost
/// postgresql://localhost?dbname=mydb&user=postgres&password=postgres
/// ```
///
/// # Example
///
/// ```rust,no_run
/// # use sqlx_core::error::Error;
/// # use sqlx_core::connection::{Connection, ConnectOptions};
/// # use sqlx_core::postgres::{PgConnectOptions, PgConnection, PgSslMode};
/// #
/// # fn main() {
/// # #[cfg(feature = "_rt-async-std")]
/// # sqlx_rt::async_std::task::block_on::<_, Result<(), Error>>(async move {
/// // URI connection string
/// let conn = PgConnection::connect("postgres://localhost/mydb").await?;
///
/// // Manually-constructed options
/// let conn = PgConnectOptions::new()
///     .host("secret-host")
///     .port(2525)
///     .username("secret-user")
///     .password("secret-password")
///     .ssl_mode(PgSslMode::Require)
///     .connect().await?;
/// # Ok(())
/// # }).unwrap();
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct PgConnectOptions {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) socket: Option<PathBuf>,
    pub(crate) username: String,
    pub(crate) password: Option<String>,
    pub(crate) database: Option<String>,
    pub(crate) ssl_mode: PgSslMode,
    pub(crate) ssl_root_cert: Option<CertificateInput>,
    pub(crate) statement_cache_capacity: usize,
    pub(crate) application_name: Option<String>,
    pub(crate) log_settings: LogSettings,
}

impl Default for PgConnectOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl PgConnectOptions {
    /// Creates a new, default set of options ready for configuration.
    ///
    /// By default, this reads the following environment variables and sets their
    /// equivalent options.
    ///
    ///  * `PGHOST`
    ///  * `PGPORT`
    ///  * `PGUSER`
    ///  * `PGPASSWORD`
    ///  * `PGDATABASE`
    ///  * `PGSSLROOTCERT`
    ///  * `PGSSLMODE`
    ///  * `PGAPPNAME`
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::postgres::PgConnectOptions;
    /// let options = PgConnectOptions::new();
    /// ```
    pub fn new() -> Self {
        let port = var("PGPORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5432);

        let host = var("PGHOST").ok().unwrap_or_else(|| default_host(port));

        let username = var("PGUSER").ok().unwrap_or_else(whoami::username);

        let database = var("PGDATABASE").ok();

        let password = var("PGPASSWORD")
            .ok()
            .or_else(|| pgpass::load_password(&host, port, &username, database.as_deref()));

        PgConnectOptions {
            port,
            host,
            socket: None,
            username,
            password,
            database,
            ssl_root_cert: var("PGSSLROOTCERT").ok().map(CertificateInput::from),
            ssl_mode: var("PGSSLMODE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            statement_cache_capacity: 100,
            application_name: var("PGAPPNAME").ok(),
            log_settings: Default::default(),
        }
    }

    /// Sets the name of the host to connect to.
    ///
    /// If a host name begins with a slash, it specifies
    /// Unix-domain communication rather than TCP/IP communication; the value is the name of
    /// the directory in which the socket file is stored.
    ///
    /// The default behavior when host is not specified, or is empty,
    /// is to connect to a Unix-domain socket
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::postgres::PgConnectOptions;
    /// let options = PgConnectOptions::new()
    ///     .host("localhost");
    /// ```
    pub fn host(mut self, host: &str) -> Self {
        self.host = host.to_owned();
        self
    }

    /// Sets the port to connect to at the server host.
    ///
    /// The default port for PostgreSQL is `5432`.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::postgres::PgConnectOptions;
    /// let options = PgConnectOptions::new()
    ///     .port(5432);
    /// ```
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Sets a custom path to a directory containing a unix domain socket,
    /// switching the connection method from TCP to the corresponding socket.
    ///
    /// By default set to `None`.
    pub fn socket(mut self, path: impl AsRef<Path>) -> Self {
        self.socket = Some(path.as_ref().to_path_buf());
        self
    }

    /// Sets the username to connect as.
    ///
    /// Defaults to be the same as the operating system name of
    /// the user running the application.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::postgres::PgConnectOptions;
    /// let options = PgConnectOptions::new()
    ///     .username("postgres");
    /// ```
    pub fn username(mut self, username: &str) -> Self {
        self.username = username.to_owned();
        self
    }

    /// Sets the password to use if the server demands password authentication.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::postgres::PgConnectOptions;
    /// let options = PgConnectOptions::new()
    ///     .username("root")
    ///     .password("safe-and-secure");
    /// ```
    pub fn password(mut self, password: &str) -> Self {
        self.password = Some(password.to_owned());
        self
    }

    /// Sets the database name. Defaults to be the same as the user name.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::postgres::PgConnectOptions;
    /// let options = PgConnectOptions::new()
    ///     .database("postgres");
    /// ```
    pub fn database(mut self, database: &str) -> Self {
        self.database = Some(database.to_owned());
        self
    }

    /// Sets whether or with what priority a secure SSL TCP/IP connection will be negotiated
    /// with the server.
    ///
    /// By default, the SSL mode is [`Prefer`](PgSslMode::Prefer), and the client will
    /// first attempt an SSL connection but fallback to a non-SSL connection on failure.
    ///
    /// Ignored for Unix domain socket communication.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::postgres::{PgSslMode, PgConnectOptions};
    /// let options = PgConnectOptions::new()
    ///     .ssl_mode(PgSslMode::Require);
    /// ```
    pub fn ssl_mode(mut self, mode: PgSslMode) -> Self {
        self.ssl_mode = mode;
        self
    }

    /// Sets the name of a file containing SSL certificate authority (CA) certificate(s).
    /// If the file exists, the server's certificate will be verified to be signed by
    /// one of these authorities.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::postgres::{PgSslMode, PgConnectOptions};
    /// let options = PgConnectOptions::new()
    ///     // Providing a CA certificate with less than VerifyCa is pointless
    ///     .ssl_mode(PgSslMode::VerifyCa)
    ///     .ssl_root_cert("./ca-certificate.crt");
    /// ```
    pub fn ssl_root_cert(mut self, cert: impl AsRef<Path>) -> Self {
        self.ssl_root_cert = Some(CertificateInput::File(cert.as_ref().to_path_buf()));
        self
    }

    /// Sets PEM encoded trusted SSL Certificate Authorities (CA).
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::postgres::{PgSslMode, PgConnectOptions};
    /// let options = PgConnectOptions::new()
    ///     // Providing a CA certificate with less than VerifyCa is pointless
    ///     .ssl_mode(PgSslMode::VerifyCa)
    ///     .ssl_root_cert_from_pem(vec![]);
    /// ```
    pub fn ssl_root_cert_from_pem(mut self, pem_certificate: Vec<u8>) -> Self {
        self.ssl_root_cert = Some(CertificateInput::Inline(pem_certificate));
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

    /// Sets the application name. Defaults to None
    ///
    /// # Example
    ///
    /// ```rust
    /// # use sqlx_core::postgres::PgConnectOptions;
    /// let options = PgConnectOptions::new()
    ///     .application_name("my-app");
    /// ```
    pub fn application_name(mut self, application_name: &str) -> Self {
        self.application_name = Some(application_name.to_owned());
        self
    }

    /// We try using a socket if hostname starts with `/` or if socket parameter
    /// is specified.
    pub(crate) fn fetch_socket(&self) -> Option<String> {
        match self.socket {
            Some(ref socket) => {
                let full_path = format!("{}/.s.PGSQL.{}", socket.display(), self.port);
                Some(full_path)
            }
            None if self.host.starts_with('/') => {
                let full_path = format!("{}/.s.PGSQL.{}", self.host, self.port);
                Some(full_path)
            }
            _ => None,
        }
    }
}

fn default_host(port: u16) -> String {
    // try to check for the existence of a unix socket and uses that
    let socket = format!(".s.PGSQL.{}", port);
    let candidates = [
        "/var/run/postgresql", // Debian
        "/private/tmp",        // OSX (homebrew)
        "/tmp",                // Default
    ];

    for candidate in &candidates {
        if Path::new(candidate).join(&socket).exists() {
            return candidate.to_string();
        }
    }

    // fallback to localhost if no socket was found
    "localhost".to_owned()
}
