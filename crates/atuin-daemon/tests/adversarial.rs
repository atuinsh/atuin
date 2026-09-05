//! Anything a shell hook (or an attacker on the socket) can put on the wire either round-trips
//! byte-for-byte into the history db, or is rejected with `InvalidArgument`. The daemon never
//! panics, never half-applies a batch, and never loses a command it accepted.
#![cfg(unix)]

mod common;

use std::time::Duration;

use atuin_client::history::{AuthorKind, HistoryId};
use atuin_common::time::OffsetDateTimeExt;
use atuin_common::utils::normalize_optional_string;
use atuin_daemon::grpc::common::pb::Uuid as WireUuid;
use atuin_daemon::grpc::history::pb::{
    CancelHistoryRequest, DeleteHistoryRequest, EndHistoryRequest, HistoryId as WireId,
    StartHistoryRequest,
};
use atuin_domain::record::CmdOrigin;
use common::{SharedEnv, TestEnv, history, strategies};
use proptest::prelude::*;
use rstest::*;
use tonic::Code;

fn tame_request() -> StartHistoryRequest {
    StartHistoryRequest {
        timestamp: i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos()).unwrap(),
        command: "echo tame".into(),
        cwd: "/tmp".into(),
        session: uuid::Uuid::now_v7().as_simple().to_string(),
        hostname: "host:user".into(),
        author: String::new(),
        intent: String::new(),
        shell: "bash".into(),
        author_kind: 0,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(160))]

    /// Start + end for any wire request: rejected cleanly iff the origin lacks a `:` or the
    /// duration is invalid; otherwise every field lands in the db exactly as sent.
    #[test]
    fn any_start_end_pair_round_trips_or_fails_cleanly(
        req in strategies::start_request(),
        exit in any::<i64>(),
        duration in strategies::wire_duration(),
    ) {
        let shared = SharedEnv::get();
        shared.block_on(async {
            let env = shared.env();
            let mut raw = env.raw_history_client().await;
            let origin = CmdOrigin::try_from(req.hostname.clone());

            let reply = match raw.start_history(req.clone()).await {
                Err(status) => {
                    prop_assert_eq!(status.code(), Code::InvalidArgument, "{}", status);
                    prop_assert!(origin.is_err(), "only a colonless hostname may be rejected");
                    return Ok(());
                }
                Ok(reply) => reply.into_inner(),
            };
            prop_assert!(origin.is_ok());
            let id: HistoryId = reply.id.unwrap().try_into().unwrap();
            prop_assert!(env.journal.get(id).is_ok());

            let valid_duration = duration.map(Duration::try_from).transpose().is_ok();
            let ended = raw
                .end_history(EndHistoryRequest { id: Some(id.into()), exit, duration })
                .await;
            match ended {
                Err(status) => {
                    prop_assert_eq!(status.code(), Code::InvalidArgument, "{}", status);
                    prop_assert!(!valid_duration);
                    prop_assert!(env.journal.get(id).is_ok(), "a rejected end leaves the command in flight");
                    raw.cancel_history(CancelHistoryRequest { id: Some(id.into()) }).await.unwrap();
                    return Ok(());
                }
                Ok(_) => prop_assert!(valid_duration, "an invalid duration must be rejected"),
            }

            // The history table has `unique(timestamp, cwd, command)` and is written with `insert
            // or ignore`, so an accepted command that exactly repeats an earlier row's triple is
            // deduped by the schema rather than stored twice (see
            // `an_exact_repeat_is_deduped_by_the_history_db`). Edge timestamps make that likely
            // across cases; a deduped command is still accounted for by the row it collided with.
            let Some(row) = env.history_db.load(id).await.unwrap() else {
                let twins = env.rows_with_triple(req.timestamp, &req.cwd, &req.command).await;
                prop_assert!(twins >= 1, "accepted command is neither persisted nor deduped");
                return Ok(());
            };
            prop_assert_eq!(&row.command, &req.command);
            prop_assert_eq!(&row.cwd, &req.cwd);
            prop_assert_eq!(&row.session, &req.session);
            prop_assert_eq!(row.cmd_origin.as_str(), req.hostname.as_str());
            prop_assert_eq!(row.exit, exit);
            prop_assert_eq!(row.timestamp, time::OffsetDateTime::from_unix_nanos_i64(req.timestamp));
            // `History::new` falls back to `ATUIN_HISTORY_AUTHOR`/`ATUIN_HISTORY_INTENT` when the
            // wire field is unset, so a developer shell exporting either must not fail this test.
            let expected_intent = normalize_optional_string(Some(req.intent.clone()))
                .or_else(|| normalize_optional_string(std::env::var("ATUIN_HISTORY_INTENT").ok()));
            prop_assert_eq!(&row.intent, &expected_intent);
            prop_assert_eq!(&row.shell, &normalize_optional_string(Some(req.shell.clone())));
            let expected_author = normalize_optional_string(Some(req.author.clone()))
                .or_else(atuin_client::history::probe_author)
                .unwrap_or_else(|| row.cmd_origin.user().to_string());
            prop_assert_eq!(&row.author, &expected_author);
            let expected_kind = match req.author_kind {
                1 => Some(AuthorKind::User),
                2 => Some(AuthorKind::Agent),
                _ => None,
            };
            prop_assert_eq!(row.author_kind, expected_kind);
            match duration.map(|d| Duration::try_from(d).unwrap()) {
                Some(d) => prop_assert_eq!(row.duration, i64::try_from(d.as_nanos()).unwrap_or(i64::MAX)),
                None => prop_assert!(row.duration >= 0),
            }
            Ok(())
        })?;
    }

    /// A delete batch is validated as a whole: one malformed id rejects the batch and nothing in
    /// it is deleted; a well-formed batch deletes exactly its persisted members.
    #[test]
    fn delete_batches_are_all_or_nothing(
        ids in proptest::collection::vec(strategies::wire_id(), 0..8),
        persist_mask in proptest::collection::vec(any::<bool>(), 8),
    ) {
        let shared = SharedEnv::get();
        shared.block_on(async {
            let env = shared.env();
            let mut client = env.history_client().await;
            let mut raw = env.raw_history_client().await;

            // Replace some well-formed ids with ids of rows we persist first.
            let mut ids = ids;
            let mut persisted = Vec::new();
            for (i, id) in ids.iter_mut().enumerate() {
                if persist_mask[i] && strategies::is_well_formed(id) {
                    let real = env.record(&mut client, &format!("echo batch {i}")).await;
                    *id = real.into();
                    persisted.push(real);
                }
            }

            let result = raw.delete_history(DeleteHistoryRequest { ids: ids.clone() }).await;
            let all_well_formed = ids.iter().all(strategies::is_well_formed);
            match result {
                Err(status) => {
                    prop_assert!(!all_well_formed, "well-formed batch rejected: {}", status);
                    prop_assert_eq!(status.code(), Code::InvalidArgument);
                    for id in &persisted {
                        prop_assert!(env.history_db.load(*id).await.unwrap().is_some(), "{id} deleted despite rejection");
                    }
                }
                Ok(reply) => {
                    prop_assert!(all_well_formed);
                    prop_assert_eq!(reply.into_inner().deleted, u64::try_from(ids.len()).unwrap());
                    for id in &persisted {
                        prop_assert!(env.history_db.load(*id).await.unwrap().is_none(), "{id} survived");
                    }
                }
            }
            Ok(())
        })?;
    }

}

