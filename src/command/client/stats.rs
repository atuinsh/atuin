use std::collections::HashMap;

use chrono::{prelude::*, Duration};
use clap::Parser;
use crossterm::style::{Color, ResetColor, SetAttribute, SetForegroundColor};
use eyre::{bail, Result};
use interim::parse_date_string;

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

    /// How many top commands to list
    #[arg(long, short, default_value = "10")]
    count: usize,
}

fn compute_stats(history: &[History], count: usize) -> Result<()> {
    let mut commands = HashMap::<&str, usize>::new();
    for i in history {
        let command = i.command.split_ascii_whitespace().next();

        if command.is_none() {
            continue;
        }

        *commands.entry(command.unwrap()).or_default() += 1;
    }
    let unique = commands.len();
    let mut top = commands.into_iter().collect::<Vec<_>>();
    top.sort_unstable_by_key(|x| std::cmp::Reverse(x.1));
    top.truncate(count);
    if top.is_empty() {
        bail!("No commands found");
    }

    let max = top.iter().map(|x| x.1).max().unwrap();
    let num_pad = max.ilog10() as usize + 1;

    for (command, count) in top {
        let gray = SetForegroundColor(Color::Grey);
        let bold = SetAttribute(crossterm::style::Attribute::Bold);

        let in_ten = 10 * count / max;
        print!("[");
        print!("{}", SetForegroundColor(Color::Red));
        for i in 0..in_ten {
            if i == 2 {
                print!("{}", SetForegroundColor(Color::Yellow));
            }
            if i == 5 {
                print!("{}", SetForegroundColor(Color::Green));
            }
            print!("▮");
        }
        for _ in in_ten..10 {
            print!(" ");
        }

        println!("{ResetColor}] {gray}{count:num_pad$}{ResetColor} {bold}{command}{ResetColor}");
    }
    println!("Total commands:   {}", history.len());
    println!("Unique commands:  {unique}");

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
        compute_stats(&history, self.count)?;
        Ok(())
    }
}
