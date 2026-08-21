//! Daemon search benchmark.
//!
//! Measures the daemon's `SearchIndex::search` end-to-end — index built from
//! shell-history-shaped data, then queried the way the interactive UI does —
//! using divan, so results are tracked on CodSpeed (the `divan` dependency is
//! CodSpeed's drop-in compat crate; it behaves as plain divan outside
//! `cargo-codspeed` builds).
//!
//! Benchmark names (`daemon_search[<scale>/<query>]`) are deliberately
//! engine-agnostic: they measure whatever fuzzy matcher the daemon currently
//! uses, so an engine swap shows up as a step change in the tracked series
//! rather than as a new set of benchmarks.
//!
//! Configuration (env vars):
//! - `BENCH_SCALES`: comma-separated history line counts (default
//!   `10000,100000,1000000`); the index deduplicates, so unique command
//!   counts are lower
//! - `BENCH_DATA`: optional file of 0x1E-separated real commands, e.g.
//!   `sqlite3 history.db ".mode ascii" "select command from history"`;
//!   cycled up to the largest scale. Without it a deterministic synthetic
//!   corpus is generated (see `src/corpus.rs`).
//! - `BENCH_SEED`: seed for the synthetic corpus (default 42)
//!
//! Sampling is divan's own; tune with `DIVAN_MAX_TIME`, `DIVAN_SAMPLE_COUNT`,
//! etc. if needed.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

use atuin_client::history::History;
use atuin_client::settings::Search as SearchSettings;
use atuin_common::filter::OrFilter;
use atuin_common::path::DisplayRichExt;
use atuin_daemon::search::{IndexFilterMode, SearchIndex};
use atuin_search_bench::corpus;
use parking_lot::Mutex;
use time::OffsetDateTime;

fn main() {
    divan::main();
}

/// Queries the way users type them: empty (frecency-only listing), short
/// low-selectivity prefixes, multi-word, and a no-match worst case.
const QUERIES: &[&str] =
    &["", "g", "git", "git p", "cargo build", "docker compose up", "zzznomatchzzz"];

/// The interactive UI requests up to 200 results per query.
const LIMIT: u32 = 200;

/// Working directories assigned round-robin to history entries, so the
/// directory-filtered benchmark has a realistic candidate subset.
const DIRS: &[&str] = &[
    "/home/user/src/atuin",
    "/home/user/src/backend",
    "/home/user/src/frontend",
    "/home/user/dotfiles",
    "/home/user",
    "/tmp/scratch",
    "/var/log",
    "/opt/deploy",
];

struct Case {
    scale: usize,
    query: &'static str,
    directory_filter: bool,
}

impl fmt::Display for Case {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let query = if self.query.is_empty() {
            "empty"
        } else {
            self.query
        };
        if self.directory_filter {
            write!(f, "{}/{query} (directory)", self.scale)
        } else {
            write!(f, "{}/{query}", self.scale)
        }
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn scales() -> &'static Vec<usize> {
    static SCALES: OnceLock<Vec<usize>> = OnceLock::new();
    SCALES.get_or_init(|| {
        let raw = std::env::var("BENCH_SCALES").unwrap_or_else(|_| "10000,100000,1000000".into());
        let mut scales = Vec::new();
        for part in raw.split(',') {
            match part.trim().parse::<usize>() {
                Ok(n) if n > 0 => scales.push(n),
                _ => {
                    eprintln!("error: BENCH_SCALES entry {part:?} is not a positive line count");
                    std::process::exit(1);
                }
            }
        }
        scales
    })
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for &scale in scales() {
        for &query in QUERIES {
            cases.push(Case {
                scale,
                query,
                directory_filter: false,
            });
        }
        cases.push(Case {
            scale,
            query: "git",
            directory_filter: true,
        });
    }
    cases
}

fn commands() -> &'static Vec<String> {
    static COMMANDS: OnceLock<Vec<String>> = OnceLock::new();
    COMMANDS.get_or_init(|| {
        let max_scale = *scales().iter().max().expect("validated non-empty");
        if let Ok(path) = std::env::var("BENCH_DATA") {
            let raw = std::fs::read(&path).expect("failed to read BENCH_DATA");
            let mut lines: Vec<String> = raw
                .split(|&b| b == 0x1E)
                .filter(|r| !r.is_empty())
                .map(|r| String::from_utf8_lossy(r).into_owned())
                .collect();
            if lines.is_empty() {
                eprintln!("error: BENCH_DATA file {path:?} contains no commands");
                std::process::exit(1);
            }
            eprintln!("corpus: real history ({path}, {} lines, cycled)", lines.len());
            // repeated commands are the norm in raw history, so cycling stays
            // representative
            let mut i = 0;
            while lines.len() < max_scale {
                lines.push(lines[i].clone());
                i += 1;
            }
            lines
        } else {
            let seed = env_usize("BENCH_SEED", 42) as u64;
            eprintln!("corpus: generating {max_scale} synthetic history lines (seed {seed})...");
            corpus::generate(max_scale, seed)
        }
    })
}

fn filter_dir() -> String {
    DIRS[0].display_rich().trailing_slash(true).to_string()
}

/// Indexes are built lazily and shared across the queries of a scale. Build
/// happens outside the timed sections; result counts are sanity-checked so a
/// broken query or filter shows up in the logs rather than as a suspiciously
/// fast run.
fn index(scale: usize) -> Arc<SearchIndex> {
    static INDEXES: OnceLock<Mutex<HashMap<usize, Arc<SearchIndex>>>> = OnceLock::new();
    let mut indexes = INDEXES.get_or_init(|| Mutex::new(HashMap::new())).lock();
    if let Some(index) = indexes.get(&scale) {
        return Arc::clone(index);
    }

    eprintln!("building index from {scale} history lines...");
    let index = SearchIndex::new(OrFilter::all());
    let now = OffsetDateTime::now_utc();
    for (i, command) in commands()[..scale].iter().enumerate() {
        let history: History = History::import()
            .timestamp(now - time::Duration::seconds(((i * 37) % 31_536_000) as i64))
            .command(command.as_str())
            .cwd(DIRS[i % DIRS.len()])
            .build()
            .into();
        index.add_history(&history);
    }
    index.rebuild_frecency(&SearchSettings::default());
    eprintln!("index ready: {} unique commands", index.command_count());

    for query in QUERIES {
        let n = index.search(query, &IndexFilterMode::Global, LIMIT).count();
        eprintln!("  {query:?}: {n} results (limit {LIMIT})");
    }
    let dir = filter_dir();
    let n = index.search("git", &IndexFilterMode::Directory(dir.clone()), LIMIT).count();
    eprintln!("  \"git\" in {dir:?}: {n} results (limit {LIMIT})");
    assert!(n > 0, "directory filter matched nothing; filter is broken");

    let index = Arc::new(index);
    indexes.insert(scale, Arc::clone(&index));
    index
}

#[divan::bench(args = cases())]
fn daemon_search(bencher: divan::Bencher, case: &Case) {
    let index = index(case.scale);
    let filter = if case.directory_filter {
        IndexFilterMode::Directory(filter_dir())
    } else {
        IndexFilterMode::Global
    };
    bencher.bench(|| index.search(case.query, &filter, LIMIT).count());
}
