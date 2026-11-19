//! `atuin trick-or-treat`: a forgotten command, or a ghost from the command graveyard, in the
//! Halloween season.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Write as _};

use atuin_client::database::Sqlite;
use atuin_client::history::{CommandRuns, HistoryImported, all_user_author_filter};
use atuin_client::settings::Settings;
use atuin_common::string::align::Alignment;
use atuin_common::string::ellipsis::{Indicator, Pos};
use atuin_common::string::{
    AlignExt as _, EllipsizeExt as _, EscapeNonPrintablePosixExt as _, Measure,
};
use atuin_history::stats::{interesting_command, split_at_pipe, strip_leading_env_vars};
use clap::Args;
use eyre::Result;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ZonedDateTime};
use rand::Rng as _;
use rand::seq::SliceRandom as _;
use unicode_width::UnicodeWidthStr as _;

use super::halloween::HalloweenExt as _;
use crate::i18n::fl;

#[derive(Args, Debug)]
pub struct Cmd;

impl Cmd {
    pub async fn run(&self, db: &Sqlite, settings: &Settings) -> Result<()> {
        let now = Timestamp::now().to_zoned_datetime(settings.timezone.time_zone());
        if !now.date().is_halloween_season() {
            writeln!(io::stdout(), "{}", fl!("trick-or-treat-closed"))?;
            return Ok(());
        }

        // An agent's commands are neither the user's treats nor what keeps the user's ghosts alive.
        let runs = db.command_runs(all_user_author_filter()).await?;
        let ghosts = Ghost::graveyard(settings, &runs, &now);
        let treats = Treat::bag(&runs, &now);

        let mut rng = rand::thread_rng();
        let mut stdout = io::stdout().lock();
        // The coin picks a side; an empty side hands over to the other.
        match (rng.gen_bool(0.5), ghosts.choose(&mut rng), treats.choose(&mut rng)) {
            (true, Some(ghost), _) | (false, Some(ghost), None) => {
                writeln!(stdout, "{}\n\n{ghost}", fl!("trick-or-treat-trick"))?;
            }
            (_, _, Some(treat)) => writeln!(stdout, "{treat}")?,
            (_, None, None) => writeln!(stdout, "{}", fl!("trick-or-treat-empty"))?,
        }

        Ok(())
    }
}

/// A command the user ran often, then never again. Displays as its tombstone.
#[derive(Debug, PartialEq, Eq)]
struct Ghost<'a> {
    name: &'a str,
    runs: u64,
    first: Timestamp,
    last: Timestamp,
    /// The zone the tombstone reads its years in.
    tz: TimeZone,
}

impl<'a> Ghost<'a> {
    const MIN_RUNS: u64 = 20;
    const MIN_SILENCE: SignedDuration = SignedDuration::from_hours(24 * 7);
    const MAX_SILENCE: SignedDuration = SignedDuration::from_hours(24 * 365);
    /// Longer names are cut so the tombstone fits a narrow terminal.
    const MAX_NAME_COLUMNS: usize = 24;

    /// The commands in `runs` that ran at least [`Self::MIN_RUNS`] times, then went quiet before
    /// `now` for a third of the history's age, kept between [`Self::MIN_SILENCE`] and
    /// [`Self::MAX_SILENCE`] so a young history has ghosts too.
    ///
    /// Commands are grouped the way `atuin stats` groups them, so `svn up` and `sudo svn log` are
    /// both `svn`, and one recent `svn log` keeps it alive. Every program in a pipeline counts, so
    /// `cat log | grep error` keeps `grep` alive too.
    fn graveyard(settings: &Settings, runs: &'a [CommandRuns], now: &ZonedDateTime) -> Vec<Self> {
        let (now, tz) = (now.timestamp(), now.time_zone());
        let age = runs
            .iter()
            .map(|run| run.first)
            .min()
            .map_or(SignedDuration::ZERO, |first| now - first);
        let silence = (age / 3_i32).clamp(Self::MIN_SILENCE, Self::MAX_SILENCE);

        let mut ghosts = HashMap::<&str, Self>::new();
        for run in runs {
            let mut names: Vec<_> = split_at_pipe(run.command.trim())
                .into_iter()
                .map(|segment| {
                    interesting_command(settings, strip_leading_env_vars(segment.trim()))
                })
                .filter(|name| {
                    !name.is_empty() && !settings.stats.ignored_commands.iter().any(|c| c == *name)
                })
                .collect();
            // A program piped into itself (`grep a | grep b`) ran once per run, not twice.
            names.sort_unstable();
            names.dedup();

            for name in names {
                let ghost = ghosts.entry(name).or_insert(Self {
                    name,
                    runs: 0,
                    first: run.first,
                    last: run.last,
                    tz: tz.clone(),
                });
                ghost.runs += u64::from(run.runs);
                ghost.first = ghost.first.min(run.first);
                ghost.last = ghost.last.max(run.last);
            }
        }

        ghosts
            .into_values()
            .filter(|ghost| ghost.runs >= Self::MIN_RUNS && now - ghost.last >= silence)
            .collect()
    }
}

