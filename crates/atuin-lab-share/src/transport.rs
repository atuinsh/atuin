//! Phoenix channel client over a WebSocket (JSON serializer, `vsn=2.0.0`).
//!
//! The whole transport is the [`Transport`] struct: [`Transport::new`] builds
//! it, [`Transport::run`] is the relay loop. `run_share` drives it with
//! `tokio::spawn(transport.run(..))` on the CLI's existing runtime — no thread
//! or runtime of its own. (The session it talks to is itself a future on that
//! same runtime.)
//!
//! A transport drop never kills the subshell: a lost socket only produces
//! `Inbound::Disconnected` and a backoff-ed reconnect that resumes the *same*
//! hub session via the secret `host_resume_token`.
//!
//! The transport is also the E2EE seam ([`crate::crypto`]): it owns the
//! per-session key, seals every outbound `output`/`keyframe` payload, opens
//! (and replay-checks) inbound `input` blobs before the session sees them, and
//! appends the key fragment to the join URL at the single point that URL
//! enters the program. The session and `ScreenState` stay key-free.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::backpressure::{Backoff, Frame, OutboundQueue};
use crate::crypto::{self, FrameKind, NONCE_LEN, SessionKey};
use crate::protocol::{Incoming, PhoenixPush, b64_encode, encode_heartbeat};
use crate::render::WriteMode;
use crate::session::{Inbound, Outbound};

/// The host's channel topic.
const TOPIC: &str = "share:host";
/// Phoenix `join_ref`; also the `ref` of the join push, so join replies are
/// identifiable. Every later push uses a strictly greater ref — see
/// [`RefSequence`].
const JOIN_REF: &str = "1";
/// Phoenix heartbeat cadence.
const HEARTBEAT: Duration = Duration::from_secs(30);
/// How many `output` frames may pile up before the backlog is collapsed into a
/// keyframe request. ~8 KiB per frame, so a couple of MiB at most.
const OUTBOUND_CAP: usize = 256;
/// How many recently accepted viewer-input nonces to remember for replay
/// dedup — see [`NonceWindow`]. 12 KiB of nonces plus the set: negligible.
const INPUT_NONCE_CAP: usize = 1024;

/// The hub's share WebSocket endpoint path, appended to any path already on
/// the base URL (a hub behind a reverse-proxy path prefix keeps working).
const WS_PATH: &str = "/sockets/share/websocket";
/// Query key selecting the Phoenix serializer version.
const WS_VSN_KEY: &str = "vsn";
/// Phoenix serializer version: v2 JSON array frames.
const WS_VSN: &str = "2.0.0";
/// Query key carrying the API token.
const WS_TOKEN_KEY: &str = "token";

/// Phoenix's channel-join control event.
const EVENT_JOIN: &str = "phx_join";
/// Host → hub push events.
const EVENT_OUTPUT: &str = "output";
const EVENT_KEYFRAME: &str = "keyframe";
const EVENT_HOST_SIZE: &str = "host_size";
const EVENT_END: &str = "end";

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

/// The Phoenix `ref` counter for one connection's pushes.
///
/// Starts one past [`JOIN_REF`] (the join push's own `ref`), so every later
/// push carries a strictly greater ref and the join reply stays unambiguous.
struct RefSequence {
    next: u64,
}

impl RefSequence {
    fn new() -> Self {
        let join_ref: u64 = JOIN_REF.parse().expect("JOIN_REF is a numeric literal");
        Self { next: join_ref + 1 }
    }

    /// Take the next ref, advancing the counter.
    fn take(&mut self) -> String {
        let r = self.next;
        self.next = self.next.saturating_add(1);
        r.to_string()
    }
}

/// A bounded sliding window of the nonces of recently accepted viewer-input
/// blobs.
///
/// Viewers are anonymous and multiple write-mode viewers interleave, so no
/// per-sender monotonic-seq rule is possible for the input direction (input
/// AAD uses the constant seq 0). Instead: an **exact** hub replay re-delivers
/// the same 12-byte nonce, and this window drops it. Only nonces of blobs that
/// *authenticated* are recorded — garbage blobs must not be able to evict real
/// nonces and reopen a replay. Forgery is impossible regardless (every blob is
/// AEAD-authenticated); reordering/delay by the hub remains an availability
/// attack, as for all frames.
struct NonceWindow {
    /// Insertion order, for evicting the oldest once full.
    order: VecDeque<[u8; NONCE_LEN]>,
    /// Membership, for the O(1) replay check.
    seen: HashSet<[u8; NONCE_LEN]>,
}