/// Whether `command`, used verbatim as a search query, is free of the daemon's fzf-like query DSL
/// (`frizbee::Pattern::parse`): a leading `!`/`^`/`'`, a trailing `$`, or a backslash change how
/// the query is parsed instead of being matched literally, and leading/trailing whitespace is
/// trimmed by the atom splitter before `Pattern::parse` ever sees it. Embedded whitespace is fine:
/// it splits the query into multiple AND'd atoms, but each still has to fuzzy-match somewhere in
/// the same command text, so a multi-word command still finds itself.
fn command_is_dsl_safe(command: &str) -> bool {
    !command.is_empty()
        && command.trim() == command
        && !command.chars().any(|c| matches!(c, '!' | '^' | '\'' | '$' | '\\'))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Whatever the journal is handed as a `History`, the search index and db agree afterwards:
    /// the row exists, and it is the index's one and only entry iff it is index-eligible. Runs on a
    /// fresh daemon per case, so the index holds exactly this command. Search by the command's own
    /// text whenever that text is safe to use as a literal query (see [`command_is_dsl_safe`]) so
    /// the assertion actually exercises content-based search, not just presence; fall back to an
    /// empty query (which matches every candidate, ranked by frecency) for the commands that would
    /// otherwise be reinterpreted by the query DSL instead of matched literally — on a fresh
    /// single-row env that still deterministically surfaces exactly this id.
    #[test]
    fn any_valid_history_persists_and_indexes_consistently(h in strategies::valid_history()) {
        common::current_thread_runtime().block_on(async {
            let env = TestEnv::builder().build().await;
            let id = env.journal.start_cmd(h.clone());
            env.journal.finish(id, 0, Duration::from_millis(1)).await.unwrap();

            let stored = env.history_db.load(id).await.unwrap().expect("row persisted");
            prop_assert_eq!(&stored.command, &h.command);
            let eligible = common::corpus::index_eligible(&h);
            prop_assert_eq!(env.index_count().await, usize::from(eligible));
            let query = if command_is_dsl_safe(&h.command) { h.command.as_str() } else { "" };
            let hits = env.index_hits(query).await;
            let expected = if eligible { vec![id] } else { vec![] };
            prop_assert_eq!(hits, expected, "command {:?}, query {:?}", h.command, query);
            Ok(())
        })?;
    }
}

