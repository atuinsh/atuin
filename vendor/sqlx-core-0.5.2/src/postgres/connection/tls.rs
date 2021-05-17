use bytes::Bytes;

use crate::error::Error;
use crate::postgres::connection::stream::PgStream;
use crate::postgres::message::SslRequest;
use crate::postgres::{PgConnectOptions, PgSslMode};

pub(super) async fn maybe_upgrade(
    stream: &mut PgStream,
    options: &PgConnectOptions,
) -> Result<(), Error> {
    // https://www.postgresql.org/docs/12/libpq-ssl.html#LIBPQ-SSL-SSLMODE-STATEMENTS
    match options.ssl_mode {
        // FIXME: Implement ALLOW
        PgSslMode::Allow | PgSslMode::Disable => {}

        PgSslMode::Prefer => {
            // try upgrade, but its okay if we fail
            upgrade(stream, options).await?;
        }

        PgSslMode::Require | PgSslMode::VerifyFull | PgSslMode::VerifyCa => {
            if !upgrade(stream, options).await? {
                // upgrade failed, die
                return Err(Error::Tls("server does not support TLS".into()));
            }
        }
    }

    Ok(())
}

async fn upgrade(stream: &mut PgStream, options: &PgConnectOptions) -> Result<bool, Error> {
    // https://www.postgresql.org/docs/current/protocol-flow.html#id-1.10.5.7.11

    // To initiate an SSL-encrypted connection, the frontend initially sends an
    // SSLRequest message rather than a StartupMessage

    stream.send(SslRequest).await?;

    // The server then responds with a single byte containing S or N, indicating that
    // it is willing or unwilling to perform SSL, respectively.

    match stream.read::<Bytes>(1).await?[0] {
        b'S' => {
            // The server is ready and willing to accept an SSL connection
        }

        b'N' => {
            // The server is _unwilling_ to perform SSL
            return Ok(false);
        }

        other => {
            return Err(err_protocol!(
                "unexpected response from SSLRequest: 0x{:02x}",
                other
            ));
        }
    }

    let accept_invalid_certs = !matches!(
        options.ssl_mode,
        PgSslMode::VerifyCa | PgSslMode::VerifyFull
    );
    let accept_invalid_hostnames = !matches!(options.ssl_mode, PgSslMode::VerifyFull);

    stream
        .upgrade(
            &options.host,
            accept_invalid_certs,
            accept_invalid_hostnames,
            options.ssl_root_cert.as_ref(),
        )
        .await?;

    Ok(true)
}