impl fmt::Display for Ghost<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (born, died) =
            (self.tz.to_datetime(self.first).year(), self.tz.to_datetime(self.last).year());
        let name = self.name.escape_non_printable();
        let epitaph = [
            fl!("trick-or-treat-rip"),
            String::new(),
            name.ellipsize(Measure::Columns(Self::MAX_NAME_COLUMNS), Pos::End, Indicator::UNICODE)
                .to_string(),
            if born == died {
                died.to_string()
            } else {
                format!("{born} - {died}")
            },
            fl!("trick-or-treat-runs", runs = self.runs),
            String::new(),
        ];
        // Two columns of stone either side of the widest line.
        let width = epitaph.iter().map(|line| line.width()).max().unwrap_or_default() + 4;

        writeln!(f, "   {}", "_".repeat(width - 2))?;
        writeln!(f, "  /{}\\", " ".repeat(width - 2))?;
        for line in &epitaph {
            writeln!(f, " |{}|", line.pad_to(Measure::Columns(width), Alignment::Center))?;
        }
        write!(f, "_|{}|_", "_".repeat(width))
    }
}

/// A long command that worked, ran once or twice, and hasn't come back since.
#[derive(Debug, PartialEq, Eq)]
struct Treat<'a> {
    command: &'a str,
    runs: u32,
    last: Timestamp,
    /// The zone the treat reads its date in.
    tz: TimeZone,
    /// Where it ran, if history knows.
    cwd: Option<&'a str>,
}

impl<'a> Treat<'a> {
    const MAX_RUNS: u32 = 2;
    /// Shorter commands are easy enough to remember, or to retype.
    const MIN_CHARS: usize = 40;
    const MIN_SILENCE: SignedDuration = SignedDuration::from_hours(24 * 3);
    const MAX_SILENCE: SignedDuration = SignedDuration::from_hours(24 * 90);

    /// The commands in `runs` worth digging up at `now`: quiet for a sixth of the history's age,
    /// kept between [`Self::MIN_SILENCE`] and [`Self::MAX_SILENCE`].
    fn bag(runs: &'a [CommandRuns], now: &ZonedDateTime) -> Vec<Self> {
        let (now, tz) = (now.timestamp(), now.time_zone());
        let age = runs
            .iter()
            .map(|run| run.first)
            .min()
            .map_or(SignedDuration::ZERO, |first| now - first);
        let silence = (age / 6_i32).clamp(Self::MIN_SILENCE, Self::MAX_SILENCE);

        runs.iter()
            .filter(|run| {
                run.runs <= Self::MAX_RUNS
                    && run.failures == 0
                    && run.command.trim().chars().count() >= Self::MIN_CHARS
                    && now - run.last >= silence
            })
            .map(|run| Self {
                command: run.command.trim(),
                runs: run.runs,
                last: run.last,
                tz: tz.clone(),
                cwd: Some(run.cwd.as_str()).filter(|cwd| *cwd != HistoryImported::UNKNOWN_CWD),
            })
            .collect()
    }
}

