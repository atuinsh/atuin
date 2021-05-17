use crate::error::Error;
use crate::mysql::connection::MySqlStream;
use crate::mysql::protocol::connect::SslRequest;
use crate::mysql::protocol::Capabilities;
use crate::mysql::{MySqlConnectOptions, MySqlSslMode};

pub(super) async fn maybe_upgrade(
    stream: &mut MySqlStream,
    options: &MySqlConnectOptions,
) -> Result<(), Error> {
    // https://www.postgresql.org/docs/12/libpq-ssl.html#LIBPQ-SSL-SSLMODE-STATEMENTS
    match options.ssl_mode {
        MySqlSslMode::Disabled => {}

        MySqlSslMode::Preferred => {
            // try upgrade, but its okay if we fail
            upgrade(stream, options).await?;
        }

        MySqlSslMode::Required | MySqlSslMode::VerifyIdentity | MySqlSslMode::VerifyCa => {
            if !upgrade(stream, options).await? {
                // upgrade failed, die
                return Err(Error::Tls("server does not support TLS".into()));
            }
        }
    }

    Ok(())
}

async fn upgrade(stream: &mut MySqlStream, options: &MySqlConnectOptions) -> Result<bool, Error> {
    if !stream.capabilities.contains(Capabilities::SSL) {
        // server does not support TLS
        return Ok(false);
    }

    stream.write_packet(SslRequest {
        max_packet_size: super::MAX_PACKET_SIZE,
        collation: stream.collation as u8,
    });

    stream.flush().await?;

    let accept_invalid_certs = !matches!(
        options.ssl_mode,
        MySqlSslMode::VerifyCa | MySqlSslMode::VerifyIdentity
    );
    let accept_invalid_host_names = !matches!(options.ssl_mode, MySqlSslMode::VerifyIdentity);

    stream
        .upgrade(
            &options.host,
            accept_invalid_certs,
            accept_invalid_host_names,
            options.ssl_ca.as_ref(),
        )
        .await?;

    Ok(true)
}
