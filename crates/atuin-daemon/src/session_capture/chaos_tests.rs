//! A chaos test of the recovery protocol on the real components: an in-memory record store and
//! sidecar, the real coordinator, capture's sink, the sync worker's reprojection and the gRPC
//! rebuild, on a multi-thread runtime. Test hooks (see [`super::hooks`]) delay every step of the
//! protocol at random, fail wipes, and fail or panic replays, while rebuilds, captures (of lines
//! already persisted and new ones), sync reprojections and reads run at once. Once it quiesces,
//! it checks the protocol's invariants.
//!
//! `ATUIN_CHAOS_ITERATIONS` sets how many seeds to run: by default 100, a few seconds in all, as
//! each takes some 50ms of in-memory SQLite.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use atuin_client::ai_session::{NativeSessionId, SourceId};
use atuin_domain::record::{RecordId, RecordTag};
use futures::future::BoxFuture;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rstest::rstest;
use time::OffsetDateTime;
use tonic::Request;

use super::hooks::{Fault, Hooks, INJECTED_PANIC, Point, Section};
use super::*;
use crate::grpc::ai::session::pb::ai_session_server::AiSession as _;
use crate::grpc::ai::session::pb::{ListSessionsRequest, RebuildSessionsRequest};
use crate::grpc::ai::session::{Service, is_rebuilding};
use crate::sync::spawn_ai_session_projector;

const TIMEOUT: Duration = Duration::from_secs(20);

/// Counts, over all iterations, of what the chaos made happen: to tell it did.
#[derive(Debug, Default)]
struct Totals {
    replays: AtomicU64,
    replay_panics: AtomicU64,
    replay_failures: AtomicU64,
    wipes: AtomicU64,
    wipe_failures: AtomicU64,
    captured: AtomicU64,
    ended_unrecoverable: AtomicU64,
    /// Captures a rebuild overtook between their wait and their lock.
    captures_overtaken: AtomicU64,
}

#[derive(Debug)]
struct Chaos {
    rng: parking_lot::Mutex<StdRng>,
    /// Whether to fail and panic things, besides delaying them.
    faults: AtomicBool,
    replays: Arc<AtomicUsize>,
    wipes: Arc<AtomicUsize>,
    captures: Arc<AtomicUsize>,
    violations: parking_lot::Mutex<Vec<String>>,
    totals: Arc<Totals>,
    probe: parking_lot::Mutex<Option<Probe>>,
    /// Wipes that went ahead.
    emptied: Arc<AtomicU64>,
    /// How many wipes had gone ahead when the last replay to end began.
    last_replayed_from: Arc<AtomicU64>,
}

/// What [`Chaos::check_complete`] looks at.
#[derive(Debug, Clone)]
struct Probe {
    sidecar: AiSessionDatabase,
    host: HostId,
    /// This host's lines pushed to the record store: those persisted, and each capture reported
    /// new. A line pushed twice (the dedup gate trusting a sidecar missing it) is reported new
    /// twice.
    pushed: Arc<parking_lot::Mutex<Vec<LineKey>>>,
}

/// Leaves a section when dropped (a panic unwinding included).
struct Leave {
    counter: Arc<AtomicUsize>,
    /// A replay's: where to say, when it ends, how many wipes had gone ahead when it began.
    replayed_from: Option<(Arc<AtomicU64>, u64)>,
}

