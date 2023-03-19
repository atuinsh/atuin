use std::sync::Arc;

use atuin_client::settings::FilterMode;
use chrono::Utc;
use skim::{prelude::ExactOrFuzzyEngineFactory, MatchEngineFactory};
use tokio::task::yield_now;

use super::interactive::{HistoryWrapper, SearchState};

pub async fn fuzzy_search(
    state: &SearchState,
    all_history: &[Arc<HistoryWrapper>],
) -> Vec<Arc<HistoryWrapper>> {
    let mut set = Vec::with_capacity(200);
    let mut ranks = Vec::with_capacity(200);
    let engine = ExactOrFuzzyEngineFactory::builder().fuzzy_algorithm(skim::FuzzyAlgorithm::SkimV2);
    let query = state.input.as_str();
    let engine = engine.create_engine(query);
    let now = Utc::now();

    for (i, item) in all_history.iter().enumerate() {
        if i % 256 == 0 {
            yield_now().await;
        }
        match state.filter_mode {
            FilterMode::Global => {}
            FilterMode::Host if item.hostname == state.context.hostname => {}
            FilterMode::Session if item.session == state.context.session => {}
            FilterMode::Directory if item.cwd == state.context.cwd => {}
            _ => continue,
        }
        #[allow(clippy::cast_lossless, clippy::cast_precision_loss)]
        if let Some(res) = engine.match_item(item.clone()) {
            let [score, begin, _, _] = res.rank;

            let mut duration = ((now - item.timestamp).num_seconds() as f64).log2();
            if !duration.is_finite() || duration <= 1.0 {
                duration = 1.0;
            }
            let count = (item.count as f64 + 16.0).log2();
            let begin = (begin as f64 + 16.0).log2();

            // reduce longer durations, raise higher counts, raise matches close to the start
            let score = (score as f64) * count / duration / begin;

            'insert: {
                // algorithm:
                // 1. find either the position that this command ranks
                // 2. find the same command positioned better than our rank.
                for i in 0..set.len() {
                    // do we out score the corrent position?
                    if ranks[i] > score {
                        ranks.insert(i, score);
                        set.insert(i, item.clone());
                        let mut j = i + 1;
                        while j < set.len() {
                            // remove duplicates that have a worse score
                            if set[j].command == item.command {
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
                    if set[i].command == item.command {
                        break 'insert;
                    }
                }

                if set.len() < 200 {
                    ranks.push(score);
                    set.push(item.clone());
                }
            }
        }
    }

    set
}