/// Field-level boundaries a proptest can miss because they depend on one exact value. The wire and
/// the history db share the same signed i64 nanosecond column, so both edges of the supported
/// `[0, i64::MAX]` domain round-trip through start + end and persist, and the daemon never crashes
/// on the cast.
#[rstest]
#[case::epoch(0)]
#[case::i64_max(i64::MAX)]
#[tokio::test]
async fn timestamp_edges_never_crash_the_daemon(#[case] timestamp: i64) {
    let env = TestEnv::builder().build().await;
    let mut raw = env.raw_history_client().await;
    let req = StartHistoryRequest {
        timestamp,
        ..tame_request()
    };

    let id: HistoryId =
        raw.start_history(req).await.unwrap().into_inner().id.unwrap().try_into().unwrap();
    raw.end_history(EndHistoryRequest {
        id: Some(id.into()),
        exit: 0,
        duration: None,
    })
    .await
    .unwrap();
    assert!(env.history_db.load(id).await.unwrap().is_some());

    // The daemon is still alive and serving.
    let mut client = env.history_client().await;
    assert!(client.status().await.unwrap().healthy);
}

/// Command payload sizes: comfortably inside the 4 MiB gRPC default, and just past it, which must
/// be a clean rejection rather than a hang or crash.
#[rstest]
#[case::sixty_four_kib(64 * 1024, true)]
#[case::one_mib(1024 * 1024, true)]
#[case::five_mib(5 * 1024 * 1024, false)]
#[tokio::test]
async fn huge_commands_round_trip_or_are_rejected(#[case] bytes: usize, #[case] accepted: bool) {
    let env = TestEnv::builder().build().await;
    let mut raw = env.raw_history_client().await;
    let req = StartHistoryRequest {
        command: "x".repeat(bytes),
        ..tame_request()
    };

    let result = raw.start_history(req).await;
    assert_eq!(result.is_ok(), accepted, "{result:?}");
    if let Err(status) = &result {
        assert_eq!(status.code(), Code::OutOfRange, "{status}");
    }
    if let Ok(reply) = result {
        let id: HistoryId = reply.into_inner().id.unwrap().try_into().unwrap();
        raw.end_history(EndHistoryRequest {
            id: Some(id.into()),
            exit: 0,
            duration: None,
        })
        .await
        .unwrap();
        assert_eq!(env.history_db.load(id).await.unwrap().unwrap().command.len(), bytes);
    }
    assert!(env.history_client().await.status().await.unwrap().healthy);
}

/// Origins that parse but look odd must be stored verbatim and split at the first colon.
#[rstest]
#[case::multi_colon("a:b:c", "a", "b:c")]
#[case::empty_host(":user", "", "user")]
#[case::empty_user("host:", "host", "")]
#[case::lone_colon(":", "", "")]
#[case::unicode("ホスト:ユーザー", "ホスト", "ユーザー")]
#[tokio::test]
async fn odd_origins_are_stored_verbatim(
    #[case] hostname: &str,
    #[case] host: &str,
    #[case] user: &str,
) {
    let env = TestEnv::builder().build().await;
    let mut raw = env.raw_history_client().await;
    let req = StartHistoryRequest {
        hostname: hostname.into(),
        ..tame_request()
    };
    let id: HistoryId =
        raw.start_history(req).await.unwrap().into_inner().id.unwrap().try_into().unwrap();
    raw.end_history(EndHistoryRequest {
        id: Some(id.into()),
        exit: 0,
        duration: None,
    })
    .await
    .unwrap();

    let row = env.history_db.load(id).await.unwrap().unwrap();
    assert_eq!(row.cmd_origin.as_str(), hostname);
    assert_eq!(row.cmd_origin.host().as_ref(), host);
    assert_eq!(row.cmd_origin.user().as_ref(), user);
}

