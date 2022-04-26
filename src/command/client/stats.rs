use std::collections::HashMap;

use chrono::{prelude::*, Duration};
use chrono_english::parse_date_string;
use clap::Parser;
use cli_table::{format::Justify, print_stdout, Cell, Style, Table};
use eyre::{bail, Result};

use atuin_client::{
    database::{current_context, Database},
    history::History,
    settings::{FilterMode, Settings},
};

#[derive(Parser)]
#[clap(infer_subcommands = true)]
pub enum Cmd {
    /// Compute statistics for all of time
    All,

    /// Compute statistics for a single day
    Day { words: Vec<String> },
}

fn compute_stats(history: &[History]) -> Result<()> {
    let mut commands = HashMap::<String, i64>::new();

    for i in history {
        *commands.entry(i.command.clone()).or_default() += 1;
    }

    let most_common_command = commands.iter().max_by(|a, b| a.1.cmp(b.1));

    if most_common_command.is_none() {
        bail!("No commands found");
    }

    let table = vec![
        vec![
            "Most used command".cell(),
            most_common_command
                .unwrap()
                .0
                .cell()
                .justify(Justify::Right),
        ],
        vec![
            "Commands ran".cell(),
            history.len().to_string().cell().justify(Justify::Right),
        ],
        vec![
            "Unique commands ran".cell(),
            commands.len().to_string().cell().justify(Justify::Right),
        ],
    ]
    .table()
    .title(vec![
        "Statistic".cell().bold(true),
        "Value".cell().bold(true),
    ])
    .bold(true);

    print_stdout(table)?;

    Ok(())
}

impl Cmd {
    pub async fn run(
        &self,
        db: &mut (impl Database + Send + Sync),
        settings: &Settings,
    ) -> Result<()> {
        let context = current_context();

        match self {
            Self::Day { words } => {
                let words = if words.is_empty() {
                    String::from("yesterday")
                } else {
                    words.join(" ")
                };

                let start = parse_date_string(&words, Local::now(), settings.dialect.into())?;
                let end = start + Duration::days(1);

                let history = db.range(start.into(), end.into()).await?;

                compute_stats(&history)?;

                Ok(())
            }

            Self::All => {
                let history = db.list(FilterMode::Global, &context, None, false).await?;

                compute_stats(&history)?;

                Ok(())
            }
        }
    }
}
