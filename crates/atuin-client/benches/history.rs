use atuin_client::history::History;
use rand::Rng;
use rand::seq::SliceRandom;

use crate::_util::context::BenchCtx;

pub struct BenchHistory;

impl BenchHistory {
    /// List of commands which will be used to create some sort of history.
    const SEED_COMMANDS: [&str; 6] = [
        "cargo build --release",
        "git commit -m 'fix bug'",
        "curl -s https://example.com/api",
        "grep -rn pattern src/",
        "docker compose up -d",
        "ls -la /tmp",
    ];

    pub fn one(ctx: &mut BenchCtx) -> History {
        let now = ctx.now().unix_timestamp();
        let cmd = *Self::SEED_COMMANDS.choose(ctx.rng()).unwrap();
        History::import()
            .command(cmd)
            .timestamp(
                time::OffsetDateTime::from_unix_timestamp(ctx.rng().gen_range(0..now)).unwrap(),
            )
            .build()
            .into()
    }

    pub fn count(ctx: &mut BenchCtx, n: usize) -> Vec<History> {
        (0..n).map(|_| Self::one(ctx)).collect()
    }
}
