//! Phoenix channel client over a WebSocket (JSON serializer, `vsn=2.0.0`).
//!
//! The session loop lives on plain OS threads talking over `std::sync::mpsc`,
//! so the transport owns a **dedicated current-thread tokio runtime in its own
//! OS thread**. It must never build a runtime on the calling thread: `atuin
//! lab share` is dispatched from inside `runtime.block_on(run_inner(..))` and a
//! nested runtime panics with *"Cannot start a runtime from within a runtime"*.
//!
//! A transport drop never kills the subshell: a lost socket only produces
//! `Inbound::Disconnected` and a backoff-ed reconnect that resumes the *same*
//! hub session via the secret `host_resume_token`.

use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio_tungstenite::tungstenite::Message;

use crate::backpressure::{Backoff, OutboundQueue};
use crate::protocol::{
    Incoming, b64_decode, b64_encode, decode, encode_heartbeat, encode_join, encode_push,
};
use crate::session::{Inbound, Outbound};

/// The host's channel topic.
const TOPIC: &str = "share:host";
/// Phoenix `join_ref`; also the `ref` of the join push, so join replies are
/// identifiable. Every later push uses a strictly greater ref.
const JOIN_REF: &str = "1";
/// Phoenix heartbeat cadence.
const HEARTBEAT: Duration = Duration::from_secs(30);
/// How many `output` frames may pile up before the backlog is collapsed into a
/// keyframe request. ~8 KiB per frame, so a couple of MiB at most.
const OUTBOUND_CAP: usize = 256;

/// Build the hub's WebSocket URL.
#[must_use]
pub fn ws_url(hub_url: &str, api_token: &str) -> String {
    format!("{hub_url}/sockets/share/websocket?vsn=2.0.0&token={api_token}")
}

/// Build the `phx_join` payload.
///
/// `resume_token` is the **secret** `host_resume_token` handed back in the join
/// reply — never the public share token, which is in the link and would let
/// anyone holding it hijack the host role.
#[must_use]
pub fn join_payload(write: bool, resume_token: Option<&str>) -> Value {
    match resume_token {
        Some(t) => json!({ "write": write, "resume_token": t }),
        None => json!({ "write": write }),
    }
}

/// Map a hub → CLI event onto an `Inbound`. Unknown events yield `None` and are
/// ignored, so the hub can add events without breaking older clients.
fn to_inbound(event: &str, payload: &Value) -> Option<Inbound> {
    match event {
        "input" => b64_decode(payload["data"].as_str().unwrap_or_default())
            .ok()
            .map(Inbound::Input),
        "set_size" => Some(Inbound::SetSize {
            cols: u16::try_from(payload["cols"].as_u64()?).ok()?,
            rows: u16::try_from(payload["rows"].as_u64()?).ok()?,
        }),
        "participants" => Some(Inbound::Participants(
            u32::try_from(payload["count"].as_u64()?).ok()?,
        )),
        "request_keyframe" => Some(Inbound::RequestKeyframe),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error)]
enum TransportError {
    /// Boxed: `tungstenite::Error` is ~136 bytes, and every `Result` in the hot
    /// relay path would otherwise carry that (`clippy::result_large_err`).
    #[error("websocket: {0}")]
    Ws(Box<tokio_tungstenite::tungstenite::Error>),
    #[error("hub rejected the channel join: {0}")]
    JoinRejected(String),
    #[error("connection closed by the hub")]
    Closed,
}

impl From<tokio_tungstenite::tungstenite::Error> for TransportError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::Ws(Box::new(e))
    }
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;

/// The write half of a joined channel, plus its Phoenix `ref` counter.
struct Wire {
    sink: WsSink,
    next_ref: u64,
}

impl Wire {
    fn take_ref(&mut self) -> String {
        let r = self.next_ref;
        self.next_ref = self.next_ref.saturating_add(1);
        r.to_string()
    }

    async fn push(&mut self, event: &str, payload: Value) -> Result<(), TransportError> {
        let r = self.take_ref();
        let frame = encode_push(JOIN_REF, &r, TOPIC, event, payload);
        self.sink.send(Message::Text(frame)).await?;
        Ok(())
    }

