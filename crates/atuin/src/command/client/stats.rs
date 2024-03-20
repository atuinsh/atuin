use clap::Parser;
use eyre::Result;
use interim::parse_date_string;
use time::{Duration, OffsetDateTime, Time};

use atuin_client::{
    database::{current_context, Database},
    settings::Settings,
    theme::Theme,
};

use atuin_history::stats::{compute, pretty_print};

#[derive(Parser, Debug)]
#[command(infer_subcommands = true)]
pub struct Cmd {
    /// Compute statistics for the specified period, leave blank for statistics since the beginning. See [this](https://docs.atuin.sh/reference/stats/) for more details.
    period: Vec<String>,

    /// How many top commands to list
    #[arg(long, short, default_value = "10")]
    count: usize,

    /// The number of consecutive commands to consider
    #[arg(long, short, default_value = "1")]
    ngram_size: usize,
}

impl Cmd {
    pub async fn run(&self, db: &impl Database, settings: &Settings, theme: &Theme) -> Result<()> {
        let context = current_context();
        let words = if self.period.is_empty() {
            String::from("all")
        } else {
            self.period.join(" ")
        };

        let now = OffsetDateTime::now_utc().to_offset(settings.timezone.0);
        let last_night = now.replace_time(Time::MIDNIGHT);

        let history = if words.as_str() == "all" {
            db.list(&[], &context, None, false, false).await?
        } else if words.trim() == "today" {
            let start = last_night;
            let end = start + Duration::days(1);
            db.range(start, end).await?
        } else if words.trim() == "month" {
            let end = last_night;
            let start = end - Duration::days(31);
            db.range(start, end).await?
        } else if words.trim() == "week" {
            let end = last_night;
            let start = end - Duration::days(7);
            db.range(start, end).await?
        } else if words.trim() == "year" {
            let end = last_night;
            let start = end - Duration::days(365);
            db.range(start, end).await?
        } else {
            let start = parse_date_string(&words, now, settings.dialect.into())?;
            let end = start + Duration::days(1);
            db.range(start, end).await?
        };

        let stats = compute(settings, &history, self.count, self.ngram_size);

        if let Some(stats) = stats {
            pretty_print(stats, self.ngram_size, theme);
        }

        Ok(())
    }
}
