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
//! — after checking the hub minted it on its own origin — appends the key
//! fragment to the join URL at the single point that URL enters the program.
//! The session and `ScreenState` stay key-free.

use std::collections::HashSet;
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
/// How many distinct viewer-input frames one host PROCESS will ever accept.
///
/// This is a *budget*, not a window: nothing is ever evicted (see
/// [`AcceptedNonces`]). Reaching it disables viewer input for the rest of the
/// process — fail closed, never forget.
///
/// 2^20 entries is ~26 MiB steady (hashbrown: 2^21 buckets x 13 B/bucket, ~39
/// MiB peak while the final grow holds both tables). One `term.onData` event
/// is one entry, and a whole paste is one `onData` event, so pastes cost 1,
/// not their length.
///
/// Two reachability figures, because they differ by ~70x and only the second
/// one bounds the risk:
///
/// * **Human typing**: ~36 h of sustained 8 events/s. Nobody types a session
///   into the fail-closed state.
/// * **A key-holding viewer typing at machine speed**: ~30 min. Nothing rate
///   limits viewer input — `ShareViewerChannel.handle_in("input", ...)` checks
///   only `byte_size(data) <= @max_input_bytes` and `Session.viewer_input/2`
///   forwards immediately, and ~570 authenticated inputs/s was measured
///   end-to-end through the real stack. After that, viewer input is dead for
///   the remaining lifetime of this host process, for *every* viewer.
///
/// That is a denial of viewer input by someone who already holds the session
/// key — i.e. someone who already has a shell on a `--write` share — so the
/// blast radius is bounded, but the second figure is the one to reason from.
/// The missing control, if that is ever unacceptable, is a per-viewer input
/// rate limit on the hub channel, **not** a bigger number here: raising the
/// cap only moves the fail-closed point, and lowering it only moves it nearer.
/// Neither ever weakens the security property, which is the never-forget rule
/// below, not the size of the budget.
const INPUT_NONCE_CAP: usize = 1 << 20;

/// The hub's cap on an `input` event's base64 `data` field, in base64
/// characters (`@max_input_bytes`, `share_viewer_channel.ex:10`).
///
/// Quoted here so [`MAX_INPUT_BLOB_BYTES`] is *derived* from it rather than
/// coincidentally equal to it: the two must not drift apart silently.
const HUB_MAX_INPUT_B64_CHARS: usize = 4096;

/// Largest viewer-input blob the host will look at, in bytes: 3072, the
/// decoded size of the hub's own cap.
///
/// Enforced here as well, so the bound does not *depend* on the hub —
/// `Inbound::from_event` (`session/mod.rs`) b64-decodes input with no length
/// bound at all, and a hostile hub would otherwise choose the size of the
/// buffer we hand to AES.
///
/// Set **equal** to the hub's decoded cap on purpose: the host must never
/// reject something the hub would forward. That equality, not the size of a
/// keystroke, is why this bound drops no legitimate input — `term.onData`
/// fires once with an entire paste (`lab_share_viewer.js` seals `data` whole
/// and never batches), so a paste over ~3 KB does exceed this. Such a paste
/// simply never gets here: the hub's `handle_in("input", ...)` guard rejects
/// it first and it falls through to the catch-all clause, silently, with no
/// feedback to the viewer. That silent large-paste drop is a pre-existing
/// viewer-facing gap in the hub, not something this bound introduces — and if
/// the hub ever raises `@max_input_bytes`, [`HUB_MAX_INPUT_B64_CHARS`] must
/// move with it or the host becomes the stricter of the two.
const MAX_INPUT_BLOB_BYTES: usize = HUB_MAX_INPUT_B64_CHARS / 4 * 3;

// Standard base64 with padding is exactly 4 characters per 3 bytes, so the
// hub's character cap has an exact byte value only for a multiple of 4 — and
// that value is 3072. Pinned at compile time so neither constant can drift
// (or be "tidied" into a bare literal) without a build failure.
const _: () = assert!(HUB_MAX_INPUT_B64_CHARS.is_multiple_of(4));
const _: () = assert!(MAX_INPUT_BLOB_BYTES == 3072);

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
    /// The hub answered a successful join with a `join_url` that is missing,
    /// unparsable, or on an origin other than the configured hub's.
    ///
    /// **Fatal, never retried** — see [`Transport::run`]. Every other variant
    /// here means "reconnect"; this one means the hub is asking us to publish
    /// a link that hands the session key fragment to an origin the user never
    /// configured, and retrying a hub that keeps answering that way is an
    /// infinite loop. The offending URL is quoted back because the whole point
    /// is to let the user see where the hub tried to send them.
    #[error(
        "the hub returned a join url on a different origin than the configured hub: got {got:?}, \
         expected a url on {want} -- refusing to hand out the session key"
    )]
    JoinUrlOrigin {
        got: String,
        want: String,
    },
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

/// Every viewer-input nonce this host PROCESS has accepted. Nothing is ever
/// removed.
///
/// Input AAD is the constant `frame_aad(Input, 0)` — viewers are anonymous and
/// multiple write-mode viewers interleave, so there is no per-sender counter to
/// bind — which makes a blob's 12-byte nonce the ONLY field distinguishing it
/// from any other input blob: no order, no time, no sender. Exact-duplicate
/// detection over nonces is therefore the only replay defence the host can
/// implement alone, and it only works if nothing is ever forgotten. **A window
/// that evicts is a perpetual-motion machine once it has rolled over once:**
/// the hub replays the evicted batch, each frame is accepted and re-recorded,
/// which evicts the next batch, and so on forever. A bigger window moves the
/// bootstrap cost, not the outcome.
///
/// So this never evicts. It is capped at [`INPUT_NONCE_CAP`], and reaching the
/// cap FAILS CLOSED: viewer input is refused for the rest of the process.
/// Only blobs that authenticated AND carried a non-empty plaintext consume
/// budget, so a keyless hub cannot grow this set by a single entry.
///
/// What this does and does not buy, precisely, so nobody re-overstates it the
/// way the type this replaced did:
///
/// * Hub **delay** of an input frame is now genuinely availability-only — a
///   blob held back for hours still authenticates, is still absent from the
///   ledger, and is still accepted. Acceptance is deliberately not
///   time-bounded: the host cannot date a blob (there is no timestamp and the
///   AAD is constant), so only *forgetting* could be time-bounded, and
///   forgetting is the defect.
/// * Hub **reordering** (R-1) is NOT. Every blob is delivered at most once, so
///   no dedup scheme sees it, yet reordering keystrokes changes what the shell
///   executes: a viewer types `rm -rf ~/scratch`, thinks better of it, sends
///   backspaces, then types `ls\r`; a hub that drops the backspaces and
///   delivers the captured `\r` after the `rm` text runs a command the user
///   composed but deliberately never submitted.
/// * Selective **omission** (R-2) is the same class: the viewer types `# `
///   then `dangerous-command\r`, the hub drops the `# `, and a comment becomes
///   a live command. Pure omission is invisible to dedup.
///
/// R-1 and R-2 stay open and need ordering plus gap detection, which needs a
/// per-viewer counter in the input AAD's currently-constant `seq` field — a
/// wire change, deliberately not made here. **The input channel is replay-free
/// after this fix; it is not integrity-sound.**
struct AcceptedNonces {
    /// Membership, for the O(1) replay check. There is no insertion order
    /// because there is no eviction.
    seen: HashSet<[u8; NONCE_LEN]>,
    cap: usize,
}

