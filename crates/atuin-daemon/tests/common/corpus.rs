//! Deterministic synthetic history: a heavily repeated head of common commands, a body of unique
//! templated commands, a handful of sessions/directories/hosts, and a sprinkle of agent-run
//! entries (which the search index must skip). Same seed, same corpus.

use std::collections::HashSet;
use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::history::store::{HistoryRecord, HistoryStore};
use atuin_client::history::{AuthorKind, History, HistoryId, Version};
use atuin_common::utils::uuid_v7;
use atuin_daemon::search::SearchIndex;
use atuin_domain::record::{Host, Record, RecordSeriesKey, RecordTag, RecordVersion};
use easy_cast::Conv;
use time::OffsetDateTime;
use tokio::sync::RwLock;
use uuid::Uuid;

/// How many rows go into one `save_bulk` / `push_batch` transaction while seeding.
pub const SEED_BATCH: usize = 5000;

const COMMON: &[&str] = &[
    "ls",
    "git status",
    "ls -la",
    "git diff",
    "cd ..",
    "git pull",
    "git push",
    "cargo build",
    "cargo test",
    "make",
    "pwd",
    "clear",
    "git log",
    "npm install",
    "npm test",
    "docker ps",
    "cargo check",
    "git stash",
    "git stash pop",
    "docker compose up",
    "docker compose down",
    "cargo run",
    "yarn dev",
    "kubectl get pods",
    "git checkout main",
    "git rebase main",
    "brew update",
    "htop",
    "vim .",
    "code .",
    "make test",
    "python3 -m http.server",
    "nvim",
    "history",
    "df -h",
    "du -sh .",
    "uptime",
    "exit",
    "echo déjà vu",
    "vim メモ.md",
];

const DIRS: &[&str] = &[
    "/home/ellie/src/atuin",
    "/home/ellie/src/atuin/crates",
    "/tmp/scratch",
    "/home/ellie",
    "/var/log",
    "/home/ellie/notes",
    "/opt/work/backend",
    "/opt/work/ui",
    "/",
    "/home/ellie/dotfiles",
    "/home/ellie/src/atuin/docs",
    "/etc",
];

const ORIGINS: &[&str] = &["laptop:ellie", "desktop:ellie", "prod-web-01:deploy"];
const SHELLS: &[Option<&str>] = &[Some("zsh"), Some("bash"), Some("fish"), None];

/// xorshift64*: dependency-free, identical on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::conv(self.next() % u64::conv(n))
    }
}

/// Deterministic stream of realistic `History` entries.
pub struct HistoryGen {
    rng: Rng,
    sessions: Vec<String>,
    base: OffsetDateTime,
    produced: u64,
}

impl HistoryGen {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng(seed.max(1));
        // Sessions must be valid UUIDs or the index skips the row. Derive them from the rng so the
        // corpus (not just the commands) is reproducible.
        let sessions = (0..8)
            .map(|_| {
                let mut bytes = [0u8; 16];
                for chunk in bytes.chunks_mut(8) {
                    let word = rng.next().to_le_bytes();
                    chunk.copy_from_slice(&word[..chunk.len()]);
                }
                Uuid::from_bytes(bytes).as_simple().to_string()
            })
            .collect();
        Self {
            rng,
            sessions,
            // ~2022-07-05, one entry every 30s from there.
            base: OffsetDateTime::from_unix_timestamp(1_657_000_000).unwrap(),
            produced: 0,
        }
    }

    /// A command for the next entry. Whether it is corpus-unique (embeds the entry ordinal, so a
    /// "job-*" command never repeats) is derived later by [`Self::is_unique`], the single source
    /// of truth for that question.
    fn command(&mut self) -> String {
        let roll = self.rng.below(100);
        if roll < 40 {
            let i = self.rng.below(COMMON.len()).min(self.rng.below(COMMON.len()));
            COMMON[i].to_owned()
        } else {
            let arg = self.rng.next() % 1_000_000;
            format!("job-{} --run --id={arg} --path /tmp/work/{}", self.produced, self.produced)
        }
    }

    /// The next entry. Roughly 5% are agent-run (skipped by the search index).
    pub fn next(&mut self) -> History {
        let command = self.command();
        let ordinal = self.produced;
        self.produced += 1;

        let is_agent = self.rng.below(100) < 5;
        let cwd = DIRS[self.rng.below(DIRS.len())].to_owned();
        let origin = ORIGINS[self.rng.below(ORIGINS.len())];
        let session = self.sessions[self.rng.below(self.sessions.len())].clone();
        let shell = SHELLS[self.rng.below(SHELLS.len())].map(str::to_owned);
        let exit = if self.rng.below(10) == 0 {
            1
        } else {
            0
        };
        let duration = i64::conv(self.rng.below(5_000_000_000));

        History::from_db()
            .id(HistoryId::from(uuid_v7()))
            .timestamp(self.base + time::Duration::seconds(i64::conv(ordinal) * 30))
            .command(command)
            .cwd(cwd)
            .exit(exit)
            .duration(duration)
            .session(session)
            .hostname(origin.to_owned())
            .author(if is_agent {
                "claude-code".to_owned()
            } else {
                "ellie".to_owned()
            })
            .intent(None)
            .deleted_at(None)
            .shell(shell)
            .author_kind(is_agent.then_some(AuthorKind::Agent))
            .build()
            .into()
    }

    /// Whether `history` (produced by this generator) has a corpus-unique command.
    #[must_use]
    pub fn is_unique(history: &History) -> bool {
        history.command.starts_with("job-")
    }
}

