use std::path::Path;

use async_trait::async_trait;
use atuin_client::{database::Database, history::History, settings::FilterMode};
use eyre::Result;
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use itertools::Itertools;
use time::OffsetDateTime;
use tokio::task::yield_now;
use tracing::{Level, instrument, warn};
use uuid;

use super::{SearchEngine, SearchState};

pub struct Search {
    all_history: Vec<(History, i32)>,
    engine: SkimMatcherV2,
}

impl Search {
    pub fn new() -> Self {
        Search {
            all_history: vec![],
            engine: SkimMatcherV2::default(),
        }
    }
}

#[async_trait]
impl SearchEngine for Search {
    #[instrument(skip_all, level = Level::TRACE, name = "skim_search", fields(query = %state.input.as_str()))]
    async fn full_query(
        &mut self,
        state: &SearchState,
        db: &mut dyn Database,
    ) -> Result<Vec<History>> {
        if self.all_history.is_empty() {
            self.all_history = load_all_history(db).await;
        }

        Ok(fuzzy_search(&self.engine, state, &self.all_history).await)
    }

    #[instrument(skip_all, level = Level::TRACE, name = "skim_highlight")]
    fn get_highlight_indices(&self, command: &str, search_input: &str) -> Vec<usize> {
        let (_, indices) = self
            .engine
            .fuzzy_indices(command, search_input)
            .unwrap_or_default();
        indices
    }
}

#[instrument(skip_all, level = Level::TRACE, name = "load_all_history")]
async fn load_all_history(db: &dyn Database) -> Vec<(History, i32)> {
    db.all_with_count().await.unwrap()
}

#[allow(clippy::too_many_lines)]
#[instrument(skip_all, level = Level::TRACE, name = "fuzzy_match", fields(history_count = all_history.len()))]
async fn fuzzy_search(
    engine: &SkimMatcherV2,
    state: &SearchState,
    all_history: &[(History, i32)],
) -> Vec<History> {
    let mut set = Vec::with_capacity(200);
    let mut ranks = Vec::with_capacity(200);
    let query = state.input.as_str();
    let now = OffsetDateTime::now_utc();

    for (i, (history, count)) in all_history.iter().enumerate() {
        if i % 256 == 0 {
            yield_now().await;
        }
        let context = &state.context;
        let git_root = context
            .git_root
            .as_ref()
            .and_then(|git_root| git_root.to_str())
            .unwrap_or(&context.cwd);
        match state.filter_mode {
            FilterMode::Global => {}
            // we aggregate host by ',' separating them
            FilterMode::Host
                if history
                    .hostname
                    .split(',')
                    .contains(&context.hostname.as_str()) => {}
            // we aggregate session by concattenating them.
            // sessions are 32 byte simple uuid formats
            FilterMode::Session
                if history
                    .session
                    .as_bytes()
                    .chunks(32)
                    .contains(&context.session.as_bytes()) => {}
            // SessionPreload: include current session + global history from before session start
            FilterMode::SessionPreload => {
                let is_current_session = {
                    history
                        .session
                        .as_bytes()
                        .chunks(32)
                        .any(|chunk| chunk == context.session.as_bytes())
                };

                if !is_current_session {
                    let Ok(uuid) = uuid::Uuid::parse_str(&context.session) else {
                        warn!("failed to parse session id '{}'", context.session);
                        continue;
                    };
                    let Some(timestamp) = uuid.get_timestamp() else {
                        warn!(
                            "failed to get timestamp from uuid '{}'",
                            uuid.as_hyphenated()
                        );
                        continue;
                    };
                    let (seconds, nanos) = timestamp.to_unix();
                    let Ok(session_start) = time::OffsetDateTime::from_unix_timestamp_nanos(
                        i128::from(seconds) * 1_000_000_000 + i128::from(nanos),
                    ) else {
                        warn!(
                            "failed to create OffsetDateTime from second: {seconds}, nanosecond: {nanos}"
                        );
                        continue;
                    };

                    if history.timestamp >= session_start {
                        continue;
                    }
                }
            }
            // we aggregate directory by ':' separating them
            FilterMode::Directory if history.cwd.split(':').contains(&context.cwd.as_str()) => {}
            FilterMode::Workspace if history.cwd.split(':').contains(&git_root) => {}
            _ => continue,
        }
        #[allow(clippy::cast_lossless, clippy::cast_precision_loss)]
        if let Some((score, indices)) = engine.fuzzy_indices(&history.command, query) {
            let begin = indices.first().copied().unwrap_or_default();

            let mut duration = (now - history.timestamp).as_seconds_f64().log2();
            if !duration.is_finite() || duration <= 1.0 {
                duration = 1.0;
            }
            // these + X.0 just make the log result a bit smoother.
            // log is very spiky towards 1-4, but I want a gradual decay.
            // eg:
            // log2(4) = 2, log2(5) = 2.3 (16% increase)
            // log2(8) = 3, log2(9) = 3.16 (5% increase)
            // log2(16) = 4, log2(17) = 4.08 (2% increase)
            let count = (*count as f64 + 8.0).log2();
            let begin = (begin as f64 + 16.0).log2();
            let path = path_dist(history.cwd.as_ref(), state.context.cwd.as_ref());
            let path = (path as f64 + 8.0).log2();

            // reduce longer durations, raise higher counts, raise matches close to the start
            let score = (-score as f64) * count / path / duration / begin;

            'insert: {
                // algorithm:
                // 1. find either the position that this command ranks
                // 2. find the same command positioned better than our rank.
                for i in 0..set.len() {
                    // do we out score the current position?
                    if ranks[i] > score {
                        ranks.insert(i, score);
                        set.insert(i, history.clone());
                        let mut j = i + 1;
                        while j < set.len() {
                            // remove duplicates that have a worse score
                            if set[j].command == history.command {
                                ranks.remove(j);
                                set.remove(j);

                                // break this while loop because there won't be any other
                                // duplicates.
                                break;
                            }
                            j += 1;
                        }

                        // keep it limited
                        if ranks.len() > 200 {
                            ranks.pop();
                            set.pop();
                        }

                        break 'insert;
                    }
                    // don't continue if this command has a better score already
                    if set[i].command == history.command {
                        break 'insert;
                    }
                }

                if set.len() < 200 {
                    ranks.push(score);
                    set.push(history.clone());
                }
            }
        }
    }

    set
}

fn path_dist(a: &Path, b: &Path) -> usize {
    let mut a: Vec<_> = a.components().collect();
    let b: Vec<_> = b.components().collect();

    let mut dist = 0;

    // pop a until there's a common ancestor
    while !b.starts_with(&a) {
        dist += 1;
        a.pop();
    }

    b.len() - a.len() + dist
}
