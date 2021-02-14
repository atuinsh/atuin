use chrono::{Duration, Timelike, Utc};
use chrono_english::{parse_date_string, Dialect};
use eyre::Result;
use structopt::StructOpt;

use crate::local::database::{Database, Sqlite};

#[derive(StructOpt)]
pub enum Cmd {
    #[structopt(
        about="compute statistics per day of the week",
        aliases=&["d", "da"],
    )]
    Day { words: Vec<String> },
}

impl Cmd {
    pub fn run(&self, db: &mut Sqlite) -> Result<()> {
        match self {
            Self::Day { words } => {
                debug!("words len {}", words.len());

                let words = if words.is_empty() {
                    String::from("yesterday")
                } else {
                    words.join(" ")
                };

                let datetime = parse_date_string(words.as_str(), Utc::now(), Dialect::Us)?;

                debug!("stats for {}", datetime);

                let datetime = datetime
                    .with_hour(0)
                    .unwrap()
                    .with_minute(0)
                    .unwrap()
                    .with_second(0)
                    .unwrap();

                let history = db.range(datetime, datetime + Duration::days(1))?;

                for i in history {
                    println!("{}", i.command);
                }

                Ok(())
            }
        }
    }
}
