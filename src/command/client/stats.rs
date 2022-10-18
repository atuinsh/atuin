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
#[command(infer_subcommands = true)]
pub struct Cmd {
    /// compute statistics for the specified period, leave blank for statistics since the beginning
    period: Vec<String>,
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
    pub async fn run(&self, db: &mut impl Database, settings: &Settings) -> Result<()> {
        let context = current_context();
        let words = if self.period.is_empty() {
            String::from("all")
        } else {
            self.period.join(" ")
        };
        let history = if words.as_str() == "all" {
            db.list(FilterMode::Global, &context, None, false).await?
        } else {
            let start = parse_date_string(&words, Local::now(), settings.dialect.into())?;
            let end = start + Duration::days(1);
            db.range(start.into(), end.into()).await?
        };
        compute_stats(&history)?;
        Ok(())
    }
}
