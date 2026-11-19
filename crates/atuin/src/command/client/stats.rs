use std::io::{self, Write as _};

use atuin_client::database::{Sqlite, current_context};
use atuin_client::settings::{Dialect, FilterMode, Settings};
use atuin_client::theme::Theme;
use atuin_history::stats::{compute, is_ignored, pretty_print};
use clap::Parser;
use eyre::Result;
use jiff::civil::Date;
use jiff::{Timestamp, ToSpan as _, ZonedDateTime};
use tracing::instrument;

use super::halloween::HalloweenExt as _;
use crate::i18n::fl;

fn parse_ngram_size(s: &str) -> Result<usize, String> {
    let value = s.parse::<usize>().map_err(|_| format!("'{s}' is not a valid window size"))?;

    if value == 0 {
        return Err("ngram window size must be at least 1".to_string());
    }

    Ok(value)
}

#[derive(Parser, Debug)]
#[command(infer_subcommands = true)]
pub struct Cmd {
    #[arg(
        help = fl!("arg-stats-period"),
        long_help = fl!("arg-stats-period", "long", url = atuin_common::docs::url("reference/stats/"))
    )]
    period: Vec<String>,

    #[arg(long, short, default_value = "10", help = fl!("arg-stats-count"))]
    count: usize,

    #[arg(
        long,
        short,
        default_value = "1",
        value_parser = parse_ngram_size,
        help = fl!("arg-stats-ngram-size")
    )]
    ngram_size: usize,

    #[arg(long = "filter-mode", help = fl!("arg-stats-filter-mode"))]
    filter_mode: Option<FilterMode>,
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(&self, db: &Sqlite, settings: &Settings, theme: &Theme) -> Result<()> {
        let context = current_context(settings).await?;
        let words = if self.period.is_empty() {
            String::from("all")
        } else {
            self.period.join(" ")
        };

        // A single filter mode, or none. `list` takes an iterator of modes so it can
        // OR several, but stats only ever scopes to one at a time.
        let filter = self.filter_mode.map(|f| vec![f]).unwrap_or_default();

        let now = Timestamp::now().to_zoned_datetime(settings.timezone.time_zone());
        let range = Self::range(&words, &now, settings.dialect)?;

        let history = db.list(filter, &context, None, false, false, range).await?;

        let stats = compute(settings, &history, self.count, self.ngram_size);

        if let Some(stats) = stats {
            pretty_print(stats, self.ngram_size, theme);
        }

        if now.date().is_halloween_season() {
            let witching = history
                .iter()
                .filter(|h| {
                    now.time_zone().to_datetime(h.timestamp).hour() == 3
                        && !is_ignored(settings, &h.command)
                })
                .count();
            if witching > 0 {
                writeln!(io::stdout(), "{}", fl!("stats-witching-hour", count = witching))?;
            }
        }

        Ok(())
    }

    /// The instants `period` covers, as seen at `now`; `None` for all of history.
    ///
    /// A day runs from its own start, which DST can move off midnight, to the next day's.
    fn range(
        period: &str,
        now: &ZonedDateTime,
        dialect: Dialect,
    ) -> Result<Option<(Timestamp, Timestamp)>> {
        let tz = now.time_zone();
        let today = now.date();
        let start_of =
            |day: Date| -> Result<Timestamp> { Ok(day.to_zoned_datetime(tz.clone())?.timestamp()) };
        let days_before_today = |days: i64| -> Result<_> {
            Ok((start_of(today.checked_sub(days.days())?)?, start_of(today)?))
        };

        let range = match period.trim() {
            "all" => return Ok(None),
            "today" => (start_of(today)?, start_of(today.tomorrow()?)?),
            "month" => days_before_today(31)?,
            "week" => days_before_today(7)?,
            "year" => days_before_today(365)?,
            _ => {
                let start = dialect.parse_date(period, now)?;
                // A date names its whole day; a time, the day from that time.
                let end = if start == start.start_of_day()? {
                    start_of(start.date().tomorrow()?)?
                } else {
                    start.checked_add(1.day())?.timestamp()
                };
                (start.timestamp(), end)
            }
        };
        Ok(Some(range))
    }
}

#[cfg(test)]
mod tests {
    use jiff::{Timestamp, ZonedDateTime};
    use rstest::rstest;

    use super::{Cmd, Dialect};

    /// America/Santiago skips from 00:00 to 01:00 on 2026-09-06, so that day starts at 01:00.
    #[rstest]
    #[case::week_into_a_short_day(
        "2026-09-06T12:00:00-03:00[America/Santiago]",
        "week",
        Some(("2026-08-30T00:00:00-04:00", "2026-09-06T01:00:00-03:00"))
    )]
    #[case::today_starting_after_a_gap(
        "2026-09-06T12:00:00-03:00[America/Santiago]",
        "today",
        Some(("2026-09-06T01:00:00-03:00", "2026-09-07T00:00:00-03:00"))
    )]
    #[case::a_date_starting_after_a_gap(
        "2026-09-10T12:00:00-03:00[America/Santiago]",
        "2026-09-06",
        Some(("2026-09-06T01:00:00-03:00", "2026-09-07T00:00:00-03:00"))
    )]
    #[case::a_time_names_the_day_from_it(
        "2026-09-10T12:00:00-03:00[America/Santiago]",
        "2026-09-08 15:00",
        Some(("2026-09-08T15:00:00-03:00", "2026-09-09T15:00:00-03:00"))
    )]
    #[case::all("2026-09-10T12:00:00-03:00[America/Santiago]", "all", None)]
    fn range_runs_from_the_start_of_each_day(
        #[case] now: &str,
        #[case] period: &str,
        #[case] expected: Option<(&str, &str)>,
    ) {
        let now: ZonedDateTime = now.parse().unwrap();
        let expected = expected.map(|(start, end)| {
            (start.parse::<Timestamp>().unwrap(), end.parse::<Timestamp>().unwrap())
        });
        assert_eq!(Cmd::range(period, &now, Dialect::Uk).unwrap(), expected);
    }
}