impl NonceWindow {
    fn new() -> Self {
        Self {
            order: VecDeque::with_capacity(INPUT_NONCE_CAP),
            seen: HashSet::with_capacity(INPUT_NONCE_CAP),
        }
    }

    /// Whether `nonce` was already recorded (i.e. this blob is a replay).
    fn contains(&self, nonce: &[u8; NONCE_LEN]) -> bool {
        self.seen.contains(nonce)
    }

    /// Record an accepted nonce, evicting the oldest once the window holds
    /// [`INPUT_NONCE_CAP`] of them.
    fn record(&mut self, nonce: [u8; NONCE_LEN]) {
        if self.order.len() == INPUT_NONCE_CAP
            && let Some(oldest) = self.order.pop_front()
        {
            self.seen.remove(&oldest);
        }
        if self.seen.insert(nonce) {
            self.order.push_back(nonce);
        }
    }
}

/// The `output` push payload: envelope `seq` in the clear (the hub orders,
/// buffers, and replays on it), `data` = b64(nonce || ciphertext || tag)
/// sealed under the session key with the seq-bound Output AAD — so a hub that
/// renumbers or splices frames causes an authentication failure on the viewer
/// instead of corrupted content.
fn output_payload(key: &SessionKey, frame: &Frame) -> Value {
    let blob = key.encrypt(
        &frame.data,
        &crypto::frame_aad(FrameKind::Output, frame.seq),
    );
    json!({ "seq": frame.seq, "data": b64_encode(&blob) })
}

/// The `keyframe` push payload; same shape as [`output_payload`] with the
/// Keyframe AAD, so a keyframe blob can never be reflected as output (or vice
/// versa).
fn keyframe_payload(key: &SessionKey, frame: &Frame) -> Value {
    let blob = key.encrypt(
        &frame.data,
        &crypto::frame_aad(FrameKind::Keyframe, frame.seq),
    );
    json!({ "seq": frame.seq, "data": b64_encode(&blob) })
}

/// The write half of a joined channel, plus its Phoenix `ref` counter.
struct Wire {
    sink: WsSink,
    refs: RefSequence,
}

impl Wire {
    /// Wrap the sink of a fresh connection. See [`RefSequence`] for where the
    /// push counter starts relative to the join.
    fn new(sink: WsSink) -> Self {
        Self {
            sink,
            refs: RefSequence::new(),
        }
    }

    /// Send a pre-encoded frame as-is. Only the join push uses this: its `ref`
    /// is [`JOIN_REF`] itself, not one taken from the counter.
    async fn send_raw(&mut self, frame: String) -> Result<(), TransportError> {
        self.sink.send(Message::Text(frame)).await?;
        Ok(())
    }

    /// Push one event to the host topic, stamped with the next `ref`.
    async fn push(&mut self, event: &str, payload: Value) -> Result<(), TransportError> {
        let r = self.refs.take();
        let frame = PhoenixPush {
            join_ref: JOIN_REF,
            ref_: &r,
            topic: TOPIC,
            event,
            payload: &payload,
        }
        .encode();
        self.send_raw(frame).await
    }

    /// Send the periodic Phoenix heartbeat.
    async fn heartbeat(&mut self) -> Result<(), TransportError> {
        let r = self.refs.take();
        let frame = encode_heartbeat(&r);
        self.send_raw(frame).await
    }

    /// Flush the underlying WebSocket sink.
    async fn flush(&mut self) -> Result<(), TransportError> {
        self.sink.flush().await?;
        Ok(())
    }
}

/// One connection's write side: the joined [`Wire`] and the outbound queue
/// whose lifetime matches it.
///
/// The queue is **fresh per connection** on purpose: anything the session
/// produced while we were disconnected is still sitting in `out_rx`, and
/// draining that backlog through a new queue is exactly what trips the
/// overflow into a keyframe request — one resync frame instead of megabytes of
/// stale replay.
struct Connection {
    wire: Wire,
    queue: OutboundQueue,
}

impl Connection {
    /// Pair a fresh queue with the sink of a just-established connection.
    fn new(sink: WsSink) -> Self {
        Self {
            wire: Wire::new(sink),
            queue: OutboundQueue::new(OUTBOUND_CAP),
        }
    }

