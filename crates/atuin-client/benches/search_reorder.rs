//! Head-to-head benchmark for the daemon search-result reorder step.
//!
//! After a fuzzy search the daemon returns history ids ranked by relevance; the client hydrates
//! the rows from its local sqlite (in arbitrary order) and must reorder them to match the daemon
//! ranking. `old_find_clone` is the shipped approach: for each id, linearly scan the hydrated Vec
//! and clone the hit — O(n^2) comparisons plus ~n full `History` clones per keystroke.
//! `new_map_move` drains the rows into a `HashMap` once and moves each hit out by id — O(n) with
//! zero clones. Both consume an identical, freshly built owned `(Vec<History>, Vec<HistoryId>)`
//! per iteration so the comparison is robust to load from other parallel builds.

use std::collections::HashMap;

use atuin_client::history::{History, HistoryId};
use rand::SeedableRng;
use rand::seq::SliceRandom;
use time::OffsetDateTime;
use uuid::Uuid;

/// The client caps daemon search results at 200 (see `fallback_to_db_search`), so this is the
/// realistic worst case for one keystroke.
const N: usize = 200;

/// Build `n` realistic history rows with distinct ids, plus a shuffled Vec of those ids standing
/// in for the daemon's relevance ranking.
fn inputs(n: usize) -> (Vec<History>, Vec<HistoryId>) {
    // A spread of real-shaped commands so `command`/`cwd`/`session` strings carry weight, which is
    // what makes a `History` clone non-trivial.
    let commands = [
        "git rebase --onto main feature/search-reorder",
        "cargo build -p atuin-client --release",
        "kubectl get pods -n production -o wide",
        "rg --hidden --glob '!.git' 'fn full_query' crates/",
        "docker compose up -d --build && docker compose logs -f",
        "psql -h db.internal -U atuin -c 'select count(*) from history'",
        "ssh deploy@10.0.0.42 'systemctl restart atuin-daemon'",
        "find . -type f -name '*.rs' -exec wc -l {} +",
    ];

    let base = OffsetDateTime::now_utc();
    let rows: Vec<History> = (0..n)
        .map(|i| {
            History::from_db()
                .id(HistoryId::new(Uuid::now_v7()))
                .timestamp(base + time::Duration::seconds(i64::try_from(i).unwrap_or(0)))
                .command(commands[i % commands.len()].to_string())
                .cwd("/home/user/projects/atuin/crates/atuin-client".to_string())
                .exit(0)
                .duration(1_234_567)
                .session("d8f3a1c04b7e4f2a9c6d5e8f0a1b2c3d".to_string())
                .hostname("workstation:user".to_string())
                .author("user".to_string())
                .intent(None)
                .deleted_at(None)
                .shell(Some("zsh".to_string()))
                .author_kind(None)
                .build()
                .into()
        })
        .collect();

    let mut ids: Vec<HistoryId> = rows.iter().map(|h| h.id).collect();
    // Deterministic shuffle: the ranking rarely matches the hydration order, so a linear scan
    // averages n/2 comparisons per id.
    let mut rng = rand::rngs::StdRng::seed_from_u64(0x5EA5_C4D5);
    ids.shuffle(&mut rng);

    (rows, ids)
}

/// Shipped approach: linear find + clone per id.
#[divan::bench]
fn old_find_clone(bencher: divan::Bencher) {
    bencher.with_inputs(|| inputs(N)).bench_values(|(results, ids): (Vec<History>, Vec<HistoryId>)| {
        let mut ordered = Vec::with_capacity(results.len());
        for id in &ids {
            if let Some(history) = results.iter().find(|h| h.id == *id) {
                ordered.push(history.clone());
            }
        }
        divan::black_box(ordered)
    });
}

/// New approach: drain into a map once, move each hit out by id.
#[divan::bench]
fn new_map_move(bencher: divan::Bencher) {
    bencher.with_inputs(|| inputs(N)).bench_values(|(results, ids): (Vec<History>, Vec<HistoryId>)| {
        let mut by_id: HashMap<HistoryId, History> =
            results.into_iter().map(|h| (h.id, h)).collect();
        let ordered: Vec<History> = ids.iter().filter_map(|id| by_id.remove(id)).collect();
        divan::black_box(ordered)
    });
}

fn main() {
    divan::main();
}
