//! Head-to-head for the `start_cmd` hot path in `history_journal.rs`.
//!
//! Every started command runs through `start_cmd`. The map insert is identical either way; the
//! only difference is whether we deep-clone the `History` (all its heap strings) for a broadcast
//! that, in the common case, has no tail subscriber.
//!
//!   - `start_cmd_clone` (OLD): `history.clone()` into the map, then move the original into the
//!     (receiver-less) broadcast.
//!   - `start_cmd_guarded` (NEW): guard the broadcast on `receiver_count() > 0` (as `finish()`
//!     already does) and move the `History` straight into the map -- no clone in the common case.
//!
//! Inputs are identical `History` values; each sample inserts into a fresh empty map so only the
//! clone-vs-move difference is timed.

use std::sync::Arc;

use atuin_client::history::History;
use atuin_client::history::HistoryId;
use atuin_daemon::CmdEvent;
use atuin_domain::record::CmdOrigin;
use dashmap::DashMap;
use tokio::sync::broadcast;
use tracing::Span;

fn main() {
    divan::main();
}

/// Faithful stand-in for the private `history_journal::InFlightCmd`.
struct InFlightCmd {
    #[allow(dead_code)]
    history: History,
    #[allow(dead_code)]
    span: Span,
    #[allow(dead_code)]
    finalization_mutex: Arc<tokio::sync::Mutex<()>>,
}

/// A realistic started command. `long` models an agent-style entry: a big pipeline, a deep cwd and
/// a rationale, so the deep clone touches several sizeable heap strings.
fn history(long: bool) -> History {
    let (command, cwd, intent): (&str, &str, &str) = if long {
        (
            "git log --oneline --graph --decorate --all | rg -n 'perf|fix' | \
             awk '{print $1}' | xargs -I{} git show --stat {} | less -R",
            "/home/user/src/company/monorepo/services/backend/crates/really-deep/nested/module",
            "inspecting recent perf and fix commits across the whole history to \
             understand a regression before bisecting",
        )
    } else {
        ("git status", "/home/user/src/atuin", "")
    };

    History::daemon()
        .timestamp(time::OffsetDateTime::now_utc())
        .command(command)
        .cwd(cwd)
        .session("018f3d2a4b7c7e9d8a1f0c2b3d4e5f60")
        .cmd_origin(CmdOrigin::try_from("workstation.example.com:marko").unwrap())
        .shell("zsh")
        .author("marko")
        .intent(intent)
        .build()
        .into()
}

#[divan::bench(args = [false, true])]
fn start_cmd_clone(bencher: divan::Bencher, long: bool) {
    // No live receivers: the tail consumer isn't subscribed, matching the common case.
    let (broadcast, _) = broadcast::channel::<CmdEvent>(128);
    let template = history(long);

    bencher
        .with_inputs(|| (DashMap::<HistoryId, InFlightCmd>::new(), template.clone()))
        .bench_values(|(map, history)| {
            let id = history.id;
            map.insert(id, InFlightCmd {
                history: history.clone(),
                span: Span::none(),
                finalization_mutex: Arc::new(tokio::sync::Mutex::new(())),
            });
            let _ = broadcast.send(CmdEvent::Started(history));
            divan::black_box(map)
        });
}

#[divan::bench(args = [false, true])]
fn start_cmd_guarded(bencher: divan::Bencher, long: bool) {
    let (broadcast, _) = broadcast::channel::<CmdEvent>(128);
    let template = history(long);

    bencher
        .with_inputs(|| (DashMap::<HistoryId, InFlightCmd>::new(), template.clone()))
        .bench_values(|(map, history)| {
            let id = history.id;
            if broadcast.receiver_count() > 0 {
                let _ = broadcast.send(CmdEvent::Started(history.clone()));
            }
            map.insert(id, InFlightCmd {
                history,
                span: Span::none(),
                finalization_mutex: Arc::new(tokio::sync::Mutex::new(())),
            });
            divan::black_box(map)
        });
}