    async fn heartbeat(&mut self) -> Result<(), TransportError> {
        let r = self.take_ref();
        self.sink.send(Message::Text(encode_heartbeat(&r))).await?;
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), TransportError> {
        self.sink.flush().await?;
        Ok(())
    }
}

/// Client state that must survive reconnects.
struct Client {
    hub_url: String,
    api_token: String,
    write: bool,
    /// The **secret** host credential from the join reply. Never the public
    /// token.
    host_resume_token: Option<String>,
    /// The last public token the hub gave us, used to notice that a resume was
    /// refused and a brand-new session was created behind our back.
    last_public_token: Option<String>,
    /// Reports the join URL to `run_share` on the FIRST successful join, so the
    /// URL can go into the subshell's `ATUIN_SHARE_URL` before the shell spawns.
    /// Taken (set to `None`) after firing once; reconnects leave the already
    /// spawned shell's environment untouched.
    first_url_tx: Option<Sender<String>>,
    backoff: Backoff,
}

impl Client {
    /// Reconnect forever until the session ends for good.
    async fn run(&mut self, out_rx: &mut UnboundedReceiver<Outbound>, in_tx: &Sender<Inbound>) {
        let mut reported = false;
        loop {
            match self.connect_once(out_rx, in_tx).await {
                // The session is over (`end`, or the session dropped its
                // sender). Do not reconnect — the link is meant to die.
                Ok(()) => return,
                Err(e) => {
                    // Report the first failure only: a misconfigured hub URL or
                    // token would otherwise fail silently forever. Later
                    // failures stay quiet so a flaky link does not spam the
                    // composited screen.
                    if !reported {
                        reported = true;
                        eprintln!("\r\n[atuin lab share] hub connection failed: {e}\r");
                    }
                    // The subshell keeps running; the session only repaints.
                    if in_tx.send(Inbound::Disconnected).is_err() {
                        return; // session gone
                    }
                    tokio::time::sleep(self.backoff.next_delay()).await;
                }
            }
        }
    }

    /// One connection: join, then relay until the socket or the session ends.
    ///
    /// `Ok(())` means the session finished for good; `Err` means reconnect.
    async fn connect_once(
        &mut self,
        out_rx: &mut UnboundedReceiver<Outbound>,
        in_tx: &Sender<Inbound>,
    ) -> Result<(), TransportError> {
        let (ws, _resp) =
            tokio_tungstenite::connect_async(ws_url(&self.hub_url, &self.api_token)).await?;
        let (sink, mut stream) = ws.split();
        let mut wire = Wire { sink, next_ref: 2 };

        let join = encode_join(
            JOIN_REF,
            JOIN_REF,
            TOPIC,
            join_payload(self.write, self.host_resume_token.as_deref()),
        );
        wire.sink.send(Message::Text(join)).await?;

        // A fresh queue per connection: anything the session produced while we
        // were disconnected is still sitting in `out_rx`, and draining it below
        // is exactly what trips the overflow into a keyframe request.
        let mut queue = OutboundQueue::new(OUTBOUND_CAP);
        let mut heartbeat = tokio::time::interval(HEARTBEAT);
        heartbeat.tick().await; // the first tick completes immediately

        loop {
            tokio::select! {
                _ = heartbeat.tick() => wire.heartbeat().await?,

                frame = stream.next() => {
                    let Some(frame) = frame else {
                        return Err(TransportError::Closed);
                    };
                    match frame? {
                        Message::Text(txt) => self.handle_text(&txt, in_tx)?,
                        Message::Close(_) => return Err(TransportError::Closed),
                        // Pongs are answered by tungstenite itself; binary and
                        // raw frames are not part of this protocol.
                        Message::Ping(_) | Message::Pong(_)
                        | Message::Binary(_) | Message::Frame(_) => {}
                    }
                }

                item = out_rx.recv() => {
                    // The session dropped its sender: nothing left to send.
                    let Some(item) = item else { return Ok(()) };
                    // Absorb everything queued while the last send was in
                    // flight, so the backlog is measured (and collapsed) in one
                    // place rather than dribbling out frame by frame.
                    let mut batch = vec![item];
                    while let Ok(more) = out_rx.try_recv() {
                        batch.push(more);
                    }
                    for it in batch {
                        if handle_outbound(it, &mut queue, &mut wire, in_tx).await? {
                            wire.flush().await?;
                            return Ok(());
                        }
                    }
                    flush_queue(&mut queue, &mut wire).await?;
                }
            }
        }
    }

