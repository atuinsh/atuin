use std::{collections::HashMap, fmt::Write};

use clap::Args;
use eyre::{Result, eyre};
use indicatif::{ProgressBar, ProgressState, ProgressStyle};

use atuin_client::{
    api_client::Client,
    encryption::load_key,
    history::store::build_history_repair_replacement,
    record::{encryption::PASETO_V4, sqlite_store::SqliteStore, store::Store},
    settings::Settings,
};
use atuin_common::record::{EncryptedData, HostId, Record, RecordIdx};

/// Repair the record store after a botched encryption key change.
///
/// When a client logs in with a correct password but wrong encryption key, it
/// writes records that nobody — including itself — can ever decrypt. Those
/// records then propagate via sync to every other host and to the server,
/// blocking history rebuild on every machine.
///
/// `atuin store repair` surgically replaces the encrypted payload of each
/// undecryptable record with a decryptable no-op (a `HistoryRecord::Delete`
/// pointing at a freshly-minted UUID that does not correspond to any real
/// history entry), preserving the record's `(id, host, idx, version, tag,
/// timestamp)` metadata. The idx chain stays intact, so sync remains healthy.
///
/// Run this on every host that holds bad records. The first host repairs the
/// server; subsequent hosts just pull the already-fixed version down and
/// overwrite their local copy.
#[derive(Args, Debug)]
pub struct Repair {
    /// Do not contact the sync server. Only rebuild the local store.
    ///
    /// Useful if the server is unreachable and you just want to unblock sync
    /// locally. Note that local-only repairs do not propagate to other hosts,
    /// and the server will still hold the bad records until someone runs a
    /// full repair.
    #[arg(long)]
    pub local_only: bool,

    /// How many records to send to (or fetch from) the sync server per request.
    /// Keep this relatively small on slow links; bump it up on fast links to
    /// reduce round-trip overhead.
    #[arg(long, default_value = "100")]
    pub page: u64,
}

impl Repair {
    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let key: [u8; 32] = load_key(settings)?.into();

        println!("Scanning local record store for undecryptable entries...");

        let all = store.load_all().await?;
        let mut bad = Vec::new();
        for record in all {
            if record.clone().decrypt::<PASETO_V4>(&key).is_err() {
                bad.push(record);
            }
        }

        if bad.is_empty() {
            println!("No undecryptable records found. Nothing to repair.");
            return Ok(());
        }

        println!("Found {} undecryptable record(s).", bad.len());

        let client = if self.local_only {
            None
        } else {
            Some(
                Client::new(
                    &settings.sync_address,
                    settings.sync_auth_token().await?,
                    settings.network_connect_timeout,
                    settings.network_timeout,
                )
                .map_err(|e| eyre!("failed to create sync client: {e}"))?,
            )
        };

        // Phase 1: fetch each affected (host, tag) chain's server-side range.
        // Progress is measured in pages fetched against the total number of
        // pages we need based on the idx ranges.
        let server_view = if let Some(client) = client.as_ref() {
            fetch_server_view(client, &bad, self.page).await?
        } else {
            HashMap::new()
        };

        // Phase 2: decide push-vs-pull for each record. Pure in-memory work,
        // very fast, so we don't show a progress bar — just a summary line.
        let mut pulled_from_server = 0usize;
        let mut to_push: Vec<Record<EncryptedData>> = Vec::new();
        let mut to_overwrite_locally: Vec<Record<EncryptedData>> = Vec::new();

        for bad_record in &bad {
            let replacement = resolve_replacement(bad_record, &key, &server_view)?;
            match replacement.source {
                ReplacementSource::ServerHadClean => pulled_from_server += 1,
                ReplacementSource::WeGenerated => {
                    if client.is_some() {
                        to_push.push(replacement.record.clone());
                    }
                }
            }
            to_overwrite_locally.push(replacement.record);
        }

        let to_generate_count = bad.len() - pulled_from_server;
        println!(
            "Resolved {} replacements: {} already fixed on server, {} to generate{}.",
            bad.len(),
            pulled_from_server,
            to_generate_count,
            if client.is_some() && to_generate_count > 0 {
                " and push"
            } else {
                ""
            },
        );

        // Phase 3: push generated replacements to the server in page-sized batches.
        let mut pushed_to_server = 0usize;
        if let Some(client) = client.as_ref()
            && !to_push.is_empty()
        {
            let push_pb = record_progress_bar(to_push.len() as u64, "pushing");
            let chunk_size = usize::try_from(self.page).unwrap_or(usize::MAX);
            for chunk in to_push.chunks(chunk_size) {
                client.repair_records(chunk).await?;
                pushed_to_server += chunk.len();
                push_pb.inc(chunk.len() as u64);
            }
            push_pb.finish_with_message("pushed");
        }

