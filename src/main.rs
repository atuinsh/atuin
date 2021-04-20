#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::use_self)] // not 100% reliable

use std::path::PathBuf;

use eyre::{eyre, Result};
use fern::colors::{Color, ColoredLevelConfig};
use human_panic::setup_panic;
use structopt::{clap::AppSettings, StructOpt};

#[macro_use]
extern crate log;

#[macro_use]
extern crate serde_derive;

use command::AtuinCmd;
use local::database::Sqlite;
use settings::Settings;

mod api;
mod command;
mod local;
mod server;
mod settings;
mod utils;

#[derive(StructOpt)]
#[structopt(
    author = "Ellie Huxtable <e@elm.sh>",
    version = "0.5.0",
    about = "Magical shell history",
    global_settings(&[AppSettings::ColoredHelp, AppSettings::DeriveDisplayOrder])
)]
struct Atuin {
    #[structopt(long, parse(from_os_str), help = "db file path")]
    db: Option<PathBuf>,

    #[structopt(subcommand)]
    atuin: AtuinCmd,
}

impl Atuin {
    async fn run(self, settings: &Settings) -> Result<()> {
        let db_path = if let Some(db_path) = self.db {
            let path = db_path
                .to_str()
                .ok_or_else(|| eyre!("path {:?} was not valid UTF-8", db_path))?;
            let path = shellexpand::full(path)?;
            PathBuf::from(path.as_ref())
        } else {
            PathBuf::from(settings.local.db_path.as_str())
        };

        let mut db = Sqlite::new(db_path)?;

        self.atuin.run(&mut db, settings).await
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let colors = ColoredLevelConfig::new()
        .warn(Color::Yellow)
        .error(Color::Red);

    fern::Dispatch::new()
        .format(move |out, message, record| {
            out.finish(format_args!(
                "{} [{}] {}",
                chrono::Local::now().to_rfc3339(),
                colors.color(record.level()),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .level_for("sqlx", log::LevelFilter::Warn)
        .chain(std::io::stdout())
        .apply()?;

    let settings = Settings::new()?;
    setup_panic!();

    Atuin::from_args().run(&settings).await
}
