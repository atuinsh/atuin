use diesel::pg::PgConnection;
use diesel::prelude::*;

use crate::settings::Settings;

#[database("atuin")]
pub struct AtuinDbConn(diesel::PgConnection);

// TODO: connection pooling
pub fn establish_connection(settings: &Settings) -> PgConnection {
    let database_url = &settings.server.db_uri;
    PgConnection::establish(database_url)
        .unwrap_or_else(|_| panic!("Error connecting to {}", database_url))
}