        // Phase 4: overwrite local copies. The store's push path ignores
        // conflicts on the primary key, so we must delete first to replace
        // in-place. Delete + insert per record is slow on sqlite, so show
        // a progress bar: with 10k+ records this takes long enough to matter.
        let local_pb = record_progress_bar(to_overwrite_locally.len() as u64, "rewriting local");
        for record in &to_overwrite_locally {
            store.delete(record.id).await?;
            store.push_batch(std::iter::once(record)).await?;
            local_pb.inc(1);
        }
        local_pb.finish_with_message("local store updated");

        println!(
            "Repair complete: {pulled_from_server} pulled from server, {pushed_to_server} pushed to server."
        );

        println!("Verifying local store decrypts with current key...");
        store.verify(&key).await?;
        println!("Local store encryption verified OK.");

        Ok(())
    }
}

#[derive(Debug)]
enum ReplacementSource {
    /// Someone else already fixed this record on the server; we pulled their fix.
    ServerHadClean,
    /// We generated a replacement. For the normal flow it will be pushed to the
    /// server in a later batch; for --local-only it will only update the local store.
    WeGenerated,
}

struct Replacement {
    record: Record<EncryptedData>,
    source: ReplacementSource,
}

/// Decide what should replace a bad record using a pre-fetched view of the server.
fn resolve_replacement(
    bad: &Record<EncryptedData>,
    key: &[u8; 32],
    server_view: &HashMap<(HostId, String, RecordIdx), Record<EncryptedData>>,
) -> Result<Replacement> {
    if let Some(remote) = server_view.get(&(bad.host.id, bad.tag.clone(), bad.idx))
        && remote.clone().decrypt::<PASETO_V4>(key).is_ok()
    {
        return Ok(Replacement {
            record: remote.clone(),
            source: ReplacementSource::ServerHadClean,
        });
    }

    let replacement = build_history_repair_replacement(bad, key)?;
    Ok(Replacement {
        record: replacement,
        source: ReplacementSource::WeGenerated,
    })
}

/// Fetch the server's current version of every bad record, in batches.
///
/// Groups bad records by (host, tag) and, for each group, pages through
/// `next_records` starting at `min(idx)` until we've seen every idx in the
/// bad set or the server runs out of records. Returns a flat lookup map.
///
/// Shows a progress bar keyed on total records we need to fetch so that long
/// discovery phases — think 150k+ records on an active host — don't look like
/// a hang.
async fn fetch_server_view(
    client: &Client<'_>,
    bad: &[Record<EncryptedData>],
    page_size: u64,
) -> Result<HashMap<(HostId, String, RecordIdx), Record<EncryptedData>>> {
    use std::collections::BTreeSet;

    // (host, tag) -> sorted set of bad idxs we want to resolve.
    let mut groups: HashMap<(HostId, String), BTreeSet<RecordIdx>> = HashMap::new();
    for b in bad {
        groups
            .entry((b.host.id, b.tag.clone()))
            .or_default()
            .insert(b.idx);
    }

    // Total records we expect to scan across all groups, used to size the bar.
    // Use the range (max - min + 1) per group since we page through the range,
    // not just the exact set of bad idxs.
    let total_range: u64 = groups
        .values()
        .map(|idxs| {
            let min = *idxs.iter().next().unwrap();
            let max = *idxs.iter().next_back().unwrap();
            max - min + 1
        })
        .sum();

    let pb = record_progress_bar(total_range, "fetching from server");

    let mut view: HashMap<(HostId, String, RecordIdx), Record<EncryptedData>> = HashMap::new();

    for ((host, tag), idxs) in groups {
        let min = *idxs.iter().next().expect("group is non-empty");
        let max = *idxs.iter().next_back().expect("group is non-empty");

        let mut cursor = min;
        while cursor <= max {
            let page = client
                .next_records(host, tag.clone(), cursor, page_size)
                .await?;
            if page.is_empty() {
                // Credit the progress bar for the rest of the range we will
                // never get to see (server ran out).
                pb.inc(max.saturating_sub(cursor) + 1);
                break;
            }
            let last_idx = page.last().expect("page non-empty").idx;
            let scanned = last_idx - cursor + 1;
            for r in page {
                if r.idx > max {
                    break;
                }
                if idxs.contains(&r.idx) {
                    view.insert((host, tag.clone(), r.idx), r);
                }
            }
            pb.inc(scanned);
            cursor = last_idx + 1;
        }
    }

    pb.finish_with_message("server view fetched");

    Ok(view)
}

/// Progress bar styled to match the project's existing sync progress bars
/// (see `crates/atuin-client/src/record/sync.rs`).
fn record_progress_bar(total: u64, msg: &'static str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {human_pos}/{human_len} ({eta}) {msg}",
        )
        .unwrap()
        .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
            write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap();
        })
        .progress_chars("#>-"),
    );
    pb.set_message(msg);
    pb
}
