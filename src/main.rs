#![feature(proc_macro_hygiene)]
#![feature(decl_macro)]
#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::use_self)] // not 100% reliable

use std::path::PathBuf;

use eyre::{eyre, Result};
use structopt::{clap::AppSettings, StructOpt};

#[macro_use]
extern crate log;

#[macro_use]
extern crate rocket;

#[macro_use]
extern crate serde_derive;

#[macro_use]
extern crate diesel;

#[macro_use]
extern crate diesel_migrations;

#[macro_use]
extern crate rocket_contrib;

use command::AtuinCmd;
use local::database::Sqlite;
use settings::Settings;

mod command;
mod local;
mod remote;
mod settings;

pub mod schema;

#[derive(StructOpt)]
#[structopt(
    author = "Ellie Huxtable <e@elm.sh>",
    version = "0.4.0",
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
    fn run(self) -> Result<()> {
        let settings = Settings::new()?;

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

        self.atuin.run(&mut db, &settings)
    }
}

fn main() -> Result<()> {
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} [{}] {}",
                chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                record.level(),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .chain(std::io::stdout())
        .apply()?;

    Atuin::from_args().run()
}
