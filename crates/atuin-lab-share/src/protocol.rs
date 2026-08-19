//! Minimal Phoenix channel codec, JSON serializer only (`vsn=2.0.0`).
//!
//! Frames are JSON arrays `[join_ref, ref, topic, event, payload]`. Terminal
//! bytes cross the wire as base64 strings in a `data` field, so the Phoenix
//! *binary* serializer is never needed.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::Value;

/// Encode raw terminal bytes for a JSON `data` field.
#[must_use]
pub(crate) fn b64_encode(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

/// Decode a JSON `data` field back into raw terminal bytes.
pub(crate) fn b64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    B64.decode(s)
}

/// One outbound Phoenix frame: `[join_ref, ref, topic, event, payload]`.
///
/// A channel join is not a separate frame shape — it is the same array with the
/// event `"phx_join"` — so a single [`PhoenixPush::encode`] covers joins and
/// data/control pushes alike.
pub(crate) struct PhoenixPush<'a> {
    pub(crate) join_ref: &'a str,
    pub(crate) ref_: &'a str,
    pub(crate) topic: &'a str,
    pub(crate) event: &'a str,
    pub(crate) payload: &'a Value,
}

impl PhoenixPush<'_> {
    /// Encode as a Phoenix v2 JSON array.
    #[must_use]
    pub(crate) fn encode(&self) -> String {
        serde_json::json!([self.join_ref, self.ref_, self.topic, self.event, self.payload])
            .to_string()
    }
}

/// Encode the periodic heartbeat.
#[must_use]
pub(crate) fn encode_heartbeat(ref_: &str) -> String {
    serde_json::json!([Value::Null, ref_, "phoenix", "heartbeat", {}]).to_string()
}

/// A decoded inbound Phoenix frame.
#[derive(Debug)]
pub(crate) enum Incoming {
    /// `phx_reply`: the ack for a push, matched to it by `ref_`. `ok` is the
    /// `"status"` field; `response` its payload.
    Reply {
        ref_: String,
        ok: bool,
        response: Value,
    },
    /// A server-initiated channel event (`output` requests, `participants`, …).
    Event {
        event: String,
        payload: Value,
    },
    /// `phx_error`: the channel process crashed; the client must rejoin.
    Error {
        // `parse` fills this in from the `phx_error` payload so the variant
        // carries the hub's explanation, but the transport only reacts to the
        // *fact* of a channel error (`Incoming::Error { .. }` → rejoin), so
        // nothing in-crate reads it. Kept because it is part of the decoded
        // Phoenix frame and is what `Debug` prints when a rejoin is logged.
        #[allow(dead_code, reason = "decoded protocol detail, no in-crate reader")]
        reason: Value,
    },
    /// `phx_close`: the channel closed cleanly.
    Close,
    /// Anything unrecognized — including short or empty arrays; see [`Incoming::parse`].
    Other,
}