    fn handle_text(&mut self, raw: &str, in_tx: &Sender<Inbound>) -> Result<(), TransportError> {
        // Malformed frames are ignored rather than fatal: a garbled message is
        // no reason to tear down a working session.
        let Ok(msg) = decode(raw) else { return Ok(()) };
        match msg {
            Incoming::Reply { ref_, ok, response } if ref_ == JOIN_REF => {
                if !ok {
                    return Err(TransportError::JoinRejected(response.to_string()));
                }
                self.on_joined(&response, in_tx);
            }
            // Acks for our own pushes and for heartbeats; nothing to do.
            Incoming::Reply { .. } | Incoming::Other => {}
            Incoming::Event { event, payload } => {
                if let Some(inbound) = to_inbound(&event, &payload) {
                    let _ = in_tx.send(inbound);
                }
            }
            Incoming::Error { .. } | Incoming::Close => return Err(TransportError::Closed),
        }
        Ok(())
    }

    fn on_joined(&mut self, response: &Value, in_tx: &Sender<Inbound>) {
        let token = response["token"].as_str().unwrap_or_default().to_string();
        let join_url = response["join_url"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // Store the SECRET resume credential. Never send the public `token` as
        // `resume_token`: it is in the share link, so anyone holding the link
        // could otherwise hijack the host role.
        self.host_resume_token = response["host_resume_token"].as_str().map(str::to_string);

        // Hand the join URL to `run_share` once, on the first join, so it lands
        // in the subshell's environment (`ATUIN_SHARE_URL`) before the shell
        // starts. A later reconnect can't retroactively change a running
        // shell's environment, so we deliberately only fire here.
        if let Some(tx) = self.first_url_tx.take() {
            let _ = tx.send(join_url.clone());
        }

        // "The session we were using was replaced." True only when we had
        // already advertised a public token and the hub handed back a different
        // one — i.e. a resume was rejected or expired and the hub silently made
        // a new session, leaving the old link dead.
        //
        // Deliberately FALSE on the first join. The hub always mints a session
        // there, so a plain "did the token change?" test is true and the first
        // thing a new user sees would be "Reconnected as a NEW session — the
        // previous link is dead", which is both wrong and alarming.
        let fresh_session = self
            .last_public_token
            .as_deref()
            .is_some_and(|previous| previous != token);
        self.last_public_token = Some(token.clone());

        // A completed join is what "connected" means, so a later outage starts
        // retrying promptly again.
        self.backoff.reset();

        let _ = in_tx.send(Inbound::Connected {
            token,
            join_url,
            fresh_session,
        });
    }
}

/// Send everything the queue holds, in `seq` order.
///
/// A no-op while a resync keyframe is outstanding: after an overflow the hub's
/// replay buffer has a gap, and nothing may precede the keyframe that closes it
/// — including the flushes done by the `host_size` and `end` paths.
async fn flush_queue(queue: &mut OutboundQueue, wire: &mut Wire) -> Result<(), TransportError> {
    if queue.awaiting_keyframe() {
        return Ok(());
    }
    for (seq, data) in queue.drain_output() {
        wire.push("output", json!({ "seq": seq, "data": b64_encode(&data) }))
            .await?;
    }
    Ok(())
}

/// Handle one item from the session. Returns `true` when the session is over.
async fn handle_outbound(
    item: Outbound,
    queue: &mut OutboundQueue,
    wire: &mut Wire,
    in_tx: &Sender<Inbound>,
) -> Result<bool, TransportError> {
    match item {
        Outbound::Output { seq, data } => {
            queue.push_output(seq, data);
            if queue.needs_keyframe() {
                // The backlog was collapsed. Replaying it would desync every
                // viewer, and synthesising a keyframe *here* would break the
                // seq invariant (a keyframe's payload and `seq` must be minted
                // together under the parser lock). So ask the session for one;
                // it answers immediately, even if the child never writes again.
                //
                // `clear_keyframe_flag` only stops us re-asking: the queue stays
                // in `awaiting_keyframe`, discarding output, until that keyframe
                // is actually written — otherwise we would send frames sitting
                // on the far side of the gap we just created.
                queue.drain_output();
                queue.clear_keyframe_flag();
                let _ = in_tx.send(Inbound::RequestKeyframe);
            }
        }
        Outbound::Keyframe { seq, data } => {
            flush_queue(queue, wire).await?;
            wire.push("keyframe", json!({ "seq": seq, "data": b64_encode(&data) }))
                .await?;
            // Ends any resync window opened by an overflow: output queued after
            // this keyframe carries a greater `seq`, so the hub's buffer is
            // contiguous again.
            queue.on_keyframe_sent();
        }
        Outbound::HostSize { cols, rows } => {
            flush_queue(queue, wire).await?;
            wire.push("host_size", json!({ "cols": cols, "rows": rows }))
                .await?;
        }
        Outbound::End => {
            flush_queue(queue, wire).await?;
            wire.push("end", json!({})).await?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Start the hub transport.
///
/// Spawns an OS thread owning a current-thread tokio runtime that connects,
/// joins `share:host`, heartbeats, relays events both ways, and reconnects with
/// exponential backoff — resuming the same session with the secret
/// `host_resume_token`. Returns immediately; the session keeps running (and the
/// subshell keeps living) whether or not the hub is reachable.
pub fn spawn_transport(
    hub_url: String,
    api_token: String,
    write: bool,
    out_rx: Receiver<Outbound>,
    in_tx: Sender<Inbound>,
    url_tx: Sender<String>,
) {
    std::thread::spawn(move || {
        // The session speaks blocking `std::sync::mpsc`; bridge it onto an
        // async channel so the client loop can select over it without blocking
        // the runtime thread.
        let (bridge_tx, mut bridge_rx) = unbounded_channel::<Outbound>();
        std::thread::spawn(move || {
            while let Ok(item) = out_rx.recv() {
                if bridge_tx.send(item).is_err() {
                    return;
                }
            }
        });

        // rustls has no default crypto provider until one is installed; every
        // other TLS user in the workspace goes through this helper.
        atuin_common::tls::ensure_crypto_provider();

        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("\r\n[atuin lab share] could not start the transport runtime: {e}\r");
                return;
            }
        };

        let mut client = Client {
            hub_url,
            api_token,
            write,
            host_resume_token: None,
            last_public_token: None,
            first_url_tx: Some(url_tx),
            backoff: Backoff::new(),
        };
        runtime.block_on(client.run(&mut bridge_rx, &in_tx));
    });
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn builds_phoenix_ws_url() {
        let u = ws_url("wss://hub.atuin.sh", "APITOKEN");
        assert_eq!(
            u,
            "wss://hub.atuin.sh/sockets/share/websocket?vsn=2.0.0&token=APITOKEN"
        );
    }

    #[test]
    fn join_payload_is_create_then_resume() {
        let create = join_payload(true, None);
        assert_eq!(create["write"], true);
        assert!(create.get("resume_token").is_none());

        // On reconnect the payload carries the SECRET host_resume_token — never
        // the public share token.
        let resume = join_payload(true, Some("secret-host-token"));
        assert_eq!(resume["resume_token"], "secret-host-token");
        assert_eq!(resume["write"], true);
    }

    /// A `Client` with no socket, for exercising the join-reply bookkeeping.
    fn test_client() -> Client {
        Client {
            hub_url: "ws://localhost:4000".into(),
            api_token: "atapi_test".into(),
            write: false,
            host_resume_token: None,
            last_public_token: None,
            first_url_tx: None,
            backoff: Backoff::new(),
        }
    }

    #[test]
    fn first_join_reports_the_url_exactly_once() {
        use std::time::Duration;

        let (url_tx, url_rx) = mpsc::channel::<String>();
        let mut client = Client {
            first_url_tx: Some(url_tx),
            ..test_client()
        };
        let (in_tx, _in_rx) = mpsc::channel::<Inbound>();

        let reply = json!({
            "token": "pub1",
            "join_url": "http://h/lab/share/pub1",
            "host_resume_token": "secret1"
        });
        client.on_joined(&reply, &in_tx);

        // A reconnect (second join) must NOT push another URL: the shell's
        // environment was fixed at spawn and can't be changed now.
        let reply2 = json!({
            "token": "pub2",
            "join_url": "http://h/lab/share/pub2",
            "host_resume_token": "secret2"
        });
        client.on_joined(&reply2, &in_tx);

        assert_eq!(
            url_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "http://h/lab/share/pub1"
        );
        assert!(
            url_rx.try_recv().is_err(),
            "the join URL must be reported exactly once"
        );
    }

    #[test]
    fn first_connect_is_not_reported_as_a_replaced_session() {
        // The hub always mints a session on a first (non-resume) join, so a
        // naive "token changed?" check is true here and the very first thing a
        // new user sees is "Reconnected as a NEW session — the previous link is
        // dead". There was no previous link; the warning is both wrong and
        // alarming. It must only fire when a resume was actually rejected.
        let (tx, rx) = mpsc::channel::<Inbound>();
        let mut client = test_client();

        client.on_joined(
            &json!({ "token": "pub1", "join_url": "u/pub1", "host_resume_token": "secret1" }),
            &tx,
        );

        match rx.try_recv() {
            Ok(Inbound::Connected { fresh_session, .. }) => assert!(
                !fresh_session,
                "a first connect must not be announced as a replaced session"
            ),
            other => panic!("expected Connected, got {:?}", other.is_ok()),
        }
    }

    #[test]
    fn reconnect_with_a_changed_token_is_reported_as_a_replaced_session() {
        // A rejected/expired resume makes the hub silently mint a new session
        // with a new public token. The old link is dead, so this one *must*
        // warn — otherwise the host keeps advertising a URL nobody can join.
        let (tx, rx) = mpsc::channel::<Inbound>();
        let mut client = test_client();

        client.on_joined(
            &json!({ "token": "pub1", "join_url": "u/pub1", "host_resume_token": "secret1" }),
            &tx,
        );
        let _ = rx.try_recv();

        // Same session resumed: same public token, no warning.
        client.on_joined(
            &json!({ "token": "pub1", "join_url": "u/pub1", "host_resume_token": "secret1" }),
            &tx,
        );
        match rx.try_recv() {
            Ok(Inbound::Connected { fresh_session, .. }) => {
                assert!(!fresh_session, "resuming the same session must not warn");
            }
            other => panic!("expected Connected, got {:?}", other.is_ok()),
        }

        // Resume rejected: the hub hands back a different public token.
        client.on_joined(
            &json!({ "token": "pub2", "join_url": "u/pub2", "host_resume_token": "secret2" }),
            &tx,
        );
        match rx.try_recv() {
            Ok(Inbound::Connected {
                fresh_session,
                join_url,
                ..
            }) => {
                assert!(fresh_session, "a replaced session must warn");
                assert_eq!(join_url, "u/pub2", "the new link must be the one printed");
            }
            other => panic!("expected Connected, got {:?}", other.is_ok()),
        }
    }

    #[test]
    fn maps_hub_events_onto_inbound() {
        match to_inbound("input", &json!({ "data": b64_encode(b"ls\n") })) {
            Some(Inbound::Input(bytes)) => assert_eq!(bytes, b"ls\n"),
            _ => panic!("input must decode to raw bytes"),
        }
        match to_inbound("set_size", &json!({ "cols": 80, "rows": 24 })) {
            Some(Inbound::SetSize { cols, rows }) => {
                assert_eq!((cols, rows), (80, 24));
            }
            _ => panic!("set_size must map to SetSize"),
        }
        match to_inbound("participants", &json!({ "count": 3 })) {
            Some(Inbound::Participants(n)) => assert_eq!(n, 3),
            _ => panic!("participants must map to Participants"),
        }
        assert!(matches!(
            to_inbound("request_keyframe", &json!({})),
            Some(Inbound::RequestKeyframe)
        ));
        // Unknown events are ignored, not fatal (forward compatibility).
        assert!(to_inbound("something_new", &json!({})).is_none());
    }
}