impl Drop for Leave {
    fn drop(&mut self) {
        if let Some((last, from)) = &self.replayed_from {
            last.store(*from, Ordering::SeqCst);
        }
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// No replay has ended yet.
const NONE_REPLAYED: u64 = u64::MAX;

impl Chaos {
    fn new(seed: u64, totals: Arc<Totals>) -> Self {
        Self {
            rng: parking_lot::Mutex::new(StdRng::seed_from_u64(seed)),
            faults: AtomicBool::new(true),
            replays: Arc::default(),
            wipes: Arc::default(),
            captures: Arc::default(),
            violations: parking_lot::Mutex::default(),
            totals,
            probe: parking_lot::Mutex::default(),
            emptied: Arc::default(),
            last_replayed_from: Arc::new(AtomicU64::new(NONE_REPLAYED)),
        }
    }

    /// I1 where it matters: the sidecar holds every line of this host pushed so far. Called
    /// holding capture's lock with the store found ready, where capture trusts the sidecar.
    async fn check_complete(&self, when: &str) {
        let Some(probe) = self.probe.lock().clone() else {
            return;
        };
        let held = keys(&probe.sidecar, Some(probe.host)).await;
        let missing: Vec<_> =
            probe.pushed.lock().iter().filter(|key| !held.contains(*key)).cloned().collect();
        if !missing.is_empty() {
            self.violation(format!("{when} while it was missing {missing:?}"));
        }
    }

    /// Watch the store's state: it settles only on a replay that began after the last wipe (the
    /// last replay to end, as one runs at a time and the coordinator settles only once it has).
    /// A settled state read with no wipe or replay ending around the read is checked.
    async fn watch_settling(&self, mut state: watch::Receiver<StoreState>) {
        while state.changed().await.is_ok() {
            let before = self.settling();
            let now = *state.borrow_and_update();
            if now == StoreState::Recovering || self.settling() != before {
                continue;
            }
            let (emptied, replayed_from) = before;
            if replayed_from != NONE_REPLAYED && replayed_from != emptied {
                self.violation(format!(
                    "{now:?} on a replay begun after {replayed_from} wipes, of {emptied}"
                ));
            }
        }
    }

    fn settling(&self) -> (u64, u64) {
        (self.emptied.load(Ordering::SeqCst), self.last_replayed_from.load(Ordering::SeqCst))
    }

    fn violation(&self, what: String) {
        self.violations.lock().push(what);
    }

    /// Roll a die: true with probability `p`, when faults are on.
    fn fault(&self, p: f64) -> bool {
        self.faults.load(Ordering::SeqCst) && self.rng.lock().gen_bool(p)
    }

    /// A random pause of up to some milliseconds, to spread rebuilds, syncs and reads over the
    /// time capture runs.
    async fn pause(&self) {
        let ms = self.rng.lock().gen_range(0..12);
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    /// A random delay: nothing, a few yields, or (rarely) a timer tick.
    async fn delay(&self) {
        let (kind, n) = {
            let mut rng = self.rng.lock();
            (rng.gen_range(0..100), rng.gen_range(1..8))
        };
        match kind {
            0..40 => {}
            40..97 => {
                for _ in 0..n {
                    tokio::task::yield_now().await;
                }
            }
            _ => tokio::time::sleep(Duration::from_micros(200)).await,
        }
    }
}

impl Hooks for Chaos {
    fn at(&self, point: Point) -> BoxFuture<'_, Fault> {
        Box::pin(async move {
            self.delay().await;
            // Widen the windows a rebuild could land in.
            if matches!(point, Point::CaptureWaited | Point::BeforeWipe)
                && self.rng.lock().gen_bool(0.3)
            {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            match point {
                Point::WipeLocked if self.fault(0.1) => {
                    self.totals.wipe_failures.fetch_add(1, Ordering::Relaxed);
                    Fault::Fail
                }
                Point::WipeLocked => {
                    // Said before it happens, but under capture's lock and while recovering.
                    self.emptied.fetch_add(1, Ordering::SeqCst);
                    Fault::None
                }
                Point::ReplayBeforeSettle if self.fault(0.1) => {
                    self.totals.replay_panics.fetch_add(1, Ordering::Relaxed);
                    Fault::Panic
                }
                Point::ReplayBeforeSettle if self.fault(0.1) => {
                    self.totals.replay_failures.fetch_add(1, Ordering::Relaxed);
                    Fault::Fail
                }
                Point::CaptureOvertaken => {
                    self.totals.captures_overtaken.fetch_add(1, Ordering::Relaxed);
                    Fault::None
                }
                Point::CaptureChecked => {
                    self.check_complete("capture checked the sidecar").await;
                    Fault::None
                }
                _ => Fault::None,
            }
        })
    }

    fn enter(&self, section: Section) -> Box<dyn Send> {
        let counter = match section {
            Section::Replay => {
                self.totals.replays.fetch_add(1, Ordering::Relaxed);
                &self.replays
            }
            Section::Wipe => {
                self.totals.wipes.fetch_add(1, Ordering::Relaxed);
                &self.wipes
            }
            Section::Capture => &self.captures,
        };
        let now = counter.fetch_add(1, Ordering::SeqCst) + 1;
        if matches!(section, Section::Replay) && now > 1 {
            self.violation(format!("{now} replays at once"));
        }
        if self.wipes.load(Ordering::SeqCst) > 0 && self.captures.load(Ordering::SeqCst) > 0 {
            self.violation("a capture between its check and its push during a wipe".to_owned());
        }
        let replayed_from = matches!(section, Section::Replay)
            .then(|| (self.last_replayed_from.clone(), self.emptied.load(Ordering::SeqCst)));
        Box::new(Leave {
            counter: counter.clone(),
            replayed_from,
        })
    }
}

fn line(session: &str, source: &str) -> Message {
    Message::builder()
        .id(RecordId(atuin_common::utils::uuid_v7()))
        .session(HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from(session.to_owned()),
        })
        .source_id(SourceId::from(source.to_owned()))
        .timestamp(OffsetDateTime::UNIX_EPOCH)
        .role(Role::User)
        .content(vec![Content::Text("hello".to_owned())])
        .build()
}

type LineKey = (String, String);

fn key_of(msg: &Message) -> LineKey {
    (msg.session.session.to_string(), msg.source_id.to_string())
}

/// Every message key the sidecar holds, of `host` only if given.
async fn keys(db: &AiSessionDatabase, host: Option<HostId>) -> BTreeSet<LineKey> {
    let mut keys = BTreeSet::new();
    for session in db.list_sessions(&SessionFilter::default()).await.unwrap() {
        let mut messages = Box::pin(db.messages(&session.handle));
        while let Some(msg) = messages.next().await {
            let msg = msg.unwrap();
            if host.is_none() || msg.host == host {
                keys.insert((session.handle.session.to_string(), msg.source_id.to_string()));
            }
        }
    }
    keys
}

/// What the record store holds, projected afresh.
async fn stored(records: &AiSessionStore) -> AiSessionDatabase {
    let fresh = AiSessionDatabase::in_memory().await.unwrap();
    records.build(&fresh).await.unwrap();
    fresh
}

async fn iteration(seed: u64, totals: Arc<Totals>) {
    let mut rng = StdRng::seed_from_u64(seed ^ 0x5eed);
    let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
    let key = Key::generate();
    let local_host = HostId(atuin_common::utils::uuid_v7());
    let records =
        AiSessionStore::builder().store(store.clone()).host_id(local_host).key(key.clone()).build();
    // Another host's records, as a sync downloads them.
    let remote = AiSessionStore::builder()
        .store(store.clone())
        .host_id(HostId(atuin_common::utils::uuid_v7()))
        .key(key)
        .build();
    let sidecar = AiSessionDatabase::in_memory().await.unwrap();

    // Lines already persisted, which capture reads again.
    let persisted: Vec<_> =
        (0..3).map(|i| line(&format!("s{}", i % 2), &format!("p{i}"))).collect();
    for msg in &persisted {
        records.push(msg).await.unwrap();
    }
    remote.push(&line("r", "r0")).await.unwrap();

    let pushed = Arc::new(parking_lot::Mutex::new(persisted.iter().map(key_of).collect()));
    let chaos = Arc::new(Chaos::new(seed, totals.clone()));
    *chaos.probe.lock() = Some(Probe {
        sidecar: sidecar.clone(),
        host: local_host,
        pushed: pushed.clone(),
    });
    let capture = Arc::new(AiHarnessSessionCapture::open_with(
        records.clone(),
        sidecar.clone(),
        false,
        BlockingPool::new(NonZeroUsize::MIN),
        Some(chaos.clone() as Arc<dyn Hooks>),
    ));
    let watching = tokio::spawn({
        let (chaos, state) = (chaos.clone(), capture.state.clone());
        async move { chaos.watch_settling(state).await }
    });
    let service = Service::new(capture.clone());
    let projector =
        spawn_ai_session_projector(records.clone(), sidecar.clone(), capture.recovery());

    let mut tasks = tokio::task::JoinSet::new();
    // Rebuilds, through gRPC: during startup recovery or another rebuild, or (mostly) once the
    // store settled, while capture runs.
    for _ in 0..rng.gen_range(1..=3) {
        let (service, chaos, capture) = (service.clone(), chaos.clone(), capture.clone());
        let (count, settled_first) = (rng.gen_range(1..=2), rng.gen_bool(0.7));
        tasks.spawn(async move {
            for _ in 0..count {
                if settled_first {
                    capture.ready().await;
                }
                chaos.pause().await;
                if let Err(status) =
                    service.rebuild_sessions(Request::new(RebuildSessionsRequest {})).await
                    && !matches!(
                        status.code(),
                        tonic::Code::FailedPrecondition | tonic::Code::Internal
                    )
                {
                    chaos.violation(format!("rebuild answered {status:?}"));
                }
            }
        });
    }
    // Sync downloads, and the sync worker reprojecting them.
    {
        let (remote, chaos, projector) = (remote.clone(), chaos.clone(), projector.clone());
        tasks.spawn(async move {
            for i in 1..3 {
                chaos.pause().await;
                remote.push(&line("r", &format!("r{i}"))).await.unwrap();
                projector.send(()).unwrap();
            }
        });
    }
    // Reads: served, or refused as rebuilding.
    {
        let (service, chaos) = (service.clone(), chaos.clone());
        tasks.spawn(async move {
            for _ in 0..3 {
                chaos.pause().await;
                let listed = service
                    .list_sessions(Request::new(ListSessionsRequest {
                        harness: None,
                        updated_since: None,
                        filter: None,
                    }))
                    .await;
                if let Err(status) = listed
                    && !is_rebuilding(&status)
                {
                    chaos.violation(format!("a read failed: {status:?}"));
                }
            }
        });
    }
    // Ready under capture's lock, the sidecar holds every record of this host (I1). (Capture
    // checks the same where it trusts the sidecar: see `Chaos::check_complete`.)
    {
        let (capture, chaos, sidecar) = (capture.clone(), chaos.clone(), sidecar.clone());
        tasks.spawn(async move {
            for _ in 0..4 {
                chaos.pause().await;
                let _local = sidecar.lock_local_projection().await;
                if capture.is_available() {
                    chaos.check_complete("ready").await;
                }
            }
        });
    }
    // Capture: two listeners re-reading the persisted lines, with overlapping new ones.
    let new: Vec<_> = (0..6).map(|i| (format!("s{}", i % 3), format!("n{i}"))).collect();
    let mut captures = tokio::task::JoinSet::new();
    for range in [0..4, 2..6] {
        let mut lines: Vec<_> =
            persisted.iter().map(key_of).chain(new[range].iter().cloned()).collect();
        if rng.gen_bool(0.5) {
            lines.reverse();
        }
        let (capture, chaos, pushed) = (capture.clone(), chaos.clone(), pushed.clone());
        captures.spawn(async move {
            for (session, source) in lines {
                chaos.delay().await;
                // The listener's append: paused while unavailable, retried once ready.
                let appended = engine::append(&capture.sink, line(&session, &source)).await;
                match appended {
                    Ok(Appended::New) => {
                        chaos.totals.captured.fetch_add(1, Ordering::Relaxed);
                        pushed.lock().push((session, source));
                    }
                    Ok(Appended::Duplicate) => {}
                    Err(err) => chaos.violation(format!("capture failed: {err}")),
                }
            }
        });
    }

    // Everything but capture ends.
    let ended = tokio::time::timeout(TIMEOUT, async {
        while let Some(task) = tasks.join_next().await {
            task.unwrap();
        }
    });
    assert!(ended.await.is_ok(), "seed {seed}: the chaos never ended");

    // No stall: the store settles (L2).
    let mut state = capture.state.clone();
    let settled = tokio::time::timeout(TIMEOUT, state.wait_for(|s| *s != StoreState::Recovering));
    let settled =
        *settled.await.unwrap_or_else(|_| panic!("seed {seed}: stalled recovering")).unwrap();

    // Healthy from here: a store left unavailable by a failed rebuild comes back with one more
    // (L1); one whose startup recovery failed stays unavailable, refusing rebuilds.
    chaos.faults.store(false, Ordering::SeqCst);
    let mut recoverable = true;
    if settled == StoreState::Unavailable {
        match capture.rebuild().await {
            Ok(()) => {}
            Err(RebuildError::Unavailable) => recoverable = false,
            Err(err) => panic!("seed {seed}: the last rebuild failed: {err}"),
        }
    }
    if recoverable {
        let ready = tokio::time::timeout(TIMEOUT, capture.ready()).await;
        assert!(
            ready.unwrap_or_else(|_| panic!("seed {seed}: stalled")),
            "seed {seed}: unavailable"
        );
        let captured = tokio::time::timeout(TIMEOUT, async {
            while let Some(task) = captures.join_next().await {
                task.unwrap();
            }
        });
        assert!(captured.await.is_ok(), "seed {seed}: capture never resumed");
    } else {
        totals.ended_unrecoverable.fetch_add(1, Ordering::Relaxed);
        assert_eq!(settled, StoreState::Unavailable);
        captures.abort_all();
    }

    watching.abort();
    let violations = chaos.violations.lock().clone();
    assert!(violations.is_empty(), "seed {seed}: {violations:#?}");

    // No duplicate record (I2): each line pushed once, and each key once in the record store.
    let pushed = pushed.lock().clone();
    let distinct: BTreeSet<_> = pushed.iter().cloned().collect();
    assert_eq!(pushed.len(), distinct.len(), "seed {seed}: a line pushed twice: {pushed:?}");
    let all = store.all_tagged(&RecordTag::AiSession).await.unwrap().len();
    let fresh = stored(&records).await;
    let expected = keys(&fresh, None).await;
    assert_eq!(all, expected.len(), "seed {seed}: a record pushed twice");
    let local = keys(&fresh, Some(local_host)).await;
    assert_eq!(local, distinct, "seed {seed}");
    if recoverable {
        // Every line captured, once.
        assert_eq!(local.len(), persisted.len() + new.len(), "seed {seed}");
        // Ready: every record projected (I1), this host's at once, downloads once the sync
        // worker is done.
        assert!(local.is_subset(&keys(&sidecar, Some(local_host)).await), "seed {seed}");
        projector.send(()).unwrap();
        let synced = tokio::time::timeout(TIMEOUT, async {
            while keys(&sidecar, None).await != expected {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        assert!(synced.await.is_ok(), "seed {seed}: downloads never projected");
    } else {
        assert_eq!(local.len(), persisted.len(), "seed {seed}: captured while unavailable");
    }
}

/// Keep the panics injected into replays out of the test output; report every other.
fn quiet_injected_panics() {
    static ONCE: parking_lot::Once = parking_lot::Once::new();
    ONCE.call_once(|| {
        let report = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if info.payload().downcast_ref::<&str>() != Some(&INJECTED_PANIC) {
                report(info);
            }
        }));
    });
}

/// Runs seeds `0..ATUIN_CHAOS_ITERATIONS` (by default 100), several at once.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_survives_chaos() {
    quiet_injected_panics();
    let iterations: u64 =
        std::env::var("ATUIN_CHAOS_ITERATIONS").ok().and_then(|n| n.parse().ok()).unwrap_or(100);
    let totals = Arc::new(Totals::default());
    let started = std::time::Instant::now();
    let mut seeds = futures::stream::iter(0..iterations)
        .map(|seed| {
            let totals = totals.clone();
            tokio::spawn(iteration(seed, totals))
        })
        .buffer_unordered(6);
    while let Some(done) = seeds.next().await {
        if let Err(err) = done {
            std::panic::resume_unwind(err.into_panic());
        }
    }
    eprintln!("{iterations} chaos iterations in {:?}: {totals:#?}", started.elapsed());
}
