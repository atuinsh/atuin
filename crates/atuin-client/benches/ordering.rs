use crate::_util::context::BenchCtx;
use crate::history::BenchHistory;
use atuin_client::ordering::reorder_fuzzy;
use atuin_client::settings::SearchMode;

/// reorder_fuzzy is invoked on every keystroke during interactive fuzzy search, so keeping the
/// performance in check is important.
///
/// The interactive search hardcodes a limit of 200 deduplicated entries (`engines/db.rs`).
#[divan::bench(args = [10, 200, 1_000], min_time = 1)]
fn reorder_fuzzy_bench(bencher: divan::Bencher, n: usize) {
    bencher
        .with_inputs(|| {
            let mut ctx = BenchCtx::new();
            BenchHistory::count(&mut ctx, n)
        })
        .bench_values(|histories| reorder_fuzzy(SearchMode::Fuzzy, "curl", histories));
}