/// Lifecycle RPCs against ids the daemon does not know, or knows in the wrong state.
#[rstest]
#[case::end_unknown(Op::End, Target::Unknown, Code::NotFound)]
#[case::cancel_unknown(Op::Cancel, Target::Unknown, Code::NotFound)]
#[case::end_twice(Op::End, Target::Ended, Code::NotFound)]
#[case::cancel_ended(Op::Cancel, Target::Ended, Code::NotFound)]
#[case::end_cancelled(Op::End, Target::Cancelled, Code::NotFound)]
#[case::end_malformed(Op::End, Target::Malformed, Code::InvalidArgument)]
#[case::cancel_malformed(Op::Cancel, Target::Malformed, Code::InvalidArgument)]
#[case::end_missing(Op::End, Target::Missing, Code::InvalidArgument)]
#[tokio::test]
async fn wrong_state_lifecycle_calls_fail_with_the_right_code(
    #[case] op: Op,
    #[case] target: Target,
    #[case] expected: Code,
) {
    let env = TestEnv::builder().build().await;
    let mut client = env.history_client().await;
    let mut raw = env.raw_history_client().await;
    let id: Option<WireId> = match target {
        Target::Unknown => Some(HistoryId::from_bytes([7u8; 16]).into()),
        Target::Ended => Some(env.record(&mut client, "echo ended").await.into()),
        Target::Cancelled => {
            let id: HistoryId = client
                .start_history(history("echo cancelled"))
                .await
                .unwrap()
                .id
                .unwrap()
                .try_into()
                .unwrap();
            client.cancel_history(id).await.unwrap();
            Some(id.into())
        }
        Target::Malformed => Some(WireId {
            uuid: Some(WireUuid {
                value: vec![1, 2, 3],
            }),
        }),
        Target::Missing => None,
    };
    let status = match op {
        Op::End => raw
            .end_history(EndHistoryRequest {
                id,
                exit: 0,
                duration: None,
            })
            .await
            .unwrap_err(),
        Op::Cancel => raw.cancel_history(CancelHistoryRequest { id }).await.unwrap_err(),
    };
    assert_eq!(status.code(), expected, "{status}");
}

#[derive(Debug, Clone, Copy)]
enum Op {
    End,
    Cancel,
}

#[derive(Debug, Clone, Copy)]
enum Target {
    Unknown,
    Ended,
    Cancelled,
    Malformed,
    Missing,
}

/// Two accepted commands with the same `(timestamp, cwd, command)`: the history table's unique
/// index on that triple plus `insert or ignore` (both older than the daemon) mean the second is
/// reported as recorded but never lands in the db. Pinned here so the proptest above can rely on
/// it; if the daemon ever starts rejecting or replacing the duplicate, update both.
#[tokio::test]
async fn an_exact_repeat_is_deduped_by_the_history_db() {
    let env = TestEnv::builder().build().await;
    let mut raw = env.raw_history_client().await;
    let mut req = tame_request();
    req.timestamp = 0;
    req.cwd = String::new();
    req.command = String::new();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let reply = raw.start_history(req.clone()).await.unwrap().into_inner();
        let id: HistoryId = reply.id.unwrap().try_into().unwrap();
        raw.end_history(EndHistoryRequest {
            id: Some(id.into()),
            exit: 0,
            duration: None,
        })
        .await
        .unwrap();
        ids.push(id);
    }
    assert_ne!(ids[0], ids[1]);
    assert!(env.history_db.load(ids[0]).await.unwrap().is_some(), "first row is stored");
    assert!(env.history_db.load(ids[1]).await.unwrap().is_none(), "exact repeat is deduped");
    assert_eq!(env.rows_with_triple(0, "", "").await, 1);
}
