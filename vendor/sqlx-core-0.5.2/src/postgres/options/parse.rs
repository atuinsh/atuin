use crate::error::Error;
use crate::postgres::PgConnectOptions;
use percent_encoding::percent_decode_str;
use std::net::IpAddr;
use std::str::FromStr;
use url::Url;

impl FromStr for PgConnectOptions {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let url: Url = s.parse().map_err(Error::config)?;

        let mut options = Self::default();

        if let Some(host) = url.host_str() {
            let host_decoded = percent_decode_str(host);
            options = match host_decoded.clone().next() {
                Some(b'/') => options.socket(&*host_decoded.decode_utf8().map_err(Error::config)?),
                _ => options.host(host),
            }
        }

        if let Some(port) = url.port() {
            options = options.port(port);
        }

        let username = url.username();
        if !username.is_empty() {
            options = options.username(
                &*percent_decode_str(username)
                    .decode_utf8()
                    .map_err(Error::config)?,
            );
        }

        if let Some(password) = url.password() {
            options = options.password(
                &*percent_decode_str(password)
                    .decode_utf8()
                    .map_err(Error::config)?,
            );
        }

        let path = url.path().trim_start_matches('/');
        if !path.is_empty() {
            options = options.database(path);
        }

        for (key, value) in url.query_pairs().into_iter() {
            match &*key {
                "sslmode" | "ssl-mode" => {
                    options = options.ssl_mode(value.parse().map_err(Error::config)?);
                }

                "sslrootcert" | "ssl-root-cert" | "ssl-ca" => {
                    options = options.ssl_root_cert(&*value);
                }

                "statement-cache-capacity" => {
                    options =
                        options.statement_cache_capacity(value.parse().map_err(Error::config)?);
                }

                "host" => {
                    if value.starts_with("/") {
                        options = options.socket(&*value);
                    } else {
                        options = options.host(&*value);
                    }
                }

                "hostaddr" => {
                    value.parse::<IpAddr>().map_err(Error::config)?;
                    options = options.host(&*value)
                }

                "port" => options = options.port(value.parse().map_err(Error::config)?),

                "dbname" => options = options.database(&*value),

                "user" => options = options.username(&*value),

                "password" => options = options.password(&*value),

                "application_name" => options = options.application_name(&*value),

                _ => log::warn!("ignoring unrecognized connect parameter: {}={}", key, value),
            }
        }

        Ok(options)
    }
}

#[test]
fn it_parses_socket_correctly_from_parameter() {
    let uri = "postgres:///?host=/var/run/postgres/";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!(Some("/var/run/postgres/".into()), opts.socket);
}

#[test]
fn it_parses_host_correctly_from_parameter() {
    let uri = "postgres:///?host=google.database.com";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!(None, opts.socket);
    assert_eq!("google.database.com", &opts.host);
}

#[test]
fn it_parses_hostaddr_correctly_from_parameter() {
    let uri = "postgres:///?hostaddr=8.8.8.8";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!(None, opts.socket);
    assert_eq!("8.8.8.8", &opts.host);
}

#[test]
fn it_parses_port_correctly_from_parameter() {
    let uri = "postgres:///?port=1234";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!(None, opts.socket);
    assert_eq!(1234, opts.port);
}

#[test]
fn it_parses_dbname_correctly_from_parameter() {
    let uri = "postgres:///?dbname=some_db";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!(None, opts.socket);
    assert_eq!(Some("some_db"), opts.database.as_deref());
}

#[test]
fn it_parses_user_correctly_from_parameter() {
    let uri = "postgres:///?user=some_user";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!(None, opts.socket);
    assert_eq!("some_user", opts.username);
}

#[test]
fn it_parses_password_correctly_from_parameter() {
    let uri = "postgres:///?password=some_pass";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!(None, opts.socket);
    assert_eq!(Some("some_pass"), opts.password.as_deref());
}

#[test]
fn it_parses_application_name_correctly_from_parameter() {
    let uri = "postgres:///?application_name=some_name";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!(Some("some_name"), opts.application_name.as_deref());
}

#[test]
fn it_parses_username_with_at_sign_correctly() {
    let uri = "postgres://user@hostname:password@hostname:5432/database";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!("user@hostname", &opts.username);
}

#[test]
fn it_parses_password_with_non_ascii_chars_correctly() {
    let uri = "postgres://username:p@ssw0rd@hostname:5432/database";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!(Some("p@ssw0rd".into()), opts.password);
}

#[test]
fn it_parses_socket_correctly_percent_encoded() {
    let uri = "postgres://%2Fvar%2Flib%2Fpostgres/database";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!(Some("/var/lib/postgres/".into()), opts.socket);
}
#[test]
fn it_parses_socket_correctly_with_username_percent_encoded() {
    let uri = "postgres://some_user@%2Fvar%2Flib%2Fpostgres/database";
    let opts = PgConnectOptions::from_str(uri).unwrap();

    assert_eq!("some_user", opts.username);
    assert_eq!(Some("/var/lib/postgres/".into()), opts.socket);
    assert_eq!(Some("database"), opts.database.as_deref());
}
