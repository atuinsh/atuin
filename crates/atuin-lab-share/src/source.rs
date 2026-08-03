//! The seam between the session and whatever produces its terminal bytes.
//!
//! `Session::run` is written against [`SourceParts`] alone: it spawns the same
//! bridged threads and runs the same select loop whether the bytes come from a
//! subshell it spawned (the classic `atuin lab share`) or from some other
//! byte-faithful source attached later. Everything source-specific — how to
//! resize, how to stop, how to learn the exit code — travels as a closure, so
//! the session never needs to know which kind of source it is running.

use std::io::{self, Read, Write};

use crate::Size;

/// Read-buffer size for source output (one chunk per `Outbound::Output`).
pub(crate) const READ_BUF: usize = 8192;

/// One ordered item from a source's reader — see [`SourceReader`].
///
/// # Ordering invariant
///
/// Resizes travel **in-band**, interleaved with the output bytes, because the
/// source serializes them that way and the session's screen model must apply
/// them in exactly that order: a resize applied early or late relative to the
/// redraw the resized program emits would wrap every long line at the wrong
/// width, and the mis-wrapped grid would then be minted into keyframes viewers
/// replay. A spawned subshell never emits `Resize` (every resize it
/// experiences is one the session applied *to* it); a source that mirrors a
/// terminal owned by someone else (a tapped proxy) reports the owner's
/// geometry changes here, at their exact position in the stream.
pub(crate) enum ReadEvent {
    /// Raw source output bytes, never empty.
    Output(Vec<u8>),
    /// The source's terminal changed size at exactly this point in the
    /// output stream.
    Resize(Size),
}

/// Blocking reader of a source's ordered output stream; runs on the detached
/// pty-reader thread. `Ok(None)` is the end of the source; like a plain
/// `Read`, it must keep honouring calls after the session stops listening
/// (drain-only mode; see `pty_reader_loop`).
pub(crate) trait SourceReader: Send {
    /// Block until the source yields its next event, ends (`Ok(None)`), or
    /// fails.
    ///
    /// # Errors
    ///
    /// An I/O error reading the source; treated exactly like `Ok(None)`.
    fn read_event(&mut self) -> io::Result<Option<ReadEvent>>;
}

/// Adapts a plain blocking byte reader (the subshell's PTY master) into a
/// [`SourceReader`]: a byte stream carries no geometry, so every event is
/// `Output`.
pub(crate) struct ByteReader<R>(pub(crate) R);

impl<R: Read + Send> SourceReader for ByteReader<R> {
    fn read_event(&mut self) -> io::Result<Option<ReadEvent>> {
        let mut buf = [0u8; READ_BUF];
        match self.0.read(&mut buf)? {
            0 => Ok(None),
            n => Ok(Some(ReadEvent::Output(buf[..n].to_vec()))),
        }
    }
}

/// A source, split into the pieces the session's task topology needs.
///
/// Each field's invariant is load-bearing — `Session::run` is built on them:
///
/// * [`reader`](Self::reader) — blocking reader of the source's ordered
///   output/resize stream; runs on the detached pty-reader thread. It must
///   keep honouring reads after the session stops listening (drain-only mode;
///   see `pty_reader_loop`) and signal the end of the source with `Ok(None)`
///   or `Err`.
/// * [`writer`](Self::writer) — the sole writer of the source's input; runs on
///   the detached pty-writer thread, so it may block freely.
/// * [`resizer`](Self::resizer) — applies a negotiated size to the source.
///   Best-effort and must not block indefinitely; a source that does not own
///   its terminal supplies a no-op.
/// * [`stop`](Self::stop) — ends the session's use of the source. It must
///   never kill a process the source does not own: a spawned subshell is the
///   source's child and is killed; a tap onto someone else's terminal only
///   detaches.
/// * [`wait`](Self::wait) — blocks until the source is finished and returns
///   the session exit code. Runs on the blocking pool, so it must be
///   blocking-safe, and it must return once `stop` has taken effect.
/// * [`bootstrap`](Self::bootstrap) — bytes fed through the screen model
///   before the select loop starts, as chunk 0 (a snapshot of what the
///   source's terminal already shows). `None` for a source that starts blank.
/// * [`answer_queries`](Self::answer_queries) — whether the session must
///   answer terminal probes (CPR / DA) synthetically. True when the
///   compositing model swallows the source's output so the real terminal
///   never sees the probe; false when a real terminal is answering for
///   itself — a synthetic reply there would double-answer.
/// * [`follows_hub_resize`](Self::follows_hub_resize) — whether the hub's
///   `set_size` negotiation is applied to the source. True for a subshell the
///   session owns; false when the source's own terminal is authoritative and
///   the hub's ask must be ignored.
pub(crate) struct SourceParts {
    /// Blocking reader of the source's ordered stream — see the module
    /// invariants and [`ReadEvent`].
    pub(crate) reader: Box<dyn SourceReader>,
    /// The sole writer of the source's input.
    pub(crate) writer: Box<dyn Write + Send>,
    /// Applies a negotiated size to the source; no-op when it owns none.
    pub(crate) resizer: Box<dyn Fn(Size) + Send>,
    /// Ends the session's use of the source — never kills what it does not own.
    pub(crate) stop: Box<dyn FnMut() + Send>,
    /// Blocks until the source is finished; returns the session exit code.
    pub(crate) wait: Box<dyn FnOnce() -> i32 + Send>,
    /// Chunk 0: what the source's terminal already showed, or `None`.
    pub(crate) bootstrap: Option<Vec<u8>>,
    /// Whether the session answers CPR/DA probes synthetically.
    pub(crate) answer_queries: bool,
    /// Whether hub `set_size` negotiation is applied to the source.
    pub(crate) follows_hub_resize: bool,
}

/// Something the session can share: consumed into its [`SourceParts`] once,
/// right before `Session::run` takes over.
pub(crate) trait SessionSource {
    /// Split the source into the pieces the session's topology needs.
    ///
    /// # Errors
    ///
    /// Returns an error when the source cannot be split — e.g. a connection
    /// handle that cannot be duplicated.
    fn into_parts(self) -> crate::Result<SourceParts>;
}
