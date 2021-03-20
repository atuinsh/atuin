use diesel::pg::PgConnection;
use diesel::prelude::*;

use crate::settings::Settings;

#[database("atuin")]
pub struct AtuinDbConn(diesel::PgConnection);

// TODO: connection pooling
pub fn establish_connection(settings: &Settings) -> PgConnection {
    let database_url = &settings.remote.db.url;
    PgConnection::establish(database_url).expect(&format!("Error connecting to {}", database_url))
}
