use diesel::pg::PgConnection;
use diesel::prelude::*;
use eyre::{eyre, Result};

use crate::settings::Settings;

#[database("atuin")]
pub struct AtuinDbConn(diesel::PgConnection);

// TODO: connection pooling
pub fn establish_connection(settings: &Settings) -> Result<PgConnection> {
    if settings.server.db_uri == "default_uri" {
        Err(eyre!(
            "Please configure your database! Set db_uri in config.toml"
        ))
    } else {
        let database_url = &settings.server.db_uri;
        let conn = PgConnection::establish(database_url)?;

        Ok(conn)
    }
}