/// What a seeding pass produced: every id, plus up to `UNIQUE_SAMPLE` full entries whose commands
/// are unique in the corpus (so a test can assert on search results for exactly one row).
#[derive(Debug, Default, Clone)]
pub struct Seeded {
    pub ids: Vec<HistoryId>,
    pub unique: Vec<History>,
}

pub const UNIQUE_SAMPLE: usize = 500;

/// Whether the search index would carry this entry (mirrors `SearchIndex::add_history` with the
/// default "all shells" filter).
#[must_use]
pub fn index_eligible(history: &History) -> bool {
    !history.is_agent() && Uuid::parse_str(&history.session).is_ok()
}

/// Insert `rows` generated entries into `db` in `SEED_BATCH` transactions, streaming so a 1M-row
/// seed never holds the whole corpus in memory. When `index` is given, each batch is also added to
/// it (what the daemon's own loader would do).
pub async fn seed_history_db(
    db: &Sqlite,
    history_gen: &mut HistoryGen,
    rows: usize,
    index: Option<&Arc<RwLock<SearchIndex>>>,
) -> Seeded {
    let mut seeded = Seeded::default();
    let mut remaining = rows;
    while remaining > 0 {
        let take = remaining.min(SEED_BATCH);
        let batch: Vec<History> = (0..take).map(|_| history_gen.next()).collect();
        db.save_bulk(&batch).await.expect("seeding history db");
        if let Some(index) = index {
            index.read().await.add_histories(&batch);
        }
        for history in &batch {
            seeded.ids.push(history.id);
            if seeded.unique.len() < UNIQUE_SAMPLE && HistoryGen::is_unique(history) {
                seeded.unique.push(history.clone());
            }
        }
        remaining -= take;
    }
    seeded
}

/// Append `Create` records for `histories` to the record store, contiguous after the current last
/// idx, encrypting with the store's key. Mirrors `HistoryStore::push_batch`, which is private.
pub async fn seed_record_store(history_store: &HistoryStore, histories: &[History]) {
    let series = RecordSeriesKey::new(history_store.host_id, RecordTag::History);
    let mut idx = history_store.store.last(&series).await.unwrap().map_or(0, |r| r.idx + 1);
    for chunk in histories.chunks(SEED_BATCH) {
        let mut records = Vec::with_capacity(chunk.len());
        for history in chunk {
            let bytes = HistoryRecord::Create(history.clone()).serialize().unwrap();
            let record = Record::builder()
                .host(Host::new(history_store.host_id))
                .version(RecordVersion::from(Version::LATEST.name()))
                .tag(RecordTag::History)
                .idx(idx)
                .data(bytes)
                .build();
            idx += 1;
            records.push(record.encrypt(&history_store.encryption_key));
        }
        history_store.store.push_batch(records.iter()).await.expect("seeding record store");
    }
}

/// Distinct index-eligible commands among the active rows of `db`: what `SearchIndex::command_count`
/// must equal once the index is in sync with the database.
///
/// The index itself loads with `all_paged(.., unique = true)`, which groups rows by
/// (command, cwd, hostname, session) and keeps one representative per group, while this oracle
/// instead counts distinct command *text* over all active rows. The two agree because every row in
/// a `unique = true` group shares the same command text by construction (that's what makes it a
/// group) — so the set of distinct commands across the deduplicated rows the index loads is
/// identical to the set of distinct commands across every active row.
pub async fn distinct_indexable_commands(db: &Sqlite) -> usize {
    let mut commands: HashSet<String> = HashSet::new();
    let mut pager = db.all_paged(SEED_BATCH, false, false);
    while let Some(page) = pager.next().await.unwrap() {
        commands.extend(page.into_iter().filter(index_eligible).map(|h| h.command));
    }
    commands.len()
}