impl Incoming {
    /// Parse an inbound Phoenix v2 JSON frame.
    ///
    /// The tolerant `Value` indexing is deliberate: a short or empty array
    /// yields [`Incoming::Other`] (ignored upstream) rather than an error, so
    /// only malformed JSON is an `Err`.
    pub(crate) fn parse(raw: &str) -> Result<Self, serde_json::Error> {
        let v: Value = serde_json::from_str(raw)?;
        let event = v[3].as_str().unwrap_or_default();
        let payload = v[4].clone();
        Ok(match event {
            "phx_reply" => Self::Reply {
                ref_: v[1].as_str().unwrap_or_default().to_string(),
                ok: payload["status"] == "ok",
                response: payload["response"].clone(),
            },
            "phx_error" => Self::Error { reason: payload },
            "phx_close" => Self::Close,
            "" => Self::Other,
            ev => Self::Event {
                event: ev.to_string(),
                payload,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // Encoders: compare *parsed* values, not raw strings — the wire format is
    // "a JSON array with these five elements", not a particular byte sequence.

    #[test]
    fn encode_join_produces_the_phoenix_v2_array() {
        // A join is a plain push whose event is `phx_join`.
        let payload = json!({ "write": true });
        let frame = PhoenixPush {
            join_ref: "1",
            ref_: "1",
            topic: "share:host",
            event: "phx_join",
            payload: &payload,
        }
        .encode();
        let v: Value = serde_json::from_str(&frame).expect("valid JSON");
        assert_eq!(v, json!(["1", "1", "share:host", "phx_join", { "write": true }]));
    }

    #[test]
    fn encode_push_produces_the_phoenix_v2_array() {
        let payload = json!({ "seq": 3, "data": "aGk=" });
        let frame = PhoenixPush {
            join_ref: "1",
            ref_: "7",
            topic: "share:host",
            event: "output",
            payload: &payload,
        }
        .encode();
        let v: Value = serde_json::from_str(&frame).expect("valid JSON");
        assert_eq!(v, json!(["1", "7", "share:host", "output", { "seq": 3, "data": "aGk=" }]));
    }

    #[test]
    fn encode_heartbeat_targets_the_phoenix_topic_with_null_join_ref() {
        let v: Value = serde_json::from_str(&encode_heartbeat("42")).expect("valid JSON");
        assert_eq!(v, json!([null, "42", "phoenix", "heartbeat", {}]));
    }

    #[test]
    fn decode_maps_phx_reply_ok() {
        let raw = r#"["1","1","share:host","phx_reply",{"status":"ok","response":{"token":"t"}}]"#;
        match Incoming::parse(raw).expect("decodes") {
            Incoming::Reply { ref_, ok, response } => {
                assert_eq!(ref_, "1");
                assert!(ok);
                assert_eq!(response, json!({ "token": "t" }));
            }
            other => panic!("expected Reply, got {other:?}"),
        }
    }

    #[test]
    fn decode_maps_phx_reply_error_status_to_not_ok() {
        let raw =
            r#"["1","9","share:host","phx_reply",{"status":"error","response":{"reason":"no"}}]"#;
        match Incoming::parse(raw).expect("decodes") {
            Incoming::Reply { ref_, ok, response } => {
                assert_eq!(ref_, "9");
                assert!(!ok);
                assert_eq!(response, json!({ "reason": "no" }));
            }
            other => panic!("expected Reply, got {other:?}"),
        }
    }

    #[test]
    fn decode_maps_phx_error_carrying_the_payload_as_reason() {
        let raw = r#"["1",null,"share:host","phx_error",{"why":"crash"}]"#;
        match Incoming::parse(raw).expect("decodes") {
            Incoming::Error { reason } => assert_eq!(reason, json!({ "why": "crash" })),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn decode_maps_phx_close() {
        let raw = r#"["1","1","share:host","phx_close",{}]"#;
        assert!(matches!(Incoming::parse(raw).expect("decodes"), Incoming::Close));
    }

    #[test]
    fn decode_maps_unknown_events_to_event() {
        let raw = r#"[null,null,"share:host","participants",{"count":3}]"#;
        match Incoming::parse(raw).expect("decodes") {
            Incoming::Event { event, payload } => {
                assert_eq!(event, "participants");
                assert_eq!(payload, json!({ "count": 3 }));
            }
            other => panic!("expected Event, got {other:?}"),
        }
    }

    // The tolerant `Value` indexing is deliberate: a short or empty array
    // yields `Other` (ignored upstream) rather than an error.

    #[test]
    fn decode_tolerates_empty_and_short_arrays_as_other() {
        assert!(matches!(Incoming::parse("[]").expect("decodes"), Incoming::Other));
        assert!(matches!(Incoming::parse(r#"["1","2"]"#).expect("decodes"), Incoming::Other));
        assert!(matches!(Incoming::parse("null").expect("decodes"), Incoming::Other));
    }

    #[test]
    fn decode_rejects_malformed_json() {
        assert!(Incoming::parse("not json").is_err());
    }

    #[test]
    fn b64_round_trips_terminal_bytes() {
        let bytes: Vec<u8> = (0u8..=255).collect();
        assert_eq!(b64_decode(&b64_encode(&bytes)).expect("decodes"), bytes);
    }

    #[test]
    fn b64_is_standard_alphabet_with_padding() {
        // The hub decodes with Elixir's standard Base64; a URL-safe or unpadded
        // alphabet here would break the wire format.
        assert_eq!(b64_encode(b"hello"), "aGVsbG8=");
        assert!(b64_decode("not base64!").is_err());
    }
}