    /// Push the channel join. It goes out with `join_ref` as its `ref` (see
    /// [`JOIN_REF`]); [`Wire::new`] started the push counter just past it.
    async fn join(&mut self, payload: &Value) -> Result<(), TransportError> {
        let frame = PhoenixPush {
            join_ref: JOIN_REF,
            ref_: JOIN_REF,
            topic: TOPIC,
            event: EVENT_JOIN,
            payload,
        }
        .encode();
        self.wire.send_raw(frame).await
    }

    /// Relay one batch of session items: the item that woke the select arm,
    /// plus everything queued behind it while the last send was in flight —
    /// absorbed here so the backlog is measured (and collapsed) in one place
    /// rather than dribbling out frame by frame. Returns `true` when the
    /// session is over.
    async fn relay_batch(
        &mut self,
        first: Outbound,
        out_rx: &mut UnboundedReceiver<Outbound>,
        in_tx: &UnboundedSender<Inbound>,
        key: &SessionKey,
    ) -> Result<bool, TransportError> {
        let mut batch = vec![first];
        while let Ok(more) = out_rx.try_recv() {
            batch.push(more);
        }
        for item in batch {
            if self.handle_outbound(item, in_tx, key).await? {
                self.wire.flush().await?;
                return Ok(true);
            }
        }
        self.flush(key).await?;
        Ok(false)
    }

    /// Send everything the queue holds, in `seq` order, sealing each frame's
    /// `data` under the session key on its way out ([`output_payload`]).
    ///
    /// A no-op while a resync keyframe is outstanding: after an overflow the
    /// hub's replay buffer has a gap, and nothing may precede the keyframe that
    /// closes it — including the flushes done by the `host_size` and `end`
    /// paths.
    async fn flush(&mut self, key: &SessionKey) -> Result<(), TransportError> {
        if self.queue.awaiting_keyframe() {
            return Ok(());
        }
        for frame in self.queue.drain_output() {
            self.wire
                .push(EVENT_OUTPUT, output_payload(key, &frame))
                .await?;
        }
        Ok(())
    }

