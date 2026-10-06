mod codegen {
    #![allow(clippy::must_use_candidate, clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("atuin.api.v1");
}

use std::time::{Duration, SystemTime};

use atuin_client::history::{CommandCapture, History as DomainHistory, HistoryId};
use atuin_common::string::NonNulStr;
use atuin_domain::record::{CmdOrigin as DomainCmdOrigin, RecordId};
pub use codegen::*;
use prost::Message;
use thiserror::Error;
use uuid::{Uuid, Version};
use xxhash_rust::xxh3::xxh3_128;

#[derive(Debug, Error)]
#[error("history id {0} is not a UUIDv7, so Octavo can't store it")]
pub struct NotUuidV7(pub HistoryId);

impl TryFrom<HistoryId> for UuidV7 {
    type Error = NotUuidV7;

    fn try_from(id: HistoryId) -> Result<Self, Self::Error> {
        let bytes = id.into_bytes();
        if Uuid::from_bytes(bytes).get_version() != Some(Version::SortRand) {
            return Err(NotUuidV7(id));
        }

        Ok(Self {
            value: bytes.to_vec(),
        })
    }
}

impl From<RecordId> for UuidV7 {
    fn from(id: RecordId) -> Self {
        id.0.into()
    }
}

impl From<Uuid> for UuidV7 {
    fn from(uuid: Uuid) -> Self {
        Self {
            value: uuid.into_bytes().to_vec(),
        }
    }
}

impl From<&DomainCmdOrigin> for CmdOrigin {
    fn from(origin: &DomainCmdOrigin) -> Self {
        Self {
            host: NonNulStr::stripping(origin.host().into_inner()).into_inner().into_owned(),
            user: NonNulStr::stripping(origin.user().into_inner()).into_inner().into_owned(),
        }
    }
}

const MAX_AUTHOR_NAME_BYTES: usize = 255;
const MAX_SESSION_BYTES: usize = 128;
const MAX_HISTORY_BYTES: usize = 1 << 20;

#[derive(Debug, Error)]
pub enum HistoryConversionError {
    #[error(transparent)]
    Id(#[from] NotUuidV7),
    #[error("exit code {0} does not fit in 32 bits")]
    ExitOutOfRange(i64),
    #[error("invalid duration: {0}")]
    InvalidDuration(#[from] prost_types::DurationError),
    #[error("the author name is {0} bytes, over the {MAX_AUTHOR_NAME_BYTES} Octavo allows")]
    AuthorTooLong(usize),
    #[error("the entry is {0} bytes, over the {MAX_HISTORY_BYTES} Octavo takes")]
    TooLarge(usize),
}

impl TryFrom<DomainHistory> for History {
    type Error = HistoryConversionError;

    fn try_from(history: DomainHistory) -> Result<Self, Self::Error> {
        let exit = i32::try_from(history.exit)
            .map_err(|_| HistoryConversionError::ExitOutOfRange(history.exit))?;
        let duration = u64::try_from(history.duration)
            .ok()
            .map(|nanos| prost_types::Duration::try_from(Duration::from_nanos(nanos)))
            .transpose()?;
        let kind = if history.is_agent() {
            AuthorKind::Agent
        } else {
            AuthorKind::User
        };
        let author = NonNulStr::stripping(history.author);
        if author.len() > MAX_AUTHOR_NAME_BYTES {
            return Err(HistoryConversionError::AuthorTooLong(author.len()));
        }

        let converted = Self {
            id: Some(history.id.try_into()?),
            record_id: None,
            cmd_origin: Some(CmdOrigin::from(&history.cmd_origin)),
            start_time: Some(SystemTime::from(history.timestamp).into()),
            duration,
            session: session_key(NonNulStr::stripping(history.session).into_inner().into_owned()),
            author: Some(Author {
                name: author.into_inner().into_owned(),
                kind: kind.into(),
            }),
            command: NonNulStr::stripping(history.command).into_inner().into_owned(),
            cwd: NonNulStr::stripping(history.cwd).into_inner().into_owned(),
            exit,
            intent: history
                .intent
                .map(|intent| NonNulStr::stripping(intent).into_inner().into_owned()),
            shell: history.shell.map(|shell| NonNulStr::stripping(shell).into_inner().into_owned()),
        };

        match converted.encoded_len() {
            len if len > MAX_HISTORY_BYTES => Err(HistoryConversionError::TooLarge(len)),
            _ => Ok(converted),
        }
    }
}

/// What `HistoryOutput.start` and `HistoryOutput.end` hold together at most.
const MAX_OUTPUT_BYTES: usize = 1 << 20;

#[derive(Debug, Error)]
#[error("the output is {0} bytes, over the {MAX_OUTPUT_BYTES} Octavo takes")]
pub struct OutputTooLarge(pub usize);

impl TryFrom<CommandCapture> for HistoryOutput {
    type Error = OutputTooLarge;

    fn try_from(capture: CommandCapture) -> Result<Self, Self::Error> {
        let len = capture.output_start.len() + capture.output_end.as_ref().map_or(0, String::len);
        if len > MAX_OUTPUT_BYTES {
            return Err(OutputTooLarge(len));
        }

        Ok(Self {
            start: capture.output_start,
            end: capture.output_end,
            observed_bytes: capture.output_observed_bytes,
            terminal_width: capture.terminal_width.into(),
            terminal_height: capture.terminal_height.into(),
        })
    }
}

/// `session` as `History.session` can carry it: as is when it fits the bytes the field allows, or
/// else as the hex of its 128-bit xxh3 hash, which keeps two long sessions apart.
fn session_key(session: String) -> String {
    if session.len() <= MAX_SESSION_BYTES {
        return session;
    }

    format!("{:032x}", xxh3_128(session.as_bytes()))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn capture(start: String, end: Option<String>) -> CommandCapture {
        CommandCapture {
            output_start: start,
            output_end: end,
            output_observed_bytes: 4_096,
            terminal_width: 80,
            terminal_height: 24,
        }
    }

    #[rstest]
    #[case::kept_whole("\x1b[31mfile\x1b[0m\nline two", None)]
    #[case::truncated("first lines", Some("last lines"))]
    #[case::no_output("", None)]
    fn keeps_every_field(#[case] start: &str, #[case] end: Option<&str>) {
        let output = HistoryOutput::try_from(capture(start.to_owned(), end.map(str::to_owned)))
            .expect("well under the cap");

        assert_eq!(output, HistoryOutput {
            start: start.to_owned(),
            end: end.map(str::to_owned),
            observed_bytes: 4_096,
            terminal_width: 80,
            terminal_height: 24,
        });
    }

    #[rstest]
    #[case::start_at_the_cap(MAX_OUTPUT_BYTES, None, true)]
    #[case::start_past_the_cap(MAX_OUTPUT_BYTES + 1, None, false)]
    #[case::halves_at_the_cap(MAX_OUTPUT_BYTES / 2, Some(MAX_OUTPUT_BYTES / 2), true)]
    #[case::halves_past_the_cap(MAX_OUTPUT_BYTES / 2, Some(MAX_OUTPUT_BYTES / 2 + 1), false)]
    fn caps_start_and_end_together(
        #[case] start: usize,
        #[case] end: Option<usize>,
        #[case] fits: bool,
    ) {
        let converted =
            HistoryOutput::try_from(capture("a".repeat(start), end.map(|end| "z".repeat(end))));

        match converted {
            Ok(_) => assert!(fits, "output past the cap was accepted"),
            Err(OutputTooLarge(len)) => {
                assert!(!fits, "output within the cap was refused");
                assert_eq!(len, start + end.unwrap_or(0));
            }
        }
    }
}
