use crate::error::Error;
use crate::mssql::MssqlConnectOptions;
use percent_encoding::percent_decode_str;
use std::str::FromStr;
use url::Url;

impl FromStr for MssqlConnectOptions {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url: Url = s.parse().map_err(Error::config)?;
        let mut options = Self::new();

        if let Some(host) = url.host_str() {
            options = options.host(host);
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

        Ok(options)
    }
}

#[test]
fn it_parses_username_with_at_sign_correctly() {
    let uri = "mysql://user@hostname:password@hostname:5432/database";
    let opts = MssqlConnectOptions::from_str(uri).unwrap();

    assert_eq!("user@hostname", &opts.username);
}

#[test]
fn it_parses_password_with_non_ascii_chars_correctly() {
    let uri = "mysql://username:p@ssw0rd@hostname:5432/database";
    let opts = MssqlConnectOptions::from_str(uri).unwrap();

    assert_eq!(Some("p@ssw0rd".into()), opts.password);
}
