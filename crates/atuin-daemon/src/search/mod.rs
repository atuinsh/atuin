//! Search module for the daemon gRPC search service.
//!
//! This module provides fuzzy search over command history using frizbee.

mod index;

use atuin_common::string::highlighted::FromHighlightedTextProtoError;
use thiserror::Error;

use crate::grpc::common::pb::{self as common};
use crate::grpc::history::pb::IdParseError;
use crate::output_capture::OutputMatch;

// Include the generated proto code
mod proto {
    #![allow(clippy::must_use_candidate, reason = "prost-generated proto code")]

    tonic::include_proto!("search");
}
pub use proto::*;

#[derive(Debug, Error)]
pub enum OutputMatchParseError {
    #[error("output match is missing its history id")]
    MissingHistoryId,
    #[error("output match is missing its output")]
    MissingOutput,
    #[error(transparent)]
    BadHistoryId(#[from] IdParseError),
    #[error(transparent)]
    BadOutput(#[from] FromHighlightedTextProtoError),
}

impl TryFrom<OutputSearchMatch> for OutputMatch {
    type Error = OutputMatchParseError;

    fn try_from(value: OutputSearchMatch) -> Result<Self, Self::Error> {
        let history_id =
            value.history_id.ok_or(OutputMatchParseError::MissingHistoryId)?.try_into()?;
        let output = value.output.ok_or(OutputMatchParseError::MissingOutput)?.try_into()?;
        Ok(Self {
            history_id,
            output,
            score: value.score,
        })
    }
}

/// Longest query the fuzzy matcher will see. Frizbee's `u16` scores overflow (and panic) somewhere
/// past ~2700 needle chars; no real query is anywhere near either limit, so longer input is
/// truncated.
const MAX_QUERY_LEN: usize = 512;

/// Truncate a query to the longest length frizbee can score without panicking in
/// [`frizbee::Matcher::from_query`]. Anything that hands a query to frizbee (including
/// client-side highlighting) must apply this.
#[must_use]
pub fn truncate_query(query: &str) -> &str {
    use atuin_common::string::TruncateCharsExt;
    query.truncate_chars(MAX_QUERY_LEN)
}

// Re-export the index and related types
pub use index::{IndexFilterMode, SearchIndex};

#[cfg(test)]
mod tests {
    use atuin_client::history::HistoryId;
    use atuin_common::string::highlighted::HighlightedTextProto;
    use rstest::rstest;

    use super::*;

    fn history_id(bytes: [u8; 16]) -> common::HistoryId {
        common::HistoryId {
            uuid: Some(common::Uuid {
                value: bytes.to_vec(),
            }),
        }
    }

    fn output(raw: &str) -> HighlightedTextProto {
        HighlightedTextProto {
            open: 0xE000,
            close: 0xE001,
            raw: raw.to_owned(),
        }
    }

    #[rstest]
    fn converts_a_well_formed_match_into_domain_types() {
        let proto = OutputSearchMatch {
            history_id: Some(history_id([1u8; 16])),
            output: Some(output("\u{E000}disk\u{E001} full")),
            score: 0.5,
        };

        let m = OutputMatch::try_from(proto).unwrap();

        assert_eq!(m.history_id, HistoryId::from_bytes([1u8; 16]));
        assert_eq!(m.output.display_plain().to_string(), "disk full");
    }

    #[rstest]
    fn rejects_a_match_missing_its_history_id() {
        let proto = OutputSearchMatch {
            history_id: None,
            output: Some(output("x")),
            score: 0.0,
        };
        assert!(matches!(
            OutputMatch::try_from(proto),
            Err(OutputMatchParseError::MissingHistoryId)
        ));
    }

    #[rstest]
    fn rejects_a_match_missing_its_output() {
        let proto = OutputSearchMatch {
            history_id: Some(history_id([1u8; 16])),
            output: None,
            score: 0.0,
        };
        assert!(matches!(OutputMatch::try_from(proto), Err(OutputMatchParseError::MissingOutput)));
    }

    #[rstest]
    fn surfaces_a_malformed_history_id() {
        let proto = OutputSearchMatch {
            history_id: Some(common::HistoryId { uuid: None }),
            output: Some(output("x")),
            score: 0.0,
        };
        assert!(matches!(
            OutputMatch::try_from(proto),
            Err(OutputMatchParseError::BadHistoryId(_))
        ));
    }

    #[rstest]
    fn surfaces_malformed_output() {
        let proto = OutputSearchMatch {
            history_id: Some(history_id([1u8; 16])),
            output: Some(HighlightedTextProto {
                open: 0xD800,
                close: 0xE001,
                raw: "x".into(),
            }),
            score: 0.0,
        };
        assert!(matches!(OutputMatch::try_from(proto), Err(OutputMatchParseError::BadOutput(_))));
    }
}
