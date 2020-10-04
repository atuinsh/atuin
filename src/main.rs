use std::env;

use clap::{Arg, App, SubCommand};
use eyre::Result;

#[macro_use] extern crate log;
use pretty_env_logger;

mod local;

use local::history::History;
use local::database::{Database, SqliteDatabase};

fn main() -> Result<()> {
    pretty_env_logger::init();

    let db = SqliteDatabase::new("~/.history.db")?;

    let matches = App::new("Shync")
        .version("0.1.0")
        .author("Ellie Huxtable <e@elm.sh>")
        .about("Keep your shell history in sync")
        .subcommand(
            SubCommand::with_name("history")
                .aliases(&["h", "hi", "his", "hist", "histo", "histor"])
                .about("manipulate shell history")
                .subcommand(
                    SubCommand::with_name("add")
                        .aliases(&["a", "ad"])
                        .about("add a new command to the history")
                        .arg(
                            Arg::with_name("command")
                                .multiple(true)
                                .required(true)
                        )
                )
                .subcommand(
                    SubCommand::with_name("list")
                        .aliases(&["l", "li", "lis"])
                        .about("list all items in history")
                )
        )
        .subcommand(
            SubCommand::with_name("import")
                .about("import shell history from file")
        )
        .subcommand(
            SubCommand::with_name("server")
                .about("start a shync server")
        )
        .get_matches();


    if let Some(m) = matches.subcommand_matches("history") {
        if let Some(m) = m.subcommand_matches("add") {
            let words: Vec<&str> = m.values_of("command").unwrap().collect();
            let command = words.join(" ");

            let cwd = env::current_dir()?;
            let h = History::new(
                command.as_str(), 
                cwd.display().to_string().as_str(),
            );

            debug!("adding history: {:?}", h);
            db.save(h)?;
            debug!("saved history to sqlite");
        }
        else if let Some(_m) = m.subcommand_matches("list") {
            db.list()?;
        }
    }

    Ok(())
}