impl AcceptedNonces {
    /// NOT pre-allocated: a read-only or short session pays nothing, and the
    /// table grows only with real accepted input. (The window this replaced
    /// pre-allocated ~26 KiB for every session, including read-only ones that
    /// can never record an entry at all.)
    fn new() -> Self {
        Self {
            seen: HashSet::new(),
            cap: INPUT_NONCE_CAP,
        }
    }

    #[cfg(test)]
    fn with_cap(cap: usize) -> Self {
        Self {
            seen: HashSet::new(),
            cap,
        }
    }

    /// Whether `nonce` was already accepted (i.e. this blob is a replay).
    fn contains(&self, nonce: &[u8; NONCE_LEN]) -> bool {
        self.seen.contains(nonce)
    }

    fn is_full(&self) -> bool {
        self.seen.len() >= self.cap
    }

    /// Record an accepted nonce.
    ///
    /// Precondition: `!contains(nonce) && !is_full()` — both are checked by
    /// [`Transport::decrypt_input`], the only caller, in that order.
    fn record(&mut self, nonce: [u8; NONCE_LEN]) {
        self.seen.insert(nonce);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.seen.len()
    }
}

/// Silent-drop tallies for the viewer-input path, reported once at teardown
/// under `ATUIN_LAB_SHARE_DEBUG`.
///
/// Every drop in [`Transport::decrypt_input`] is silent by design (nothing
/// unauthenticated may travel toward the PTY), which makes this the only way to
/// tell "the hub is replaying you" from "my keyboard is broken".
#[derive(Default)]
struct InputDrops {
    /// Nonce already accepted — an exact replay.
    replay: u64,
    /// Refused because the budget is exhausted (fail closed).
    exhausted: u64,
    /// Wrong length, or AEAD authentication failure.
    rejected: u64,
    /// Authenticated but empty plaintext.
    empty: u64,
    /// Arrived on a read-only share.
    read_only: u64,
    /// Accepted and forwarded to the child.
    accepted: u64,
}

/// The `output` push payload: envelope `seq` in the clear (the hub orders,
/// buffers, and replays on it), `data` = b64(nonce || ciphertext || tag)
/// sealed under the session key with the seq-bound Output AAD — so a hub that
/// renumbers or splices frames causes an authentication failure on the viewer
/// instead of corrupted content.
fn output_payload(key: &SessionKey, frame: &Frame) -> Value {
    let blob = key.encrypt(&frame.data, &crypto::frame_aad(FrameKind::Output, frame.seq));
    json!({ "seq": frame.seq, "data": b64_encode(&blob) })
}