    /// Handle one item from the session. Returns `true` when the session is
    /// over.
    async fn handle_outbound(
        &mut self,
        item: Outbound,
        in_tx: &UnboundedSender<Inbound>,
        key: &SessionKey,
    ) -> Result<bool, TransportError> {
        match item {
            Outbound::Output(frame) => {
                self.queue.push_output(frame);
                if self.queue.needs_keyframe() {
                    // The backlog was collapsed. Replaying it would desync every
                    // viewer, and synthesising a keyframe *here* would break the
                    // seq invariant (a keyframe's payload and `seq` must be
                    // minted together by the session's single owner). So ask the
                    // session for one; it answers immediately, even if the child
                    // never writes again.
                    //
                    // `clear_keyframe_flag` only stops us re-asking: the queue
                    // stays in `awaiting_keyframe`, discarding output, until that
                    // keyframe is actually written — otherwise we would send
                    // frames sitting on the far side of the gap we just created.
                    self.queue.drain_output();
                    self.queue.clear_keyframe_flag();
                    let _ = in_tx.send(Inbound::RequestKeyframe);
                }
            }
            Outbound::Keyframe(frame) => {
                self.flush(key).await?;
                // A re-sent keyframe (hub request, rejoin repaint, overflow
                // resync) reaches this arm as a fresh `Frame` minted by the
                // session — it is re-encrypted here, never cached as
                // ciphertext, so a fresh `seq` and a fresh random nonce always
                // travel together.
                self.wire
                    .push(EVENT_KEYFRAME, keyframe_payload(key, &frame))
                    .await?;
                // Ends any resync window opened by an overflow: output queued
                // after this keyframe carries a greater `seq`, so the hub's
                // buffer is contiguous again.
                self.queue.on_keyframe_sent();
            }
            Outbound::HostSize { cols, rows } => {
                self.flush(key).await?;
                self.wire
                    .push(EVENT_HOST_SIZE, json!({ "cols": cols, "rows": rows }))
                    .await?;
            }
            Outbound::End => {
                self.flush(key).await?;
                self.wire.push(EVENT_END, json!({})).await?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// The hub transport: connects, joins `share:host`, relays events both ways, and
/// reconnects with exponential backoff — resuming the same session with the
/// secret `host_resume_token`.
///
/// The fields are the state that must survive reconnects.
pub(crate) struct Transport {
    hub_url: Url,
    api_token: String,
    write: WriteMode,
    /// The per-session E2EE key, owned (and zeroized on drop) here: the
    /// transport is the only place plaintext meets the wire. On a
    /// reconnect-as-new-session the same key is reused under the new token —
    /// same process, `seq` continues monotonically, and random nonces make
    /// reuse safe.
    key: SessionKey,
    /// `key`'s URL-fragment encoding, computed once in [`Transport::new`] so
    /// [`Transport::on_joined`] does not re-encode on every (re)join.
    key_fragment: String,
    /// Replay dedup for viewer input. Lives here — not on the per-connection
    /// state — so a hub replay across a reconnect is still caught.
    input_nonces: NonceWindow,
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
    first_url_tx: Option<oneshot::Sender<String>>,
    backoff: Backoff,
}

impl Transport {
    /// Build a transport. It takes ownership of the session `key` (there is
    /// exactly one owner, so zeroize-on-drop means something); `first_url_tx`
    /// receives the join URL — key fragment included — on the first successful
    /// join; the reconnect state starts empty.
    pub(crate) fn new(
        hub_url: Url,
        api_token: String,
        write: WriteMode,
        key: SessionKey,
        first_url_tx: oneshot::Sender<String>,
    ) -> Self {
        let key_fragment = key.to_fragment();
        Self {
            hub_url,
            api_token,
            write,
            key,
            key_fragment,
            input_nonces: NonceWindow::new(),
            host_resume_token: None,
            last_public_token: None,
            first_url_tx: Some(first_url_tx),
            backoff: Backoff::new(),
        }
    }

    /// Relay until the session ends, reconnecting with backoff in between.
    ///
    /// Runs as a task on the caller's tokio runtime (`tokio::spawn`) — it needs
    /// no thread or runtime of its own. The session keeps running (and the
    /// subshell keeps living) whether or not the hub is reachable.
    pub(crate) async fn run(
        mut self,
        mut out_rx: UnboundedReceiver<Outbound>,
        in_tx: UnboundedSender<Inbound>,
    ) {
        let mut reported = false;
        loop {
            match self.connect_once(&mut out_rx, &in_tx).await {
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
        in_tx: &UnboundedSender<Inbound>,
    ) -> Result<(), TransportError> {
        let (ws, _resp) = tokio_tungstenite::connect_async(self.ws_url().as_str()).await?;
        let (sink, mut stream) = ws.split();

        // Per-connection write state (wire + fresh queue), joined immediately.
        let mut conn = Connection::new(sink);
        conn.join(&self.join_payload()).await?;

        let mut heartbeat = tokio::time::interval(HEARTBEAT);
        heartbeat.tick().await; // the first tick completes immediately

        loop {
            tokio::select! {
                _ = heartbeat.tick() => conn.wire.heartbeat().await?,

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
                    if conn.relay_batch(item, out_rx, in_tx, &self.key).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// The hub's channel WebSocket endpoint, derived from the base `hub_url`:
    /// [`WS_PATH`] is **appended** to any path on the base (never replaces it
    /// — `wss://example.com/hub` must keep its `/hub` prefix), and the query
    /// is rebuilt from scratch.
    fn ws_url(&self) -> Url {
        let mut url = self.hub_url.clone();
        let prefix = url.path().trim_end_matches('/').to_string();
        url.set_path(&format!("{prefix}{WS_PATH}"));
        url.query_pairs_mut()
            .clear()
            .append_pair(WS_VSN_KEY, WS_VSN)
            .append_pair(WS_TOKEN_KEY, &self.api_token);
        url
    }

    /// The `phx_join` payload.
    ///
    /// The resume token is the **secret** `host_resume_token` handed back in the
    /// join reply — never the public share token, which is in the link and would
    /// let anyone holding it hijack the host role.
    fn join_payload(&self) -> Value {
        let write = self.write.is_write_enabled();
        match self.host_resume_token.as_deref() {
            Some(t) => json!({ "write": write, "resume_token": t }),
            None => json!({ "write": write }),
        }
    }

    fn handle_text(
        &mut self,
        raw: &str,
        in_tx: &UnboundedSender<Inbound>,
    ) -> Result<(), TransportError> {
        // Malformed frames are ignored rather than fatal: a garbled message is
        // no reason to tear down a working session.
        let Ok(msg) = Incoming::parse(raw) else {
            return Ok(());
        };
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
                if let Some(inbound) = Inbound::from_event(&event, &payload) {
                    // `from_event` stays keyless: for `input` it hands back
                    // the sealed blob, opened (and replay-checked) here before
                    // the session ever sees it. A blob that fails to open is
                    // dropped silently — nothing unauthenticated may ever
                    // travel toward the PTY.
                    let inbound = match inbound {
                        Inbound::Input(blob) => match self.decrypt_input(&blob) {
                            Some(plaintext) => Inbound::Input(plaintext),
                            None => return Ok(()),
                        },
                        other => other,
                    };
                    let _ = in_tx.send(inbound);
                }
            }
            Incoming::Error { .. } | Incoming::Close => return Err(TransportError::Closed),
        }
        Ok(())
    }

    /// Open a sealed viewer-input blob: replay-check its nonce, authenticate
    /// and decrypt it (input AAD is `frame_aad(Input, 0)` — viewers are
    /// anonymous, so there is no per-sender seq to bind), and record the nonce
    /// only once the blob proved genuine, so garbage cannot evict real nonces
    /// from the window.
    ///
    /// `None` — a silent drop, with nothing forwarded — for replays, blobs too
    /// short to carry a nonce, and every authentication failure.
    fn decrypt_input(&mut self, blob: &[u8]) -> Option<Vec<u8>> {
        let nonce: [u8; NONCE_LEN] = blob.get(..NONCE_LEN)?.try_into().ok()?;
        if self.input_nonces.contains(&nonce) {
            return None;
        }
        let plaintext = self
            .key
            .decrypt(blob, &crypto::frame_aad(FrameKind::Input, 0))
            .ok()?;
        self.input_nonces.record(nonce);
        Some(plaintext)
    }

    fn on_joined(&mut self, response: &Value, in_tx: &UnboundedSender<Inbound>) {
        // The public view token. Kept here to feed `is_fresh_session` across
        // rejoins; deliberately NOT forwarded to the session, which only ever
        // prints `join_url` — and that already embeds the token.
        let token = response["token"].as_str().unwrap_or_default().to_string();
        // The single point the hub-minted URL enters the program: append the
        // key as a URL fragment HERE, so the printed link, the frozen
        // `ATUIN_SHARE_URL`, and every other consumer only ever see the full
        // fragmented URL. Browsers never send fragments in HTTP requests, so
        // the hub never sees the key; a reconnect-as-new-session re-appends
        // the same (cached) fragment to the new link.
        let join_url = format!(
            "{}#{}",
            response["join_url"].as_str().unwrap_or_default(),
            self.key_fragment
        );

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

        let fresh_session = is_fresh_session(self.last_public_token.as_deref(), &token);
        self.last_public_token = Some(token);

        // A completed join is what "connected" means, so a later outage starts
        // retrying promptly again.
        self.backoff.reset();

        let _ = in_tx.send(Inbound::Connected {
            join_url,
            fresh_session,
        });
    }
}

/// "The session we were using was replaced." True only when we had already
/// advertised a public token (`last`) and the hub handed back a different one —
/// i.e. a resume was rejected or expired and the hub silently made a new
/// session, leaving the old link dead.
///
/// Deliberately FALSE on the first join (`last` is `None`). The hub always
/// mints a session there, so a plain "did the token change?" test is true and
/// the first thing a new user sees would be "Reconnected as a NEW session — the
/// previous link is dead", which is both wrong and alarming.
fn is_fresh_session(last: Option<&str>, new: &str) -> bool {
    last.is_some_and(|previous| previous != new)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::protocol::b64_decode;

    /// A fixed key so encrypt/decrypt assertions are reproducible across the
    /// helper and the expectations built inside each test.
    fn test_key() -> SessionKey {
        SessionKey::from_bytes([0x42; crypto::KEY_LEN])
    }

    fn test_transport(write: bool) -> Transport {
        let (url_tx, _url_rx) = oneshot::channel();
        Transport::new(
            Url::parse("https://hub.example.com/some/base?stale=1").expect("valid URL"),
            "api-token-123".to_string(),
            WriteMode::from_flag(write),
            test_key(),
            url_tx,
        )
    }

    /// Seal `plaintext` exactly as a viewer's `encryptBlob` would for the
    /// input channel: Input AAD, constant seq 0.
    fn sealed_input(plaintext: &[u8]) -> Vec<u8> {
        test_key().encrypt(plaintext, &crypto::frame_aad(FrameKind::Input, 0))
    }

    #[test]
    fn ws_url_appends_to_the_base_path_and_rebuilds_the_query() {
        let url = test_transport(false).ws_url();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("hub.example.com"));
        // A path-prefixed base (reverse-proxied hub) keeps its prefix.
        assert_eq!(url.path(), "/some/base/sockets/share/websocket");
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("vsn".to_string(), "2.0.0".to_string()),
                ("token".to_string(), "api-token-123".to_string()),
            ]
        );
    }

    /// A bare origin normalizes to path `/`; appending must not produce a
    /// double slash. A trailing slash on a real prefix is trimmed the same
    /// way (the pre-typed code did this in `lab.rs`).
    #[test]
    fn ws_url_handles_bare_origins_and_trailing_slashes() {
        let base_to_path = [
            ("wss://hub.example.com", "/sockets/share/websocket"),
            ("wss://hub.example.com/hub/", "/hub/sockets/share/websocket"),
        ];
        for (base, want) in base_to_path {
            let (url_tx, _url_rx) = oneshot::channel();
            let t = Transport::new(
                Url::parse(base).expect("valid URL"),
                "tok".to_string(),
                WriteMode::from_flag(false),
                test_key(),
                url_tx,
            );
            assert_eq!(t.ws_url().path(), want, "base: {base}");
        }
    }

    #[test]
    fn join_payload_has_no_resume_token_on_the_first_join() {
        assert_eq!(
            test_transport(false).join_payload(),
            json!({ "write": false })
        );
    }

    #[test]
    fn join_payload_resumes_with_the_secret_token_never_the_public_one() {
        let mut t = test_transport(true);
        t.host_resume_token = Some("secret-resume".to_string());
        t.last_public_token = Some("public-token".to_string());
        assert_eq!(
            t.join_payload(),
            json!({ "write": true, "resume_token": "secret-resume" })
        );
    }

    /// `from_event` is keyless: it b64-decodes the sealed blob and passes it
    /// through untouched — decryption happens in `Transport::decrypt_input`.
    #[test]
    fn from_event_passes_the_sealed_input_blob_through_undecrypted() {
        let blob = sealed_input(b"ls\n");
        let payload = json!({ "data": b64_encode(&blob) });
        match Inbound::from_event("input", &payload) {
            Some(Inbound::Input(bytes)) => assert_eq!(bytes, blob),
            other => panic!("expected Input, got {}", kind(&other)),
        }
    }

    #[test]
    fn from_event_drops_input_with_bad_base64() {
        let payload = json!({ "data": "not base64!" });
        assert!(Inbound::from_event("input", &payload).is_none());
    }

    #[test]
    fn decrypt_input_yields_the_plaintext_of_a_genuine_blob() {
        let mut t = test_transport(true);
        let blob = sealed_input(b"echo hi\n");
        assert_eq!(t.decrypt_input(&blob).as_deref(), Some(&b"echo hi\n"[..]));
    }

    /// An exact hub replay re-delivers the same nonce: one delivery only.
    #[test]
    fn replayed_input_blob_is_delivered_exactly_once() {
        let mut t = test_transport(true);
        let blob = sealed_input(b"echo hi\n");
        assert!(t.decrypt_input(&blob).is_some());
        assert_eq!(t.decrypt_input(&blob), None);
        // A *different* encryption of the same keystrokes (fresh nonce) is a
        // new input, not a replay.
        assert!(t.decrypt_input(&sealed_input(b"echo hi\n")).is_some());
    }

    #[test]
    fn garbage_input_blobs_are_dropped_without_panic_or_delivery() {
        let mut t = test_transport(true);
        assert_eq!(t.decrypt_input(b""), None);
        assert_eq!(t.decrypt_input(&[0u8; 5]), None); // shorter than a nonce
        assert_eq!(t.decrypt_input(&[0u8; 27]), None); // nonce, but no room for a tag
        assert_eq!(t.decrypt_input(&[0u8; 64]), None); // right shape, wrong everything

        // A tampered blob is dropped WITHOUT burning its nonce: the genuine
        // blob it was forged from must still deliver afterwards.
        let mut blob = sealed_input(b"x");
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert_eq!(t.decrypt_input(&blob), None);
        blob[last] ^= 0x01;
        assert_eq!(t.decrypt_input(&blob).as_deref(), Some(&b"x"[..]));
    }

    /// Cross-channel reflection: a blob the host sealed as *output* must never
    /// open on the input path, even under the same key.
    #[test]
    fn output_sealed_blob_is_rejected_as_input() {
        let mut t = test_transport(true);
        let blob = test_key().encrypt(b"x", &crypto::frame_aad(FrameKind::Output, 0));
        assert_eq!(t.decrypt_input(&blob), None);
    }

    #[test]
    fn nonce_window_is_bounded_and_evicts_oldest_first() {
        let nonce_of = |i: u32| {
            let mut n = [0u8; NONCE_LEN];
            n[..4].copy_from_slice(&i.to_be_bytes());
            n
        };
        let mut w = NonceWindow::new();
        let cap = u32::try_from(INPUT_NONCE_CAP).expect("cap fits u32");
        for i in 0..cap {
            w.record(nonce_of(i));
        }
        assert!(w.contains(&nonce_of(0)));
        w.record(nonce_of(cap));
        assert!(!w.contains(&nonce_of(0)), "oldest nonce is evicted");
        assert!(w.contains(&nonce_of(1)));
        assert!(w.contains(&nonce_of(cap)));
    }

    #[test]
    fn output_payload_seals_data_under_the_seq_bound_output_aad() {
        let key = test_key();
        let frame = Frame {
            seq: 42,
            data: b"hello viewer".to_vec(),
        };
        let payload = output_payload(&key, &frame);
        // Envelope `seq` stays plaintext — the hub orders and replays on it.
        assert_eq!(payload["seq"], 42);
        let blob = b64_decode(payload["data"].as_str().expect("data is a string"))
            .expect("data is valid base64");
        assert_eq!(
            key.decrypt(&blob, &crypto::frame_aad(FrameKind::Output, 42))
                .expect("genuine output blob decrypts"),
            b"hello viewer"
        );
        // Wrong kind (reflection) and wrong seq (renumbering) must both fail.
        assert!(
            key.decrypt(&blob, &crypto::frame_aad(FrameKind::Keyframe, 42))
                .is_err()
        );
        assert!(
            key.decrypt(&blob, &crypto::frame_aad(FrameKind::Output, 43))
                .is_err()
        );
    }

    #[test]
    fn keyframe_payload_seals_data_under_the_seq_bound_keyframe_aad() {
        let key = test_key();
        let frame = Frame {
            seq: 7,
            data: b"\x1b[2Jrepaint".to_vec(),
        };
        let payload = keyframe_payload(&key, &frame);
        assert_eq!(payload["seq"], 7);
        let blob = b64_decode(payload["data"].as_str().expect("data is a string"))
            .expect("data is valid base64");
        assert_eq!(
            key.decrypt(&blob, &crypto::frame_aad(FrameKind::Keyframe, 7))
                .expect("genuine keyframe blob decrypts"),
            b"\x1b[2Jrepaint"
        );
        assert!(
            key.decrypt(&blob, &crypto::frame_aad(FrameKind::Output, 7))
                .is_err()
        );
    }

    /// Each encryption draws a fresh random nonce, so equal plaintext under
    /// the same AAD still yields distinct wire blobs.
    #[test]
    fn identical_frames_seal_to_distinct_blobs() {
        let key = test_key();
        let frame = Frame {
            seq: 1,
            data: b"same bytes".to_vec(),
        };
        assert_ne!(
            output_payload(&key, &frame)["data"],
            output_payload(&key, &frame)["data"]
        );
    }

    #[test]
    fn on_joined_appends_exactly_one_key_fragment_to_the_join_url() {
        let (url_tx, mut url_rx) = oneshot::channel();
        let mut t = Transport::new(
            Url::parse("https://hub.example.com").expect("valid URL"),
            "tok".to_string(),
            WriteMode::from_flag(false),
            test_key(),
            url_tx,
        );
        let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel();
        t.on_joined(
            &json!({
                "token": "pub-token",
                "join_url": "https://hub.example.com/lab/share/pub-token",
                "host_resume_token": "secret",
            }),
            &in_tx,
        );

        let fragment = test_key().to_fragment();
        assert_eq!(fragment.len(), 43);
        let want = format!("https://hub.example.com/lab/share/pub-token#{fragment}");
        assert_eq!(want.matches('#').count(), 1);

        // Both consumers see the SAME fragmented URL: the first-join oneshot
        // (feeds `ATUIN_SHARE_URL`) and the session's `Connected`.
        assert_eq!(
            url_rx.try_recv().expect("first join fires the oneshot"),
            want
        );
        match in_rx.try_recv().expect("Connected was sent") {
            Inbound::Connected {
                join_url,
                fresh_session,
            } => {
                assert_eq!(join_url, want);
                assert!(!fresh_session);
            }
            _ => panic!("expected Connected"),
        }

        // A rejoin appends the same cached fragment to the new link — and only
        // once.
        t.on_joined(
            &json!({
                "token": "new-token",
                "join_url": "https://hub.example.com/lab/share/new-token",
            }),
            &in_tx,
        );
        match in_rx.try_recv().expect("second Connected was sent") {
            Inbound::Connected {
                join_url,
                fresh_session,
            } => {
                assert_eq!(
                    join_url,
                    format!("https://hub.example.com/lab/share/new-token#{fragment}")
                );
                assert!(fresh_session);
            }
            _ => panic!("expected Connected"),
        }
    }

    #[test]
    fn from_event_maps_set_size() {
        let payload = json!({ "cols": 120, "rows": 40 });
        match Inbound::from_event("set_size", &payload) {
            Some(Inbound::SetSize { cols, rows }) => {
                assert_eq!((cols, rows), (120, 40));
            }
            other => panic!("expected SetSize, got {}", kind(&other)),
        }
    }

    #[test]
    fn from_event_drops_set_size_that_is_incomplete_or_out_of_range() {
        assert!(Inbound::from_event("set_size", &json!({ "cols": 120 })).is_none());
        assert!(Inbound::from_event("set_size", &json!({ "cols": 70_000, "rows": 40 })).is_none());
    }

    #[test]
    fn from_event_maps_participants() {
        match Inbound::from_event("participants", &json!({ "count": 7 })) {
            Some(Inbound::Participants(n)) => assert_eq!(n, 7),
            other => panic!("expected Participants, got {}", kind(&other)),
        }
    }

    #[test]
    fn from_event_maps_request_keyframe() {
        assert!(matches!(
            Inbound::from_event("request_keyframe", &json!({})),
            Some(Inbound::RequestKeyframe)
        ));
    }

    /// Unknown events are ignored, so the hub can add events without breaking
    /// older clients.
    #[test]
    fn from_event_ignores_unknown_events() {
        assert!(Inbound::from_event("brand_new_event", &json!({ "x": 1 })).is_none());
    }

    /// Deliberately FALSE on the first join: the hub always mints a session
    /// there, and greeting a new user with "Reconnected as a NEW session" would
    /// be both wrong and alarming.
    #[test]
    fn fresh_session_is_false_on_the_first_join() {
        assert!(!is_fresh_session(None, "tok"));
    }

    #[test]
    fn fresh_session_is_true_only_when_the_public_token_changed() {
        assert!(is_fresh_session(Some("old"), "new"));
        assert!(!is_fresh_session(Some("tok"), "tok"));
    }

    /// Every push after the join must carry a strictly greater ref than
    /// [`JOIN_REF`] (`"1"`), so the join reply — matched by ref — stays
    /// unambiguous for the whole connection.
    #[test]
    fn push_refs_start_past_the_join_ref_and_strictly_increase() {
        let mut refs = RefSequence::new();
        assert_eq!(refs.take(), "2");
        assert_eq!(refs.take(), "3");
        assert_eq!(refs.take(), "4");
    }

    /// `Inbound` carries no `Debug` impl (it holds raw terminal bytes); a
    /// variant name is enough for a test failure message.
    fn kind(v: &Option<Inbound>) -> &'static str {
        match v {
            None => "None",
            Some(Inbound::Input(_)) => "Input",
            Some(Inbound::SetSize { .. }) => "SetSize",
            Some(Inbound::Participants(_)) => "Participants",
            Some(Inbound::RequestKeyframe) => "RequestKeyframe",
            Some(Inbound::Connected { .. }) => "Connected",
            Some(Inbound::Disconnected) => "Disconnected",
        }
    }
}