impl fmt::Display for Treat<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let date = self.tz.to_datetime(self.last).date().to_string();
        let heading = match self.cwd {
            Some(cwd) => {
                let cwd = cwd.escape_non_printable().into_owned();
                fl!("trick-or-treat-treat", runs = self.runs, date = date, cwd = cwd)
            }
            None => fl!("trick-or-treat-treat-nowhere", runs = self.runs, date = date),
        };
        writeln!(f, "{heading}\n")?;
        write!(f, "    {}", self.command.escape_non_printable())
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::history::CommandRuns;
    use atuin_client::settings::Settings;
    use jiff::tz::TimeZone;
    use jiff::{Timestamp, ZonedDateTime};
    use rstest::{fixture, rstest};

    use super::{Ghost, Treat};

    /// `2026-10-31T12:00:00Z`.
    const NOW: Timestamp = Timestamp::constant(1_793_448_000, 0);
    /// `2023-06-01T00:00:00Z`.
    const LONG_AGO: Timestamp = Timestamp::constant(1_685_577_600, 0);
    /// `2026-10-24T00:00:00Z`.
    const LAST_WEEK: Timestamp = Timestamp::constant(1_792_800_000, 0);
    /// `2026-10-10T00:00:00Z`.
    const OCTOBER_10: Timestamp = Timestamp::constant(1_791_590_400, 0);
    /// `2026-08-03T00:00:00Z`, where a young history starts.
    const AUGUST: Timestamp = Timestamp::constant(1_785_715_200, 0);
    const LONG: &str = "ffmpeg -i in.mov -vf \"fps=10,scale=640:-1\" out.gif";

    /// [`NOW`] in UTC.
    #[fixture]
    fn now() -> ZonedDateTime {
        NOW.to_zoned_datetime(TimeZone::UTC)
    }

    /// `command`, run `runs` times without failing, the last time at `last`.
    fn ran(command: &str, runs: u32, last: Timestamp) -> CommandRuns {
        CommandRuns {
            command: command.to_owned(),
            runs,
            failures: 0,
            first: "2019-03-01T00:00:00Z".parse::<Timestamp>().unwrap(),
            last,
            cwd: "/srv".to_owned(),
        }
    }

    #[rstest]
    #[case::long_dead(vec![ran("svn up", 30, LONG_AGO)], &["svn"])]
    #[case::runs_add_up_across_a_family(
        vec![ran("svn up", 10, LONG_AGO), ran("svn commit -m wip", 10, LONG_AGO)],
        &["svn"]
    )]
    #[case::too_few_runs(vec![ran("svn up", 19, LONG_AGO)], &[])]
    #[case::kept_alive_by_a_sibling(
        vec![ran("svn up", 30, LONG_AGO), ran("svn log", 1, LAST_WEEK)],
        &[]
    )]
    #[case::not_quite_a_year(vec![ran("svn up", 30, "2025-11-01T00:00:00Z".parse::<Timestamp>().unwrap())], &[])]
    #[case::prefix_and_env_dropped(
        vec![ran("sudo svn up", 10, LONG_AGO), ran("LANG=C svn up", 10, LONG_AGO)],
        &["svn"]
    )]
    #[case::subcommands_haunt_alone(
        vec![ran("cargo fuzz run", 30, LONG_AGO), ran("cargo build", 1, LAST_WEEK)],
        &["cargo fuzz"]
    )]
    #[case::piped_use_keeps_a_program_alive(
        vec![ran("grep -r foo .", 30, LONG_AGO), ran("cat log | grep error", 1, LAST_WEEK)],
        &[]
    )]
    #[case::every_program_in_a_pipeline_haunts(
        vec![ran("cat a.json | jq .", 20, LONG_AGO)],
        &["cat", "jq"]
    )]
    #[case::stderr_pipe_operator_is_no_ghost(
        vec![ran("make |& tee build.log", 20, LONG_AGO)],
        &["make", "tee"]
    )]
    #[case::piped_into_itself_counts_once(vec![ran("grep a | grep b", 10, LONG_AGO)], &[])]
    #[case::a_young_history_buries_sooner(
        vec![CommandRuns { first: AUGUST, ..ran("cargo asm", 31, "2026-09-15T00:00:00Z".parse::<Timestamp>().unwrap()) }],
        &["cargo asm"]
    )]
    #[case::a_new_history_waits_a_week(
        vec![CommandRuns {
            first: "2026-10-24T00:00:00Z".parse::<Timestamp>().unwrap(),
            ..ran("cargo asm", 31, "2026-10-27T00:00:00Z".parse::<Timestamp>().unwrap())
        }],
        &[]
    )]
    fn graveyard(now: ZonedDateTime, #[case] runs: Vec<CommandRuns>, #[case] expected: &[&str]) {
        let settings = Settings::utc();
        let mut names: Vec<_> =
            Ghost::graveyard(&settings, &runs, &now).into_iter().map(|ghost| ghost.name).collect();
        names.sort_unstable();
        assert_eq!(names, expected);
    }

    #[rstest]
    fn tombstone_centres_the_epitaph() {
        let ghost = Ghost {
            name: "svn",
            runs: 412,
            first: "2019-03-01T00:00:00Z".parse::<Timestamp>().unwrap(),
            last: LONG_AGO,
            tz: TimeZone::UTC,
        };
        assert_eq!(
            ghost.to_string(),
            [
                "   _____________",
                "  /             \\",
                " |    R.I.P.     |",
                " |               |",
                " |      svn      |",
                " |  2019 - 2023  |",
                " |   412 runs    |",
                " |               |",
                "_|_______________|_",
            ]
            .join("\n")
        );
    }

    #[rstest]
    #[case::forgotten(vec![ran(LONG, 1, LONG_AGO)], true)]
    #[case::twice(vec![ran(LONG, 2, LONG_AGO)], true)]
    #[case::three_times_is_a_habit(vec![ran(LONG, 3, LONG_AGO)], false)]
    #[case::too_short(vec![ran("ls -la", 1, LONG_AGO)], false)]
    #[case::too_recent(vec![ran(LONG, 1, "2026-09-01T00:00:00Z".parse::<Timestamp>().unwrap())], false)]
    #[case::failed(vec![CommandRuns { failures: 1, ..ran(LONG, 1, LONG_AGO) }], false)]
    #[case::a_young_history_digs_sooner(
        vec![
            CommandRuns { first: AUGUST, ..ran("ls", 100, LAST_WEEK) },
            CommandRuns { first: OCTOBER_10, ..ran(LONG, 1, OCTOBER_10) },
        ],
        true
    )]
    #[case::a_new_history_waits_three_days(
        vec![
            CommandRuns { first: "2026-10-30T00:00:00Z".parse::<Timestamp>().unwrap(), ..ran("ls", 100, NOW) },
            CommandRuns {
                first: "2026-10-30T00:00:00Z".parse::<Timestamp>().unwrap(),
                ..ran(LONG, 1, "2026-10-30T00:00:00Z".parse::<Timestamp>().unwrap())
            },
        ],
        false
    )]
    fn treats(now: ZonedDateTime, #[case] runs: Vec<CommandRuns>, #[case] treat: bool) {
        assert_eq!(Treat::bag(&runs, &now).iter().any(|t| t.command == LONG), treat);
    }

    #[rstest]
    #[case::once(
        ran(LONG, 1, LONG_AGO),
        "🍬 Treat! You ran this once, on 2023-06-01 in /srv, and never again:"
    )]
    #[case::twice(
        ran(LONG, 2, LONG_AGO),
        "🍬 Treat! You ran this twice, last on 2023-06-01 in /srv, and never again:"
    )]
    #[case::imported_without_a_directory(
        CommandRuns { cwd: "unknown".to_owned(), ..ran(LONG, 1, LONG_AGO) },
        "🍬 Treat! You ran this once, on 2023-06-01, and never again:"
    )]
    fn treat_says_how_often_and_where_it_ran(
        now: ZonedDateTime,
        #[case] runs: CommandRuns,
        #[case] heading: &str,
    ) {
        let treats: Vec<_> =
            Treat::bag(std::slice::from_ref(&runs), &now).iter().map(ToString::to_string).collect();
        assert_eq!(treats, [format!("{heading}\n\n    {LONG}")]);
    }
}