/// The `keyframe` push payload; same shape as [`output_payload`] with the
/// Keyframe AAD, so a keyframe blob can never be reflected as output (or vice
/// versa).
fn keyframe_payload(key: &SessionKey, frame: &Frame) -> Value {
    let blob = key.encrypt(&frame.data, &crypto::frame_aad(FrameKind::Keyframe, frame.seq));
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
            self.wire.push(EVENT_OUTPUT, output_payload(key, &frame)).await?;
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
                self.wire.push(EVENT_KEYFRAME, keyframe_payload(key, &frame)).await?;
                // Ends any resync window opened by an overflow: output queued
                // after this keyframe carries a greater `seq`, so the hub's
                // buffer is contiguous again.
                self.queue.on_keyframe_sent();
            }
            Outbound::HostSize { cols, rows } => {
                self.flush(key).await?;
                self.wire.push(EVENT_HOST_SIZE, json!({ "cols": cols, "rows": rows })).await?;
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
    /// Replay dedup for viewer input. Lives here — NOT on the per-connection
    /// state — so a hub replay across a reconnect is still caught, and it is
    /// cleared by nothing: not a WebSocket reconnect ([`Transport::connect_once`]
    /// builds a fresh `Connection`, never a fresh `Transport`), not a Phoenix
    /// rejoin, not [`Transport::on_joined`], and specifically **not** a
    /// hub-forced fresh session (`is_fresh_session == true`, where the hub
    /// rejected the resume token and minted a new public token while the SAME
    /// key is reused). If the ledger were rebuilt on a fresh session, a hub
    /// could force a resume rejection on demand and then replay every blob it
    /// had ever captured. See [`AcceptedNonces`].
    input_nonces: AcceptedNonces,
    /// Latches the one-shot "input disabled" notice, so an exhausted budget
    /// does not re-announce itself once per refused frame.
    input_exhausted_notified: bool,
    /// The armed half of that latch: set together with it, drained by
    /// [`Transport::take_input_disabled_notice`] at the one call site that
    /// holds the session channel. Two flags rather than one because "has ever
    /// fired" (never cleared) and "not yet handed to the session" (cleared on
    /// delivery) are different facts, and [`Transport::decrypt_input`] — where
    /// the condition is detected — has no `in_tx` to send on.
    input_disabled_pending: bool,
    /// Silent-drop tallies for the input path. See [`InputDrops`].
    input_drops: InputDrops,
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
            input_nonces: AcceptedNonces::new(),
            input_exhausted_notified: false,
            input_disabled_pending: false,
            input_drops: InputDrops::default(),
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
    ///
    /// Returning is a meaningful teardown signal: `run_share` awaits this
    /// task's handle (bounded) after the session ends, so the final
    /// `Outbound::End` has genuinely been pushed to the hub — link
    /// invalidated now, not after the hub's disconnect grace period — before
    /// the process exits.
    pub(crate) async fn run(
        mut self,
        out_rx: UnboundedReceiver<Outbound>,
        in_tx: UnboundedSender<Inbound>,
    ) {
        // The relay body has several `return` points (session over, fatal
        // join-URL origin, session gone); the input tallies must be reported on
        // every one of them, so the loop lives in a helper and the report
        // happens exactly once, here, after it comes back.
        self.relay(out_rx, in_tx).await;
        self.report_input_drops();
    }

    /// One-line teardown summary of the viewer-input path, under
    /// `ATUIN_LAB_SHARE_DEBUG`.
    ///
    /// Nothing security-relevant depends on it: every decision in
    /// [`Transport::decrypt_input`] is a silent drop by design, and this is the
    /// only observability into which kind of drop happened.
    fn report_input_drops(&self) {
        if std::env::var_os("ATUIN_LAB_SHARE_DEBUG").is_none() {
            return;
        }
        let d = &self.input_drops;
        eprintln!(
            "\r\n[atuin lab share] input: accepted={} replay={} exhausted={} rejected={} empty={} \
             read_only={}\r",
            d.accepted, d.replay, d.exhausted, d.rejected, d.empty, d.read_only
        );
    }

    /// The reconnecting relay loop itself; see [`Transport::run`].
    async fn relay(
        &mut self,
        mut out_rx: UnboundedReceiver<Outbound>,
        in_tx: UnboundedSender<Inbound>,
    ) {
        let mut reported = false;
        // Items picked up while disconnected (the backoff select below),
        // relayed as soon as the next connection joins — most importantly a
        // queued `End`, which cuts the backoff short.
        let mut stashed: Vec<Outbound> = Vec::new();
        loop {
            match self.connect_once(&mut stashed, &mut out_rx, &in_tx).await {
                // The session is over (`end`, or the session dropped its
                // sender). Do not reconnect — the link is meant to die.
                Ok(()) => return,
                // FATAL, and the one error that is never retried: a hub that
                // answers the join with a foreign-origin URL will keep doing
                // it, so backoff would be an infinite loop that publishes
                // nothing and explains nothing. Printed UNCONDITIONALLY,
                // bypassing the `reported` latch below, because this is the
                // only time the user hears the real reason: returning drops
                // `self` and with it `first_url_tx`, so `connect_to_hub` sees
                // a closed oneshot and reports the vague
                // `Error::TransportStopped`. (The fully typed alternative —
                // making the oneshot carry `Result<String, Error>` so the
                // crate-level error is precise — touches `Transport::new`,
                // `connect_to_hub` and both test constructors; deliberately
                // deferred, not overlooked.)
                Err(e @ TransportError::JoinUrlOrigin { .. }) => {
                    eprintln!("\r\n[atuin lab share] {e}\r");
                    return;
                }
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
                    // The backoff sleep — cut short the moment the session
                    // queues `End` (stashed for the immediate reconnect
                    // attempt) or drops its sender: `--stop` and clean exits
                    // must not sit out a reconnect delay before the hub
                    // hears the session is over. One failed attempt at
                    // delivering a stashed `End` is the limit — after it the
                    // session is gone, the `Disconnected` send above fails,
                    // and we return; the hub's grace period is the fallback.
                    let delay = tokio::time::sleep(self.backoff.next_delay());
                    tokio::pin!(delay);
                    loop {
                        tokio::select! {
                            () = &mut delay => break,
                            item = out_rx.recv() => match item {
                                Some(item) => {
                                    let ends = matches!(item, Outbound::End);
                                    stashed.push(item);
                                    if ends {
                                        break;
                                    }
                                }
                                // Session gone with nothing more queued.
                                None => return,
                            },
                        }
                    }
                }
            }
        }
    }

    /// One connection: join, relay anything stashed during the backoff
    /// window, then relay until the socket or the session ends.
    ///
    /// `Ok(())` means the session finished for good; `Err` means reconnect.
    async fn connect_once(
        &mut self,
        stashed: &mut Vec<Outbound>,
        out_rx: &mut UnboundedReceiver<Outbound>,
        in_tx: &UnboundedSender<Inbound>,
    ) -> Result<(), TransportError> {
        let (ws, _resp) = tokio_tungstenite::connect_async(self.ws_url().as_str()).await?;
        let (sink, mut stream) = ws.split();

        // Per-connection write state (wire + fresh queue), joined immediately.
        let mut conn = Connection::new(sink);
        conn.join(&self.join_payload()).await?;

        // The backoff window's pickups go first — the same order the session
        // produced them in — so a stashed `End` ends the session here and
        // now, exactly as if it had arrived over `out_rx`.
        for item in stashed.drain(..) {
            if conn.handle_outbound(item, in_tx, &self.key).await? {
                conn.wire.flush().await?;
                return Ok(());
            }
        }
        conn.flush(&self.key).await?;

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
                self.on_joined(&response, in_tx)?;
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
                            // Every drop is silent toward the PTY, but exactly
                            // one of them is a permanent state change the host
                            // must see: the budget running out. This is the
                            // only place that owns both the detection (above)
                            // and the session channel, so the hand-off happens
                            // here rather than inside `decrypt_input`.
                            None => {
                                if self.take_input_disabled_notice() {
                                    let _ = in_tx.send(Inbound::InputDisabled);
                                }
                                return Ok(());
                            }
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

    /// Open a sealed viewer-input blob: bound its size, replay-check its nonce,
    /// spend one unit of the never-forget budget, authenticate, and deliver.
    /// (Input AAD is `frame_aad(Input, 0)` — viewers are anonymous, so there is
    /// no per-sender seq to bind.)
    ///
    /// `None` — a silent drop, with nothing forwarded toward the PTY — for
    /// input on a read-only share, oversized or truncated blobs, exact replays,
    /// an exhausted budget, every authentication failure, and empty plaintexts.
    ///
    /// The order of the steps is load-bearing; see the comments on each.
    fn decrypt_input(&mut self, blob: &[u8]) -> Option<Vec<u8>> {
        // 0. A read-only share has no input path at all. Enforced here as well
        //    as in `Session::handle_input` and on the hub (defence in depth).
        //    Doing it HERE means a read-only host does zero AEAD work on
        //    hub-supplied bytes and its ledger stays empty — the "--write only"
        //    blast radius becomes structural rather than incidental.
        if !self.write.is_write_enabled() {
            self.input_drops.read_only += 1;
            return None;
        }

        // 1. Shape, before any AES work. The hub already caps `data` at 4096
        //    base64 chars; we do not depend on that.
        if blob.len() > MAX_INPUT_BLOB_BYTES {
            self.input_drops.rejected += 1;
            return None;
        }
        let Some(nonce) = blob.get(..NONCE_LEN).and_then(|n| <[u8; NONCE_LEN]>::try_from(n).ok())
        else {
            self.input_drops.rejected += 1;
            return None;
        };

        // 2. Replay check FIRST: a re-delivered blob costs no AEAD work, and
        //    stays classified as a replay even after the budget is exhausted
        //    (so the notice below is not fired by traffic that would have been
        //    dropped anyway).
        if self.input_nonces.contains(&nonce) {
            self.input_drops.replay += 1;
            return None;
        }

        // 3. FAIL CLOSED. Never evict => never forget => refuse. This is the
        //    whole fix: the alternative to refusing is forgetting, and
        //    forgetting is the replay defect. An exhausted host also stops
        //    doing AEAD work entirely, hence this precedes step 4.
        if self.input_nonces.is_full() {
            self.input_drops.exhausted += 1;
            self.note_input_exhausted();
            return None;
        }

        // 4. Authenticate BEFORE spending budget. Garbage from a keyless hub
        //    must never consume a slot — that is what makes the memory bound
        //    and the fail-closed point un-inflatable by a hostile hub. It also
        //    preserves the older invariant that garbage cannot displace real
        //    nonces, strengthened into "garbage cannot consume budget".
        let Ok(plaintext) = self.key.decrypt(blob, &crypto::frame_aad(FrameKind::Input, 0)) else {
            self.input_drops.rejected += 1;
            return None;
        };

        // 5. An empty plaintext writes zero bytes to the PTY, so it has no
        //    legitimate use — and spending budget on it is exactly the
        //    amplifier that used to flush a 1024-entry window in 1.8 seconds.
        //    Drop it WITHOUT spending anything.
        //
        //    This is BUDGET INTEGRITY, NOT A REPLAY DEFENCE. A key holder can
        //    approximate a zero-visibility filler byte with `\0` or a lone
        //    `ESC`; empty-rejection only removes the free, zero-trace
        //    amplifier. Anyone reading this step as "the amplifier is fixed, so
        //    replay is fixed" has misread it — what closes replay is steps 2, 3
        //    and 6 together, i.e. the never-forget ledger and the fail-closed
        //    cap. The harness's one-byte flood case exists to catch exactly
        //    that misreading.
        if plaintext.is_empty() {
            self.input_drops.empty += 1;
            return None;
        }

        // 6. Spend one unit and deliver. Exactly once, for the life of this
        //    process, at any delay, in any order, across any reconnect and any
        //    hub-forced fresh session.
        self.input_nonces.record(nonce);
        self.input_drops.accepted += 1;
        Some(plaintext)
    }

    /// Arm the one-shot host notice for an exhausted input budget.
    ///
    /// Fires on the first input frame refused for budget, whatever that frame
    /// was (we deliberately do not decrypt it to find out). The session does
    /// NOT tear down: output keeps flowing, viewers keep watching, the host's
    /// own keystrokes are unaffected, the link stays live. Only viewer typing
    /// stops mattering.
    ///
    /// Nothing is printed from here. The notice travels as
    /// [`Inbound::InputDisabled`] to the session, which owns the host's
    /// terminal and turns it into a **sticky bar segment** plus one
    /// explanatory line. A bare `eprintln!` from this side would be composited
    /// over by the next repaint — and since this state is permanent and the
    /// latch fires once, that would leave a fail-closed host with no signal at
    /// all a few keystrokes later.
    ///
    /// The viewer and the hub are still deliberately NOT told: telling them
    /// needs a new channel event, i.e. a wire change, which this fix forbids.
    fn note_input_exhausted(&mut self) {
        if self.input_exhausted_notified {
            return;
        }
        self.input_exhausted_notified = true;
        self.input_disabled_pending = true;
    }

    /// Take the armed "viewer input is disabled" notice, if any. True at most
    /// once per process — see [`Transport::input_disabled_pending`].
    fn take_input_disabled_notice(&mut self) -> bool {
        std::mem::take(&mut self.input_disabled_pending)
    }

    /// Handle a successful join reply: validate the hub-minted join URL,
    /// append the key fragment, cache the session's tokens, and report the URL
    /// to both consumers.
    ///
    /// # Errors
    ///
    /// [`TransportError::JoinUrlOrigin`] if `join_url` is absent, unparsable,
    /// relative, or on a foreign origin. On that path **nothing** is reported:
    /// no `first_url_tx`, no [`Inbound::Connected`], no cached tokens — so no
    /// consumer ever sees a link carrying the key fragment off the configured
    /// hub's origin. The error is fatal in [`Transport::run`], not retried.
    fn on_joined(
        &mut self,
        response: &Value,
        in_tx: &UnboundedSender<Inbound>,
    ) -> Result<(), TransportError> {
        // The single point the hub-minted URL enters the program: validate its
        // origin and append the key as a URL fragment HERE, so the printed
        // link, the frozen `ATUIN_SHARE_URL`, and every other consumer only
        // ever see one full fragmented URL — and only ever one pointing at the
        // hub the user configured. Browsers never send fragments in HTTP
        // requests, so the hub never sees the key; a reconnect-as-new-session
        // re-appends the same (cached) fragment to the new link.
        //
        // Validating the origin is what bounds the blast radius of a hostile
        // or compromised hub to the hub itself: without it, a join reply of
        // `join_url = "https://attacker.example.com/collect?s=1"` makes the CLI
        // print — and freeze into the subshell's `ATUIN_SHARE_URL` — a link
        // that hands the AES key to an attacker origin the moment anyone opens
        // it.
        //
        // Parsing first also closes a latent hole: `as_str().unwrap_or_default()`
        // on a missing `join_url` used to produce the bare string
        // `"#<43-char key fragment>"` — a naked key with no URL at all.
        //
        // The raw string (not the reserialized `Url`) is what gets the
        // fragment, so the hub's link is published byte-for-byte as minted. A
        // hub that mints a URL which *already* has a fragment therefore yields
        // a link the viewer cannot read a key out of; that is availability
        // only — the key still never leaves the hub's own origin — and a hub
        // can deny service far more simply than that.
        let raw = response["join_url"].as_str().unwrap_or_default();
        let origin_ok = Url::parse(raw).is_ok_and(|join| same_origin(&self.hub_url, &join));
        if !origin_ok {
            return Err(TransportError::JoinUrlOrigin {
                got: raw.to_string(),
                want: self.hub_url.to_string(),
            });
        }
        let join_url = format!("{raw}#{}", self.key_fragment);

        // The public view token. Kept here to feed `is_fresh_session` across
        // rejoins; deliberately NOT forwarded to the session, which only ever
        // prints `join_url` — and that already embeds the token.
        let token = response["token"].as_str().unwrap_or_default().to_string();

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
        Ok(())
    }
}

