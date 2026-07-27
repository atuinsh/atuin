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
pub fn b64_encode(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

/// Decode a JSON `data` field back into raw terminal bytes.
pub fn b64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    B64.decode(s)
}

/// Encode a `phx_join` frame.
#[must_use]
pub fn encode_join(join_ref: &str, ref_: &str, topic: &str, payload: Value) -> String {
    serde_json::json!([join_ref, ref_, topic, "phx_join", payload]).to_string()
}

/// Encode a data/control push on an already-joined channel.
#[must_use]
pub fn encode_push(join_ref: &str, ref_: &str, topic: &str, event: &str, payload: Value) -> String {
    serde_json::json!([join_ref, ref_, topic, event, payload]).to_string()
}

/// Encode the periodic heartbeat.
#[must_use]
pub fn encode_heartbeat(ref_: &str) -> String {
    serde_json::json!([Value::Null, ref_, "phoenix", "heartbeat", {}]).to_string()
}

/// A decoded inbound Phoenix frame.
#[derive(Debug)]
pub enum Incoming {
    Reply {
        ref_: String,
        ok: bool,
        response: Value,
    },
    Event {
        event: String,
        payload: Value,
    },
    Error {
        // `decode` fills this in from the `phx_error` payload so the variant
        // carries the hub's explanation, but the transport only reacts to the
        // *fact* of a channel error (`Incoming::Error { .. }` → rejoin), so
        // nothing in-crate reads it. Kept because it is part of the decoded
        // Phoenix frame and is what `Debug` prints when a rejoin is logged.
        #[allow(dead_code, reason = "decoded protocol detail, no in-crate reader")]
        reason: Value,
    },
    Close,
    Other,
}

/// Parse an inbound Phoenix v2 JSON frame.
pub fn decode(raw: &str) -> Result<Incoming, serde_json::Error> {
    let v: Value = serde_json::from_str(raw)?;
    let event = v[3].as_str().unwrap_or_default();
    let payload = v[4].clone();
    Ok(match event {
        "phx_reply" => Incoming::Reply {
            ref_: v[1].as_str().unwrap_or_default().to_string(),
            ok: payload["status"] == "ok",
            response: payload["response"].clone(),
        },
        "phx_error" => Incoming::Error { reason: payload },
        "phx_close" => Incoming::Close,
        "" => Incoming::Other,
        ev => Incoming::Event {
            event: ev.to_string(),
            payload,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_encodes_as_phoenix_v2_array() {
        let msg = encode_join("1", "1", "share:host", serde_json::json!({ "write": true }));
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v[0], "1"); // join_ref
        assert_eq!(v[1], "1"); // ref
        assert_eq!(v[2], "share:host"); // topic
        assert_eq!(v[3], "phx_join"); // event
        assert_eq!(v[4]["write"], true); // payload
    }

    #[test]
    fn heartbeat_uses_null_join_ref_and_phoenix_topic() {
        let msg = encode_heartbeat("7");
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert!(v[0].is_null());
        assert_eq!(v[1], "7");
        assert_eq!(v[2], "phoenix");
        assert_eq!(v[3], "heartbeat");
    }

    #[test]
    fn decode_reply_ok_extracts_response() {
        let raw = r#"["1","1","share:host","phx_reply",{"status":"ok","response":{"token":"abc","join_url":"u"}}]"#;
        match decode(raw).unwrap() {
            Incoming::Reply { ref_, ok, response } => {
                assert_eq!(ref_, "1");
                assert!(ok);
                assert_eq!(response["token"], "abc");
            }
            other => panic!("expected reply, got {other:?}"),
        }
    }

    #[test]
    fn decode_event_extracts_event_and_payload() {
        let raw = r#"["1","5","share:host","set_size",{"cols":80,"rows":24}]"#;
        match decode(raw).unwrap() {
            Incoming::Event { event, payload } => {
                assert_eq!(event, "set_size");
                assert_eq!(payload["rows"], 24);
            }
            other => panic!("expected event, got {other:?}"),
        }
    }

    #[test]
    fn base64_round_trips_raw_bytes() {
        let bytes = [0u8, 27, 91, 255, 10];
        assert_eq!(b64_decode(&b64_encode(&bytes)).unwrap(), bytes);
    }
}
