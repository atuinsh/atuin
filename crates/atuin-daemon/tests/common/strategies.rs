//! proptest strategies for adversarial daemon input: what a hostile or buggy shell hook could put
//! on the wire, and what a legitimate but unusual history looks like.

use atuin_client::history::History;
use atuin_daemon::grpc::common::pb::Uuid as WireUuid;
use atuin_daemon::grpc::history::pb::{HistoryId as WireId, StartHistoryRequest};
use atuin_domain::record::CmdOrigin;
use proptest::prelude::*;

/// Command text: realistic lines, arbitrary unicode, control characters, NUL bytes, whitespace
/// only, empty, and multi-kilobyte monsters.
pub fn command() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => "[ -~]{1,120}",
        3 => "[^\\p{Cc}]{0,64}",
        2 => "[\\x00-\\x1f]{1,16}",
        1 => Just(String::new()),
        1 => "[ \\t]{1,32}",
        1 => "[a-z]{1,8}\\x00[a-z]{0,8}",
        1 => "[a-z ]{4000,12000}",
        1 => Just("echo déjà vu ẞ 𝔘𝔫𝔦𝔠𝔬𝔡𝔢 \u{200f}rtl\u{200e} 👩‍👩‍👧‍👧".to_owned()),
    ]
}

pub fn cwd() -> impl Strategy<Value = String> {
    prop_oneof![
        5 => "(/[a-z0-9._-]{1,12}){0,8}/?",
        1 => "C:\\\\[A-Za-z0-9\\\\ ]{0,40}",
        1 => Just("unknown".to_owned()),
        1 => Just(String::new()),
        1 => "[^\\p{Cc}]{0,64}",
    ]
}

/// UUIDs in both spellings the index accepts, plus things that are not UUIDs at all.
pub fn session() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => proptest::array::uniform16(any::<u8>()).prop_map(|b| uuid::Uuid::from_bytes(b).as_simple().to_string()),
        2 => proptest::array::uniform16(any::<u8>()).prop_map(|b| uuid::Uuid::from_bytes(b).as_hyphenated().to_string()),
        2 => "[a-z0-9-]{0,40}",
        1 => Just(String::new()),
    ]
}

/// The `hostname` wire field. Only `host:user` shapes parse; everything else must be rejected.
pub fn hostname() -> impl Strategy<Value = String> {
    prop_oneof![
        5 => "[a-z0-9.-]{1,24}:[a-z_][a-z0-9_-]{0,15}",
        1 => "[a-z]{1,8}:[a-z]{1,8}:[a-z]{1,8}",
        1 => ":[a-z]{0,8}",
        1 => "[a-z]{1,8}:",
        1 => Just(":".to_owned()),
        1 => "[^\\p{Cc}]{0,24}:[^\\p{Cc}]{0,24}",
        2 => "[a-z0-9.-]{0,24}",
        1 => Just(String::new()),
    ]
}

pub fn shell() -> impl Strategy<Value = String> {
    prop_oneof![Just(String::new()), Just("bash".to_owned()), Just("zsh".to_owned()), "[a-z]{1,8}"]
}

pub fn author() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => Just(String::new()),
        2 => "[a-z]{1,12}",
        2 => Just("claude-code".to_owned()),
        1 => "[ \\t]{1,4}",
        1 => "[^\\p{Cc}]{0,32}",
    ]
}

/// Nanosecond timestamps across the supported `[0, i64::MAX]` domain, weighted toward the edges:
/// epoch, now, and the i64 max (~year 2262). The wire and the history db share a signed i64 column
/// and the record store a u64 one, so every non-negative value up to `i64::MAX` round-trips through
/// all three; there is no unstorable value left to probe.
pub fn timestamp_nanos() -> impl Strategy<Value = i64> {
    let now = i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos()).unwrap();
    prop_oneof![
        4 => now - 86_400_000_000_000..now + 86_400_000_000_000,
        1 => Just(0i64),
        1 => Just(i64::MAX),
        1 => 0i64..=i64::MAX,
    ]
}

/// A wire duration: absent, tiny, huge, or invalid (negative, mixed sign, nanos out of range).
pub fn wire_duration() -> impl Strategy<Value = Option<prost_types::Duration>> {
    prop_oneof![
        2 => Just(None),
        3 => (0i64..10_000, 0i32..1_000_000_000).prop_map(|(seconds, nanos)| Some(prost_types::Duration { seconds, nanos })),
        1 => Just(Some(prost_types::Duration { seconds: i64::MAX, nanos: 0 })),
        1 => Just(Some(prost_types::Duration { seconds: 315_576_000_000, nanos: 999_999_999 })),
        1 => Just(Some(prost_types::Duration { seconds: -1, nanos: 0 })),
        1 => Just(Some(prost_types::Duration { seconds: 1, nanos: -1 })),
        1 => Just(Some(prost_types::Duration { seconds: 0, nanos: 1_000_000_000 })),
        1 => any::<(i64, i32)>().prop_map(|(seconds, nanos)| Some(prost_types::Duration { seconds, nanos })),
    ]
}

prop_compose! {
    pub fn start_request()(
        timestamp in timestamp_nanos(),
        command in command(),
        cwd in cwd(),
        session in session(),
        hostname in hostname(),
        author in author(),
        intent in prop_oneof![Just(String::new()), "[^\\p{Cc}]{0,40}"],
        shell in shell(),
        author_kind in prop_oneof![Just(0i32), Just(1), Just(2), any::<i32>()],
    ) -> StartHistoryRequest {
        StartHistoryRequest { timestamp, command, cwd, session, hostname, author, intent, shell, author_kind }
    }
}

/// A wire history id: valid, missing, or the wrong length.
pub fn wire_id() -> impl Strategy<Value = WireId> {
    prop_oneof![
        4 => proptest::array::uniform16(any::<u8>()).prop_map(|b| WireId { uuid: Some(WireUuid { value: b.to_vec() }) }),
        1 => Just(WireId { uuid: None }),
        1 => proptest::collection::vec(any::<u8>(), 0..40).prop_filter("not 16", |v| v.len() != 16)
            .prop_map(|value| WireId { uuid: Some(WireUuid { value }) }),
    ]
}

pub fn is_well_formed(id: &WireId) -> bool {
    id.uuid.as_ref().is_some_and(|u| u.value.len() == 16)
}

/// A domain `History` a well-behaved client could hand the journal: any command/cwd/session, but a
/// parseable origin. For journal-level tests.
pub fn valid_history() -> impl Strategy<Value = History> {
    (command(), cwd(), session(), "[a-z0-9.-]{1,24}:[a-z_][a-z0-9_-]{0,15}", shell()).prop_map(
        |(command, cwd, session, origin, shell)| {
            History::daemon()
                .timestamp(time::OffsetDateTime::now_utc())
                .command(command)
                .cwd(cwd)
                .session(session)
                .cmd_origin(CmdOrigin::try_from(origin).unwrap())
                .shell(shell)
                // Explicit, so a developer shell exporting `ATUIN_HISTORY_AUTHOR` can't turn
                // every generated history into an agent entry the index skips.
                .author("test-user")
                .build()
                .into()
        },
    )
}
