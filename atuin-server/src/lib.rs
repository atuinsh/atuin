#![forbid(unsafe_code)]

use std::net::IpAddr;

use eyre::Result;

use crate::settings::Settings;

#[macro_use]
extern crate log;

#[macro_use]
extern crate serde_derive;

pub mod auth;
pub mod database;
pub mod handlers;
pub mod models;
pub mod router;
pub mod settings;

pub async fn launch(settings: &Settings, host: String, port: u16) -> Result<()> {
    // routes to run:
    // index, register, add_history, login, get_user, sync_count, sync_list
    let host = host.parse::<IpAddr>()?;

    let r = router::router(settings).await?;

    warp::serve(r).run((host, port)).await;

    Ok(())
}