/// A URL scheme normalized for origin comparison: `ws` and `wss` are the
/// transport spellings of `http` and `https`, and everything else is left
/// alone.
///
/// This is the whole subtlety of [`same_origin`]. `Transport::hub_url` is a
/// **ws/wss** URL by construction (`lab_ws_url` derives it http->ws,
/// https->wss) while a hub-minted `join_url` is **http/https** — a browser
/// link. So comparing raw schemes, or comparing `url::Origin` values (which
/// carry the raw scheme), rejects every legitimate join.
fn normalized_scheme(scheme: &str) -> &str {
    match scheme {
        "ws" => "http",
        "wss" => "https",
        other => other,
    }
}

/// Whether the hub-minted `join` URL sits on the same origin as the configured
/// `hub`.
///
/// Compares exactly the triple (normalized scheme, host, port), and:
///
/// * The join side must be `http`/`https` outright, not after normalization:
///   it is a link handed to a browser, so `ws:`, `javascript:`, `file:` and
///   `data:` are never share links — and normalizing ws->http on this side too
///   would otherwise let `ws://hub/...` through as "same origin".
/// * Ports use [`Url::port_or_known_default`], never `port()`: the latter is
///   `None` for an implicit default and `Some(443)` when the same port is
///   written out, so it would reject `wss://hub` vs `https://hub:443`. `url`
///   maps `http|ws => 80` and `https|wss => 443`, which is exactly the
///   equivalence we want.
/// * The **path is deliberately not compared**. The hub may mint any path
///   under its own origin, and [`Transport::ws_url`] already documents that a
///   reverse-proxy path prefix on the base URL is legitimate — so the base's
///   path and the join URL's path routinely differ on a healthy hub.
fn same_origin(hub: &Url, join: &Url) -> bool {
    if !matches!(join.scheme(), "http" | "https") {
        return false;
    }
    normalized_scheme(hub.scheme()) == join.scheme()
        && hub.host_str() == join.host_str()
        && hub.port_or_known_default() == join.port_or_known_default()
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

    /// A transport whose input ledger fails closed after `cap` accepted
    /// frames, so the never-forget rule is testable without minting 2^20
    /// blobs. Test-only: production always uses [`INPUT_NONCE_CAP`].
    fn capped_transport(write: bool, cap: usize) -> Transport {
        let mut t = test_transport(write);
        t.input_nonces = AcceptedNonces::with_cap(cap);
        t
    }

    /// Seal `plaintext` exactly as a viewer's `encryptBlob` would for the
    /// input channel: Input AAD, constant seq 0.
    fn sealed_input(plaintext: &[u8]) -> Vec<u8> {
        test_key().encrypt(plaintext, &crypto::frame_aad(FrameKind::Input, 0))
    }

    /// A distinct, deterministic nonce per `i`, for the ledger's own tests.
    fn nonce_of(i: u32) -> [u8; NONCE_LEN] {
        let mut n = [0u8; NONCE_LEN];
        n[..4].copy_from_slice(&i.to_be_bytes());
        n
    }

    #[test]
    fn ws_url_appends_to_the_base_path_and_rebuilds_the_query() {
        let url = test_transport(false).ws_url();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("hub.example.com"));
        // A path-prefixed base (reverse-proxied hub) keeps its prefix.
        assert_eq!(url.path(), "/some/base/sockets/share/websocket");
        let pairs: Vec<(String, String)> =
            url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        assert_eq!(pairs, vec![
            ("vsn".to_string(), "2.0.0".to_string()),
            ("token".to_string(), "api-token-123".to_string()),
        ]);
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
        assert_eq!(test_transport(false).join_payload(), json!({ "write": false }));
    }

    #[test]
    fn join_payload_resumes_with_the_secret_token_never_the_public_one() {
        let mut t = test_transport(true);
        t.host_resume_token = Some("secret-resume".to_string());
        t.last_public_token = Some("public-token".to_string());
        assert_eq!(t.join_payload(), json!({ "write": true, "resume_token": "secret-resume" }));
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

    /// Also the **memory-integrity** test: none of this spends a unit of the
    /// fail-closed budget, so a keyless hub — which can only ever produce
    /// blobs like these — cannot drive the host toward refusing viewer input,
    /// nor grow its ledger by a single entry.
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
        assert_eq!(t.input_nonces.len(), 0, "unauthenticated bytes never consume budget");
        blob[last] ^= 0x01;
        assert_eq!(t.decrypt_input(&blob).as_deref(), Some(&b"x"[..]));
        assert_eq!(t.input_nonces.len(), 1, "only the genuine blob spent a slot");
    }

    /// Cross-channel reflection: a blob the host sealed as *output* must never
    /// open on the input path, even under the same key.
    #[test]
    fn output_sealed_blob_is_rejected_as_input() {
        let mut t = test_transport(true);
        let blob = test_key().encrypt(b"x", &crypto::frame_aad(FrameKind::Output, 0));
        assert_eq!(t.decrypt_input(&blob), None);
    }

    /// Within the budget the ledger is a pure accumulator: no entry is ever
    /// displaced by a newer one, because the *only* thing that makes exact
    /// nonce dedup a replay defence is that it never forgets.
    #[test]
    fn accepted_nonces_never_forget_within_the_cap() {
        let mut ledger = AcceptedNonces::with_cap(4);
        for i in 0..4 {
            ledger.record(nonce_of(i));
        }
        for i in 0..4 {
            assert!(ledger.contains(&nonce_of(i)), "nonce {i} was forgotten");
        }
        assert_eq!(ledger.len(), 4);
    }

    /// Full means full: the oldest entry stays, and the ledger refuses to grow
    /// rather than making room. The predecessor of this type evicted here —
    /// which is precisely what made a captured blob replayable.
    #[test]
    fn accepted_nonces_refuse_new_entries_once_full() {
        let mut ledger = AcceptedNonces::with_cap(4);
        for i in 0..4 {
            ledger.record(nonce_of(i));
        }
        assert!(ledger.is_full());
        assert!(ledger.contains(&nonce_of(0)), "the oldest nonce is NOT evicted to make room");
        assert_eq!(ledger.len(), 4);
    }

    /// **The replay defect in miniature**, mirroring the `rv-f.sh` harness
    /// step for step: capture one sealed keystroke, push a whole cap's worth of
    /// further *genuine, non-empty* inputs on top of it — enough to have
    /// evicted the capture from any window of this size — then re-send the
    /// capture byte-for-byte.
    ///
    /// A cap's worth on top of `X` is deliberately one more than fits: under a
    /// FIFO window that is exactly the push that evicted `X` and made the
    /// replay execute. Here it is simply refused, and `X` stays remembered
    /// forever. There must be no cap at which this flips.
    #[test]
    fn decrypt_input_rejects_a_replay_after_a_full_cap_of_newer_inputs() {
        const CAP: usize = 16;
        let mut t = capped_transport(true, CAP);
        let x = sealed_input(b"RVMARKER\n");
        assert_eq!(t.decrypt_input(&x).as_deref(), Some(&b"RVMARKER\n"[..]));

        // The flood: CAP distinct, non-empty, genuine inputs. The budget only
        // has room for CAP-1 more, so the tail of the flood is refused —
        // failing closed, never forgetting.
        for i in 0..CAP {
            t.decrypt_input(&sealed_input(format!("k{i}").as_bytes()));
        }

        // Asserted before anything else about the ledger's state, so that a
        // regression to an evicting policy fails HERE, on the security
        // property, rather than on a bookkeeping precondition.
        assert_eq!(t.decrypt_input(&x), None, "the captured blob must never execute a second time");
        assert_eq!(t.input_drops.replay, 1);

        // And the flood's tail was refused rather than allowed to displace it.
        assert!(t.input_nonces.is_full());
        assert_eq!(t.input_drops.accepted, u64::try_from(CAP).expect("fits"));
        assert_eq!(t.input_drops.exhausted, 1);
    }

    /// The fail-closed half of the trade this fix knowingly makes: once the
    /// budget is spent, even brand-new genuine input is refused. Viewer typing
    /// stops; output, viewing and the host's own keystrokes are unaffected.
    #[test]
    fn decrypt_input_fails_closed_on_fresh_genuine_input_when_full() {
        const CAP: usize = 8;
        let mut t = capped_transport(true, CAP);
        for i in 0..CAP {
            assert!(t.decrypt_input(&sealed_input(format!("k{i}").as_bytes())).is_some());
        }
        assert_eq!(
            t.decrypt_input(&sealed_input(b"never delivered")),
            None,
            "a full ledger refuses rather than forgets"
        );
        assert_eq!(t.input_drops.exhausted, 1);
        assert_eq!(t.input_nonces.len(), CAP);
    }

    /// An authenticated but EMPTY input writes zero bytes to the PTY, so it has
    /// no legitimate use — and under the old window each one evicted a real
    /// nonce, which is what made the whole ledger flushable in 1.8 seconds.
    /// They are now dropped without spending a single slot.
    #[test]
    fn empty_plaintext_input_is_dropped_without_spending_budget() {
        let mut t = capped_transport(true, 4);
        for _ in 0..100 {
            assert_eq!(t.decrypt_input(&sealed_input(b"")), None);
        }
        assert_eq!(t.input_nonces.len(), 0, "empties never consume budget");
        assert_eq!(t.input_drops.empty, 100);
        // And the budget they did not spend is still there for real input.
        assert!(t.decrypt_input(&sealed_input(b"ls\n")).is_some());
        assert_eq!(t.input_nonces.len(), 1);
    }

    /// Read-only shares have no input path at all: the transport refuses before
    /// any AES work, so the ledger of a read-only host stays empty and its
    /// fail-closed budget can never be approached by hub traffic.
    #[test]
    fn read_only_transport_never_decrypts_or_records_input() {
        let mut t = test_transport(false);
        assert_eq!(t.decrypt_input(&sealed_input(b"rm -rf /\n")), None);
        assert_eq!(t.input_nonces.len(), 0);
        assert_eq!(t.input_drops.read_only, 1);
        assert_eq!(t.input_drops.accepted, 0);
    }

    /// The size bound is the host's own, not the hub's: `Inbound::from_event`
    /// b64-decodes input with no length bound at all. Boundary on both sides —
    /// a genuine blob one byte over is refused before decryption, and a genuine
    /// blob exactly at the limit still delivers.
    #[test]
    fn oversized_input_blob_is_refused_before_decryption() {
        let mut t = test_transport(true);

        // Genuine and decryptable, but one byte too long: 3045 + 12 + 16.
        let too_long = sealed_input(&vec![b'a'; MAX_INPUT_BLOB_BYTES - 27]);
        assert_eq!(too_long.len(), MAX_INPUT_BLOB_BYTES + 1);
        assert_eq!(t.decrypt_input(&too_long), None);
        assert_eq!(t.input_nonces.len(), 0, "refused before any AES work");
        assert_eq!(t.input_drops.rejected, 1);

        // Exactly at the limit: 3044 + 12 + 16 = 3072.
        let at_limit = sealed_input(&vec![b'a'; MAX_INPUT_BLOB_BYTES - 28]);
        assert_eq!(at_limit.len(), MAX_INPUT_BLOB_BYTES);
        assert_eq!(t.decrypt_input(&at_limit).map(|p| p.len()), Some(MAX_INPUT_BLOB_BYTES - 28));
    }

    /// The ledger is process-lifetime and cleared by NOTHING — in particular
    /// not by a hub-forced fresh session, where the hub rejects our resume
    /// token and mints a new public token while the same key is reused. If it
    /// were rebuilt there, a hub could force a resume rejection on demand and
    /// replay every blob it had ever captured.
    #[test]
    fn input_ledger_survives_a_hub_forced_fresh_session() {
        let (url_tx, _url_rx) = oneshot::channel();
        let mut t = Transport::new(
            Url::parse("https://hub.example.com").expect("valid URL"),
            "tok".to_string(),
            WriteMode::from_flag(true),
            test_key(),
            url_tx,
        );
        let (in_tx, _in_rx) = tokio::sync::mpsc::unbounded_channel();

        let x = sealed_input(b"echo hi\n");
        assert!(t.decrypt_input(&x).is_some());

        // Two joins with DIFFERENT public tokens: the second is the hub
        // silently replacing the session behind our back.
        t.on_joined(
            &json!({ "token": "tok-a", "join_url": "https://hub.example.com/lab/share/tok-a" }),
            &in_tx,
        )
        .expect("a same-origin join url is accepted");
        t.on_joined(
            &json!({ "token": "tok-b", "join_url": "https://hub.example.com/lab/share/tok-b" }),
            &in_tx,
        )
        .expect("a same-origin join url is accepted");
        assert_eq!(t.last_public_token.as_deref(), Some("tok-b"));

        assert_eq!(
            t.decrypt_input(&x),
            None,
            "a fresh session must not resurrect an already-accepted nonce"
        );
    }

    /// The exhaustion notice is a one-shot: an exhausted budget must not
    /// re-announce itself once per refused frame for the rest of the session.
    #[test]
    fn input_exhausted_notice_fires_at_most_once() {
        const CAP: usize = 4;
        let mut t = capped_transport(true, CAP);
        for i in 0..CAP {
            assert!(t.decrypt_input(&sealed_input(format!("k{i}").as_bytes())).is_some());
        }
        assert!(!t.input_exhausted_notified);
        for i in 0..10 {
            assert_eq!(t.decrypt_input(&sealed_input(format!("x{i}").as_bytes())), None);
        }
        assert!(t.input_exhausted_notified, "the notice latches");
        assert_eq!(t.input_drops.exhausted, 10, "every refusal is still counted");
        // Armed exactly once, and drained by the first taker.
        assert!(t.take_input_disabled_notice());
        assert!(!t.take_input_disabled_notice());
    }

    /// The fail-closed state must reach the **session**, not just stderr: the
    /// session owns the status bar, and a bar segment is the only host-facing
    /// surface a repaint does not erase. Driven through `handle_text`, the
    /// real hub-frame path, so the wiring — not just the flag — is covered.
    #[test]
    fn an_exhausted_budget_tells_the_session_exactly_once() {
        const CAP: usize = 2;
        let mut t = capped_transport(true, CAP);
        let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel();

        let input_frame = |plaintext: &str| {
            format!(
                r#"["1","2","share:host","input",{{"data":"{}"}}]"#,
                crate::protocol::b64_encode(&sealed_input(plaintext.as_bytes()))
            )
        };

        // Spend the budget: every one of these is delivered as input.
        for i in 0..CAP {
            t.handle_text(&input_frame(&format!("k{i}")), &in_tx)
                .expect("a genuine input frame is not a transport error");
            assert!(matches!(in_rx.try_recv(), Ok(Inbound::Input(_))));
        }

        // The first refused frame: nothing reaches the PTY, and the session is
        // told once.
        t.handle_text(&input_frame("refused"), &in_tx)
            .expect("a refused input frame is not a transport error");
        assert!(matches!(in_rx.try_recv(), Ok(Inbound::InputDisabled)));

        // Every later refusal is silent — the host's bar already says so.
        for i in 0..5 {
            t.handle_text(&input_frame(&format!("x{i}")), &in_tx)
                .expect("still not a transport error");
        }
        assert!(in_rx.try_recv().is_err(), "the notice must not repeat");
        assert_eq!(t.input_drops.exhausted, 6);
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
        assert!(key.decrypt(&blob, &crypto::frame_aad(FrameKind::Keyframe, 42)).is_err());
        assert!(key.decrypt(&blob, &crypto::frame_aad(FrameKind::Output, 43)).is_err());
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
        assert!(key.decrypt(&blob, &crypto::frame_aad(FrameKind::Output, 7)).is_err());
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
        assert_ne!(output_payload(&key, &frame)["data"], output_payload(&key, &frame)["data"]);
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
        )
        .expect("a same-origin join url is accepted");

        let fragment = test_key().to_fragment();
        assert_eq!(fragment.len(), 43);
        let want = format!("https://hub.example.com/lab/share/pub-token#{fragment}");
        assert_eq!(want.matches('#').count(), 1);

        // Both consumers see the SAME fragmented URL: the first-join oneshot
        // (feeds `ATUIN_SHARE_URL`) and the session's `Connected`.
        assert_eq!(url_rx.try_recv().expect("first join fires the oneshot"), want);
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
        )
        .expect("a same-origin join url is accepted on a rejoin too");
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

    /// Drive one `on_joined` against a transport configured for hub base `hub`.
    ///
    /// On acceptance, both join-URL consumers — the first-join oneshot (which
    /// feeds `ATUIN_SHARE_URL`) and the session's `Connected` — must have been
    /// handed the SAME string, which is returned. On refusal both must have
    /// been handed NOTHING: that silence is the security property, so it is
    /// asserted here once instead of in every caller.
    fn join_once(hub: &str, response: &Value) -> Result<String, TransportError> {
        let (url_tx, mut url_rx) = oneshot::channel();
        let mut t = Transport::new(
            Url::parse(hub).expect("valid hub URL"),
            "tok".to_string(),
            WriteMode::from_flag(false),
            test_key(),
            url_tx,
        );
        let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel();
        let result = t.on_joined(response, &in_tx);
        let to_env = url_rx.try_recv().ok();
        let to_session = match in_rx.try_recv() {
            Ok(Inbound::Connected { join_url, .. }) => Some(join_url),
            Ok(other) => panic!("expected Connected, got {}", kind(&Some(other))),
            Err(_) => None,
        };
        match result {
            Ok(()) => {
                let url = to_env.expect("an accepted join fires the first-url oneshot");
                assert_eq!(
                    to_session.as_deref(),
                    Some(url.as_str()),
                    "both consumers must see the same URL"
                );
                Ok(url)
            }
            Err(e) => {
                assert_eq!(
                    to_env, None,
                    "a refused join must not hand the key fragment to ATUIN_SHARE_URL"
                );
                assert_eq!(to_session, None, "a refused join must not report Connected");
                // Nothing about the refused session is remembered either.
                assert_eq!(t.host_resume_token, None);
                assert_eq!(t.last_public_token, None);
                Err(e)
            }
        }
    }

    /// The D5 attack, verbatim: a hub answering the join with a URL on an
    /// origin it does not own would otherwise make the CLI print — and freeze
    /// into `ATUIN_SHARE_URL` — a link handing the AES key to that origin.
    #[test]
    fn on_joined_refuses_a_foreign_origin_join_url() {
        let err = join_once(
            "wss://hub.example.com",
            &json!({
                "token": "pub-token",
                "join_url": "https://attacker.example.com/collect?s=1",
                "host_resume_token": "secret",
            }),
        )
        .expect_err("a foreign origin is refused");
        match &err {
            TransportError::JoinUrlOrigin { got, want } => {
                assert_eq!(got, "https://attacker.example.com/collect?s=1");
                assert!(want.contains("hub.example.com"), "want: {want}");
            }
            other => panic!("expected JoinUrlOrigin, got {other:?}"),
        }
        // The message names the offending URL and never the key fragment.
        let msg = err.to_string();
        assert!(msg.is_ascii(), "user-visible copy stays ASCII: {msg}");
        assert!(msg.contains("attacker.example.com"), "{msg}");
        assert!(!msg.contains(&test_key().to_fragment()), "{msg}");
    }

    /// The regression a naive check introduces: the hub base is **ws/wss** by
    /// construction (`lab_ws_url` derives http->ws, https->wss) while the join
    /// URL is a browser link, so raw-scheme or `url::Origin` equality would
    /// reject every real share while passing an `https` test fixture.
    #[test]
    fn on_joined_accepts_an_https_join_url_from_a_wss_hub() {
        let url = join_once(
            "wss://hub.example.com",
            &json!({ "token": "t", "join_url": "https://hub.example.com/lab/share/t" }),
        )
        .expect("wss hub and https join url are the same origin");
        assert_eq!(
            url,
            format!("https://hub.example.com/lab/share/t#{}", test_key().to_fragment())
        );
    }

    /// The local-dev / repro configuration: a plain-HTTP hub reached as
    /// `ws://`, which `ATUIN_LAB_HUB_URL` passes through as given.
    #[test]
    fn on_joined_accepts_an_http_join_url_from_a_ws_hub_on_an_explicit_port() {
        let url = join_once(
            "ws://127.0.0.1:4131",
            &json!({ "token": "t", "join_url": "http://127.0.0.1:4131/lab/share/t" }),
        )
        .expect("ws hub and http join url on the same port are the same origin");
        assert!(url.starts_with("http://127.0.0.1:4131/lab/share/t#"));
    }

    /// Origin is scheme+host+port and nothing else: a reverse-proxied hub's
    /// base path and the minted link's path routinely differ, and a default
    /// port spelled out on one side only must still match.
    #[test]
    fn on_joined_accepts_a_different_path_and_a_spelled_out_default_port() {
        let url = join_once(
            "wss://hub.example.com/hub/",
            &json!({ "token": "t", "join_url": "https://hub.example.com:443/lab/share/t?v=2" }),
        )
        .expect("path and explicit default port do not change the origin");
        assert!(url.contains("/lab/share/t?v=2#"));
    }

    /// A missing `join_url` used to yield the bare string `"#<43-char key
    /// fragment>"` — a naked session key with no URL at all — because the
    /// format string ran on `unwrap_or_default()`. Parsing first refuses it.
    #[test]
    fn on_joined_refuses_a_missing_join_url_instead_of_printing_a_naked_key() {
        let err = join_once("wss://hub.example.com", &json!({ "token": "pub-token" }))
            .expect_err("a missing join url is refused");
        match err {
            TransportError::JoinUrlOrigin { got, .. } => assert_eq!(got, ""),
            other => panic!("expected JoinUrlOrigin, got {other:?}"),
        }
    }

    /// The pure origin rule, including every way a hostile hub can try to be
    /// "almost" the configured origin.
    #[test]
    fn same_origin_compares_scheme_host_and_port_and_nothing_else() {
        let accept = [
            // ws/wss hub bases against the http/https links they mint.
            ("wss://hub.example.com", "https://hub.example.com/lab/share/t"),
            ("wss://hub.example.com", "https://hub.example.com:443/x"),
            ("ws://127.0.0.1:4131", "http://127.0.0.1:4131/lab/share/t"),
            ("ws://localhost", "http://localhost:80/x"),
            // An http/https base (an `ATUIN_LAB_HUB_URL` given as given).
            ("https://hub.example.com/hub/", "https://hub.example.com/a/b?c=1"),
            ("http://127.0.0.1:4131", "http://127.0.0.1:4131/x"),
        ];
        for (hub, join) in accept {
            assert!(
                same_origin(&Url::parse(hub).expect("hub"), &Url::parse(join).expect("join")),
                "expected same origin: {hub} vs {join}"
            );
        }

        let refuse = [
            // Different host, including a lookalike subdomain and suffix.
            ("wss://hub.example.com", "https://attacker.example.com/collect?s=1"),
            ("wss://hub.example.com", "https://hub.example.com.evil.test/x"),
            ("wss://hub.example.com", "https://evil.hub.example.com/x"),
            // Different port, including the implicit-vs-wrong-explicit pair.
            ("ws://127.0.0.1:4131", "http://127.0.0.1:4132/x"),
            ("wss://hub.example.com", "https://hub.example.com:8443/x"),
            // Scheme downgrade: a wss hub never mints a plaintext link.
            ("wss://hub.example.com", "http://hub.example.com/x"),
            ("ws://hub.example.com", "https://hub.example.com/x"),
            // Not a browser link at all. `ws:` must NOT be normalized on the
            // join side, or `ws://hub/...` would read as same-origin.
            ("wss://hub.example.com", "ws://hub.example.com/x"),
            ("wss://hub.example.com", "javascript:alert(1)"),
            ("wss://hub.example.com", "data:text/html,x"),
            ("wss://hub.example.com", "file:///etc/passwd"),
        ];
        for (hub, join) in refuse {
            assert!(
                !same_origin(&Url::parse(hub).expect("hub"), &Url::parse(join).expect("join")),
                "expected different origins: {hub} vs {join}"
            );
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

    /// The VIEWER-DIRECTION half of the cross-language interop harness, and
    /// the only machine check that Rust and JS agree on the **input** AAD.
    ///
    /// `crypto::tests::frozen_vector_encrypts_to_the_exact_bytes` proves the
    /// Rust->JS direction against a byte-frozen vector, and
    /// `crypto::tests::emit_interop_blob` hands a Rust-sealed blob to the
    /// shipped viewer `crypto.js`. Neither exercises the direction this fix is
    /// actually about: a blob the **viewer** sealed, opened by the host's
    /// accept path. This does, and it runs in every `cargo test` — not behind
    /// `#[ignore]`, and not depending on a file outside the repository.
    ///
    /// The record is produced by `tests/interop/seal-input.mjs`, which imports
    /// the shipped, unmodified `hub/assets/js/lab_share/crypto.js` and seals a
    /// plaintext with `encryptBlob(key, bytes, frameAad(INPUT, 0))` — the exact
    /// call `term.onData` makes. One such record is **frozen** next to it and
    /// compiled in here, so CI keeps proving the property with no node step and
    /// no hub checkout; `INTEROP_INPUT` swaps in a freshly generated one when
    /// re-running the emitter against a changed viewer.
    ///
    /// Three assertions:
    ///
    /// 1. Both sides built the same 9-byte input AAD (`03` then eight zero
    ///    bytes), compared as bytes rather than assumed.
    /// 2. [`Transport::decrypt_input`] returns the exact plaintext JS sealed,
    ///    so the size bound, replay check, fail-closed gate, and authentication
    ///    step did not disturb the wire.
    /// 3. Feeding the **byte-identical** blob a second time returns `None` —
    ///    the never-forget ledger refusing a replay of a genuine, JS-minted,
    ///    correctly-authenticated frame. That is D1's attack in miniature,
    ///    driven from the real viewer implementation.
    #[test]
    fn open_js_sealed_input_blob() {
        /// The frozen viewer-sealed input blob (see the doc above). Compiled
        /// in, so this test has no runtime dependency on anything outside the
        /// crate.
        const FROZEN: &str = include_str!("../tests/interop/js-sealed-input.json");

        let record: Value = match std::env::var("INTEROP_INPUT") {
            Ok(path) => {
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read INTEROP_INPUT"))
                    .expect("INTEROP_INPUT is JSON")
            }
            Err(_) => serde_json::from_str(FROZEN).expect("the frozen record is JSON"),
        };

        let field = |name: &str| -> String {
            record[name]
                .as_str()
                .unwrap_or_else(|| panic!("the interop record has a string {name}"))
                .to_string()
        };

        // If `test_key` ever changes, the JS side sealed under a different key
        // and every assertion below would fail for an unrelated reason. Say so
        // here instead.
        assert_eq!(
            field("fragment"),
            test_key().to_fragment(),
            "the JS sealer and this test must use the same session key"
        );

        // Both implementations independently built the input AAD; compare the
        // bytes rather than trusting that they agree.
        let aad = crypto::frame_aad(FrameKind::Input, 0);
        assert_eq!(
            field("aad_hex"),
            hex::encode(aad),
            "Rust and JS disagree on the input AAD layout"
        );

        let blob = hex::decode(field("blob_hex")).expect("blob_hex is hex");
        let expected = hex::decode(field("plaintext_hex")).expect("plaintext_hex is hex");
        assert!(!expected.is_empty(), "the sealed plaintext must be non-empty");

        let mut transport = test_transport(true);

        // The host opens what the viewer sealed.
        assert_eq!(
            transport.decrypt_input(&blob).as_deref(),
            Some(expected.as_slice()),
            "the host must open a blob sealed by the shipped viewer crypto.js"
        );

        // And refuses the byte-identical replay of that same genuine blob.
        assert!(
            transport.decrypt_input(&blob).is_none(),
            "a byte-identical replay of a JS-sealed input blob must be refused"
        );
        assert_eq!(transport.input_drops.replay, 1);
        assert_eq!(transport.input_nonces.len(), 1, "a replay records nothing");
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
            Some(Inbound::InputDisabled) => "InputDisabled",
            Some(Inbound::Connected { .. }) => "Connected",
            Some(Inbound::Disconnected) => "Disconnected",
        }
    }
}
