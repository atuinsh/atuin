use clap::Parser;
use eyre::Result;
use interim::parse_date_string;
use time::{Duration, OffsetDateTime, Time};

use atuin_client::{
    database::{Database, current_context},
    settings::{FilterMode, Settings},
    theme::Theme,
};

use atuin_history::stats::{compute, pretty_print};

fn parse_ngram_size(s: &str) -> Result<usize, String> {
    let value = s
        .parse::<usize>()
        .map_err(|_| format!("'{s}' is not a valid window size"))?;

    if value == 0 {
        return Err("ngram window size must be at least 1".to_string());
    }

    Ok(value)
}

#[derive(Parser, Debug)]
#[command(infer_subcommands = true)]
pub struct Cmd {
    /// Compute statistics for the specified period, leave blank for statistics since the beginning. See [this](https://docs.atuin.sh/reference/stats/) for more details.
    period: Vec<String>,

    /// How many top commands to list
    #[arg(long, short, default_value = "10")]
    count: usize,

    /// The number of consecutive commands to consider
    #[arg(long, short, default_value = "1", value_parser = parse_ngram_size)]
    ngram_size: usize,

    /// Filter commands by scope [global, host, session, directory, workspace]
    #[arg(long = "filter-mode")]
    filter_mode: Option<FilterMode>,
}

impl Cmd {
    pub async fn run(&self, db: &impl Database, settings: &Settings, theme: &Theme) -> Result<()> {
        let context = current_context().await?;
        let words = if self.period.is_empty() {
            String::from("all")
        } else {
            self.period.join(" ")
        };

        // A single filter mode, or none. `list` takes a slice so it can OR several,
        // but stats only ever scopes to one at a time.
        let filter = self.filter_mode.map(|f| vec![f]).unwrap_or_default();

        let now = OffsetDateTime::now_utc().to_offset(settings.timezone.0);
        let last_night = now.replace_time(Time::MIDNIGHT);

        let range = if words.as_str() == "all" {
            None
        } else if words.trim() == "today" {
            let start = last_night;
            let end = start + Duration::days(1);
            Some((start, end))
        } else if words.trim() == "month" {
            let end = last_night;
            let start = end - Duration::days(31);
            Some((start, end))
        } else if words.trim() == "week" {
            let end = last_night;
            let start = end - Duration::days(7);
            Some((start, end))
        } else if words.trim() == "year" {
            let end = last_night;
            let start = end - Duration::days(365);
            Some((start, end))
        } else {
            let start = parse_date_string(&words, now, settings.dialect.into())?;
            let end = start + Duration::days(1);
            Some((start, end))
        };

        let history = db
            .list(filter.as_slice(), &context, None, false, false, range)
            .await?;

        let stats = compute(settings, &history, self.count, self.ngram_size);

        if let Some(stats) = stats {
            pretty_print(stats, self.ngram_size, theme);
        }

        Ok(())
    }
}
