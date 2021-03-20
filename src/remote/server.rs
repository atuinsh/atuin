use rocket::config::{Config, Environment, LoggingLevel, Value};
use rocket_contrib::databases::diesel;

use std::collections::HashMap;

use crate::remote::database::establish_connection;
use crate::settings::Settings;

embed_migrations!("migrations");

#[database("atuin")]
struct AtuinDbConn(diesel::PgConnection);

#[get("/")]
fn index(_conn: AtuinDbConn) -> &'static str {
    "Hello, world!"
}

pub fn launch(settings: &Settings) {
    let mut database_config = HashMap::new();
    let mut databases = HashMap::new();

    database_config.insert("url", Value::from(settings.remote.db.url.clone()));
    databases.insert("atuin", Value::from(database_config));

    let connection = establish_connection(settings);
    embedded_migrations::run(&connection).expect("failed to run migrations");

    let config = Config::build(Environment::Production)
        .address("0.0.0.0")
        .log_level(LoggingLevel::Normal)
        .port(8080)
        .extra("databases", databases)
        .finalize()
        .unwrap();

    let app = rocket::custom(config);
    app.mount("/", routes![index]).launch();
}
