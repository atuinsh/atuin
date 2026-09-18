//! TODO(markovejnovic): This is complete slop, I have no time to review this in-depth.
//! Following a growing file, `tail -f`-style.
//!
//! ```no_run
//! use atuin_common::fs::tail::{Anchor, Read, Tail};
//! use futures::StreamExt;
//!
//! # async fn example() -> std::io::Result<()> {
//! // Follow a log file as UTF-8 lines, like `tail -f`.
//! let mut lines = std::pin::pin!(Tail::builder().path("/var/log/app.log").build().lines_utf8());
//! while let Some(line) = lines.next().await {
//!     println!("{}", line?);
//! }
//!
//! // Or read an existing file once, to completion, as raw byte lines.
//! let bounded = Tail::builder().path("data.txt").read(Read::Once(Anchor::Beginning)).build();
//! let mut lines = std::pin::pin!(bounded.lines());
//! while let Some(line) = lines.next().await {
//!     let _line: Vec<u8> = line?;
//! }
//! # Ok(())
//! # }
//! ```

use std::io::{self, SeekFrom};
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;
use std::time::Duration;

use futures::{Stream, StreamExt};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use typed_builder::TypedBuilder;

use crate::futures::{Backoff, Schedule};
use crate::os::fs::{FdIdentity, FdIdentityExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineTooLong {
    pub(crate) len: usize,
}

impl From<LineTooLong> for io::Error {
    fn from(LineTooLong { len }: LineTooLong) -> Self {
        Self::new(io::ErrorKind::InvalidData, format!("line exceeds maximum length ({len} bytes)"))
    }
}

#[derive(Debug)]
pub(crate) struct LineAccumulator {
    carry: Vec<u8>,
    start: usize,
    max: Option<usize>,
    discarding: bool,
    consumed: u64,
}

impl LineAccumulator {
    pub(crate) fn new(max: Option<usize>) -> Self {
        Self {
            carry: Vec::new(),
            start: 0,
            max,
            discarding: false,
            consumed: 0,
        }
    }

    pub(crate) fn push(&mut self, buf: &[u8]) {
        if self.start > 0 {
            self.carry.drain(..self.start);
            self.start = 0;
        }
        self.carry.extend_from_slice(buf);
    }

    fn advance(&mut self, bytes: usize) {
        self.start += bytes;
        self.consumed += u64::try_from(bytes).expect("consumed byte count fits u64");
    }

    pub(crate) fn consumed(&self) -> u64 {
        self.consumed
    }

    pub(crate) fn next_line(&mut self) -> Option<Result<Vec<u8>, LineTooLong>> {
        loop {
            let newline = memchr::memchr(b'\n', &self.carry[self.start..]);

            if self.discarding {
                match newline {
                    Some(pos) => {
                        self.advance(pos + 1);
                        self.discarding = false;
                    }
                    None => {
                        let len = self.carry.len() - self.start;
                        self.advance(len);
                        return None;
                    }
                }
                continue;
            }

            match newline {
                Some(pos) => {
                    if self.max.is_some_and(|max| pos > max) {
                        self.advance(pos + 1);
                        return Some(Err(LineTooLong { len: pos }));
                    }
                    let line = self.carry[self.start..self.start + pos].to_vec();
                    self.advance(pos + 1);
                    return Some(Ok(line));
                }
                None => {
                    let len = self.carry.len() - self.start;
                    if self.max.is_some_and(|max| len > max) {
                        self.advance(len);
                        self.discarding = true;
                        return Some(Err(LineTooLong { len }));
                    }
                    return None;
                }
            }
        }
    }

    pub(crate) fn finish(&mut self) -> Option<Result<Vec<u8>, LineTooLong>> {
        let len = self.carry.len() - self.start;
        if self.discarding {
            self.advance(len);
            return None;
        }
        if len == 0 {
            return None;
        }
        if self.max.is_some_and(|max| len > max) {
            self.advance(len);
            return Some(Err(LineTooLong { len }));
        }
        let line = self.carry[self.start..].to_vec();
        self.advance(len);
        Some(Ok(line))
    }

    pub(crate) fn reset(&mut self) {
        self.carry.clear();
        self.start = 0;
        self.discarding = false;
        self.consumed = 0;
    }
}

/// Where a bounded [`Read::Once`] begins reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// From the start of the file.
    Beginning,
    /// From a specific byte offset, e.g. a persisted resume point.
    Offset(u64),
}

/// Where a following [`Read::Follow`] begins reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// From the start of the file.
    Beginning,
    /// From the current end, emitting only what is appended afterwards.
    End,
    /// From a specific byte offset, e.g. a persisted resume point.
    Offset(u64),
}

/// How a [`Tail`] consumes its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    /// Read existing content from `Anchor`, then stop at end of file.
    Once(Anchor),
    /// Follow the file from `Start`, waiting for data appended afterwards.
    Follow(Start),
}

/// How a [`Tail`] reacts when the file at its path is replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    /// Keep following the originally opened file (`tail -f`).
    Fd,
    /// Reopen the path when it is replaced by a different file (`tail -F`).
    Name,
}

/// A [`Tail`] item paired with the byte offset immediately after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Positioned<T> {
    /// Byte offset just past `value`; use as an [`Anchor::Offset`] / [`Start::Offset`] to resume here.
    pub offset: u64,
    /// The item at this position.
    pub value: T,
}

#[derive(Debug)]
enum Event {
    Data {
        bytes: Vec<u8>,
        offset: u64,
    },
    /// The file was truncated or rewritten under us; discard any buffered partial line.
    Reset,
    /// The path was rotated to a new file; flush the old file's buffered partial line first.
    Rotated,
    Error(io::Error),
}

#[derive(Debug, Clone, Copy)]
enum ResetKind {
    Discard,
    Flush,
}

trait TailSource: Send {
    fn read_at(
        &mut self,
        offset: u64,
        buf: &mut [u8],
    ) -> impl std::future::Future<Output = io::Result<usize>> + Send;

    fn size(&mut self) -> impl std::future::Future<Output = io::Result<u64>> + Send;

    fn identity(&mut self) -> impl std::future::Future<Output = io::Result<FdIdentity>> + Send;

    fn path_identity(
        &mut self,
    ) -> impl std::future::Future<Output = io::Result<Option<FdIdentity>>> + Send;

    fn reopen(&mut self) -> impl std::future::Future<Output = io::Result<()>> + Send;
}

trait Waiter: Send {
    fn wait(&mut self) -> impl std::future::Future<Output = ()> + Send;
    fn reset(&mut self);
}

struct BackoffWaiter {
    schedule: Schedule,
}

impl BackoffWaiter {
    fn new(backoff: Backoff) -> Self {
        Self {
            schedule: backoff.schedule(),
        }
    }
}

impl Waiter for BackoffWaiter {
    async fn wait(&mut self) {
        tokio::time::sleep(self.schedule.next_delay()).await;
    }

    fn reset(&mut self) {
        self.schedule.reset();
    }
}

struct FileSource {
    file: tokio::fs::File,
    path: PathBuf,
    pos: u64,
}

impl FileSource {
    async fn open(path: PathBuf) -> io::Result<Self> {
        let file = tokio::fs::File::open(&path).await?;
        if !file.metadata().await?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "tail target is not a regular file",
            ));
        }
        Ok(Self { file, path, pos: 0 })
    }
}

impl TailSource for FileSource {
    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        // Reads walk forward sequentially, so the cursor is usually already at `offset`; only
        // seek when it is not (resume, anchor probe, rotation).
        if self.pos != offset {
            self.file.seek(SeekFrom::Start(offset)).await?;
            self.pos = offset;
        }
        let n = self.file.read(buf).await?;
        self.pos += u64::try_from(n).expect("read count fits u64");
        Ok(n)
    }

    async fn size(&mut self) -> io::Result<u64> {
        Ok(self.file.metadata().await?.len())
    }

    async fn identity(&mut self) -> io::Result<FdIdentity> {
        self.file.identity()
    }

    async fn path_identity(&mut self) -> io::Result<Option<FdIdentity>> {
        match tokio::fs::File::open(&self.path).await {
            Ok(file) => Ok(Some(file.identity()?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn reopen(&mut self) -> io::Result<()> {
        self.file = tokio::fs::File::open(&self.path).await?;
        self.pos = 0;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct EngineCfg {
    read: Read,
    rotation: Rotation,
    read_size: NonZeroUsize,
    backoff: Backoff,
}

async fn anchor_mismatch<S: TailSource>(src: &mut S, offset: u64, last_byte: Option<u8>) -> bool {
    let Some(expected) = last_byte else {
        return false;
    };
    let mut one = [0u8; 1];
    match src.read_at(offset - 1, &mut one).await {
        Ok(1) => one[0] != expected,
        Ok(_) => true,
        Err(_) => false,
    }
}

fn follow<S, W>(mut src: S, mut waiter: W, cfg: EngineCfg) -> impl Stream<Item = Event> + Send
where
    S: TailSource + 'static,
    W: Waiter + 'static,
{
    async_stream::stream! {
        let mut offset: u64 = match cfg.read {
            Read::Once(Anchor::Beginning) | Read::Follow(Start::Beginning) => 0,
            Read::Once(Anchor::Offset(at)) | Read::Follow(Start::Offset(at)) => at,
            Read::Follow(Start::End) => match src.size().await {
                Ok(size) => size,
                Err(e) => {
                    yield Event::Error(e);
                    return;
                }
            },
        };
        let follow = matches!(cfg.read, Read::Follow(_));

        let mut open_id = if cfg.rotation == Rotation::Name {
            src.identity().await.ok()
        } else {
            None
        };
        let mut last_byte: Option<u8> = None;
        let mut buf = vec![0u8; cfg.read_size.get()];

        'follow: loop {
            loop {
                match src.read_at(offset, &mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        offset += u64::try_from(n).expect("read count fits u64");
                        last_byte = Some(buf[n - 1]);
                        yield Event::Data { bytes: buf[..n].to_vec(), offset };
                    }
                    Err(e) => {
                        yield Event::Error(e);
                        if !follow {
                            return;
                        }
                        waiter.wait().await;
                        continue 'follow;
                    }
                }
            }

            if !follow {
                return;
            }
            waiter.reset();

            let reset = loop {
                match src.size().await {
                    Ok(size) if size < offset => break Some(ResetKind::Discard),
                    Ok(size) => {
                        if anchor_mismatch(&mut src, offset, last_byte).await {
                            break Some(ResetKind::Discard);
                        }
                        if size > offset {
                            break None;
                        }
                    }
                    Err(_) => {}
                }

                if cfg.rotation == Rotation::Name {
                    let path_id = src.path_identity().await.ok().flatten();
                    if matches!((path_id, open_id), (Some(path), Some(open)) if path != open) {
                        loop {
                            match src.read_at(offset, &mut buf).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    offset += u64::try_from(n).expect("read count fits u64");
                                    last_byte = Some(buf[n - 1]);
                                    yield Event::Data { bytes: buf[..n].to_vec(), offset };
                                }
                                Err(e) => {
                                    yield Event::Error(e);
                                    break;
                                }
                            }
                        }
                        if let Err(e) = src.reopen().await {
                            yield Event::Error(e);
                            waiter.wait().await;
                            continue;
                        }
                        open_id = src.identity().await.ok();
                        break Some(ResetKind::Flush);
                    }
                }

                waiter.wait().await;
            };

            if let Some(kind) = reset {
                yield match kind {
                    ResetKind::Flush => Event::Rotated,
                    ResetKind::Discard => Event::Reset,
                };
                offset = 0;
                last_byte = None;
            }
        }
    }
}

fn default_read_size() -> NonZeroUsize {
    NonZeroUsize::new(64 * 1024).expect("64 KiB is nonzero")
}

fn default_backoff() -> Backoff {
    Backoff::Exponential {
        initial: Duration::from_millis(25),
        max: Duration::from_secs(1),
        factor: NonZeroU32::new(2).expect("2 is nonzero"),
    }
}

/// A `tail -f`-style follower for a single file, built via [`Tail::builder`].
#[derive(Debug, Clone, TypedBuilder)]
pub struct Tail {
    #[builder(setter(into))]
    path: PathBuf,
    #[builder(default = Read::Follow(Start::Beginning))]
    read: Read,
    #[builder(default = Rotation::Fd)]
    rotation: Rotation,
    #[builder(default = default_read_size())]
    read_size: NonZeroUsize,
    #[builder(default, setter(strip_option))]
    max_line_len: Option<usize>,
    #[builder(default = default_backoff())]
    backoff: Backoff,
}

impl Tail {
    fn engine_cfg(&self) -> EngineCfg {
        EngineCfg {
            read: self.read,
            rotation: self.rotation,
            read_size: self.read_size,
            backoff: self.backoff,
        }
    }

    fn events(self) -> impl Stream<Item = Event> + Send {
        let cfg = self.engine_cfg();
        let path = self.path;
        async_stream::stream! {
            let mut schedule = cfg.backoff.schedule();
            let src = loop {
                match FileSource::open(path.clone()).await {
                    Ok(src) => break src,
                    Err(e)
                        if cfg.rotation == Rotation::Name
                            && e.kind() == io::ErrorKind::NotFound =>
                    {
                        tokio::time::sleep(schedule.next_delay()).await;
                    }
                    Err(e) => {
                        yield Event::Error(e);
                        return;
                    }
                }
            };
            let inner = follow(src, BackoffWaiter::new(cfg.backoff), cfg);
            futures::pin_mut!(inner);
            while let Some(event) = inner.next().await {
                yield event;
            }
        }
    }

    /// Stream the file as raw read chunks of bytes.
    pub fn chunks(self) -> impl Stream<Item = io::Result<Vec<u8>>> + Send {
        let events = self.events();
        async_stream::stream! {
            futures::pin_mut!(events);
            while let Some(event) = events.next().await {
                match event {
                    Event::Data { bytes, .. } => yield Ok(bytes),
                    Event::Error(e) => yield Err(e),
                    Event::Reset | Event::Rotated => {}
                }
            }
        }
    }

    /// Stream the file as newline-delimited byte lines, with the trailing `\n` stripped.
    pub fn lines(self) -> impl Stream<Item = io::Result<Vec<u8>>> + Send {
        line_bytes_stream(self.max_line_len, self.events())
    }

    /// Stream the file as UTF-8 line strings; invalid UTF-8 yields an error.
    pub fn lines_utf8(self) -> impl Stream<Item = io::Result<String>> + Send {
        let lines = self.lines();
        async_stream::stream! {
            futures::pin_mut!(lines);
            while let Some(line) = lines.next().await {
                yield line.and_then(decode_utf8);
            }
        }
    }

    /// Stream byte lines, each paired with the byte offset just past it for resumable follows.
    pub fn lines_positioned(self) -> impl Stream<Item = Positioned<io::Result<Vec<u8>>>> + Send {
        let max = self.max_line_len;
        let events = self.events();
        async_stream::stream! {
            let mut acc = LineAccumulator::new(max);
            let mut epoch_start: Option<u64> = None;
            futures::pin_mut!(events);
            while let Some(event) = events.next().await {
                match event {
                    Event::Data { bytes, offset } => {
                        if epoch_start.is_none() {
                            let len = u64::try_from(bytes.len()).expect("chunk len fits u64");
                            epoch_start = Some(offset - len);
                        }
                        acc.push(&bytes);
                        while let Some(line) = acc.next_line() {
                            let offset = epoch_start.unwrap_or(0) + acc.consumed();
                            yield Positioned { offset, value: line.map_err(io::Error::from) };
                        }
                    }
                    Event::Rotated => {
                        if let Some(line) = acc.finish() {
                            let offset = epoch_start.unwrap_or(0) + acc.consumed();
                            yield Positioned { offset, value: line.map_err(io::Error::from) };
                        }
                        acc.reset();
                        epoch_start = None;
                    }
                    Event::Reset => {
                        acc.reset();
                        epoch_start = None;
                    }
                    Event::Error(e) => {
                        let offset = epoch_start.unwrap_or(0) + acc.consumed();
                        yield Positioned { offset, value: Err(e) };
                    }
                }
            }
            if let Some(line) = acc.finish() {
                let offset = epoch_start.unwrap_or(0) + acc.consumed();
                yield Positioned { offset, value: line.map_err(io::Error::from) };
            }
        }
    }
}

fn line_bytes_stream(
    max: Option<usize>,
    events: impl Stream<Item = Event> + Send + 'static,
) -> impl Stream<Item = io::Result<Vec<u8>>> + Send {
    async_stream::stream! {
        let mut acc = LineAccumulator::new(max);
        futures::pin_mut!(events);
        while let Some(event) = events.next().await {
            match event {
                Event::Data { bytes, .. } => {
                    acc.push(&bytes);
                    while let Some(line) = acc.next_line() {
                        yield line.map_err(io::Error::from);
                    }
                }
                Event::Rotated => {
                    if let Some(line) = acc.finish() {
                        yield line.map_err(io::Error::from);
                    }
                    acc.reset();
                }
                Event::Reset => acc.reset(),
                Event::Error(e) => yield Err(e),
            }
        }
        if let Some(line) = acc.finish() {
            yield line.map_err(io::Error::from);
        }
    }
}

fn decode_utf8(bytes: Vec<u8>) -> io::Result<String> {
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use futures::channel::mpsc;
    use futures::task::noop_waker;
    use parking_lot::Mutex;
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    type Line = Result<Vec<u8>, LineTooLong>;

    fn run(input: &[u8], chunks: &[usize], max: Option<usize>) -> (Vec<Line>, Option<Line>) {
        let mut acc = LineAccumulator::new(max);
        let mut out = Vec::new();
        let mut at = 0;
        for &chunk in chunks {
            let end = (at + chunk).min(input.len());
            acc.push(&input[at..end]);
            while let Some(line) = acc.next_line() {
                out.push(line);
            }
            at = end;
        }
        if at < input.len() {
            acc.push(&input[at..]);
            while let Some(line) = acc.next_line() {
                out.push(line);
            }
        }
        (out, acc.finish())
    }

    fn drain_all(acc: &mut LineAccumulator, pushes: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut lines = Vec::new();
        for p in pushes {
            acc.push(p);
            while let Some(line) = acc.next_line() {
                lines.push(line.expect("no cap set"));
            }
        }
        lines
    }

    #[rstest]
    fn emits_a_complete_line_without_its_newline() {
        let mut acc = LineAccumulator::new(None);
        acc.push(b"hello\n");
        assert_eq!(acc.next_line(), Some(Ok(b"hello".to_vec())));
        assert_eq!(acc.next_line(), None);
    }

    #[rstest]
    #[case(vec![b"a\nb\nc".as_slice()], vec![b"a".to_vec(), b"b".to_vec()])]
    #[case(vec![b"hell".as_slice(), b"o\n".as_slice()], vec![b"hello".to_vec()])]
    #[case(vec![b"\n\n".as_slice()], vec![vec![], vec![]])]
    #[case(vec![b"".as_slice()], vec![])]
    #[case(vec![b"a\n".as_slice(), b"".as_slice(), b"b\n".as_slice()], vec![b"a".to_vec(), b"b".to_vec()])]
    fn drains_only_complete_lines(#[case] pushes: Vec<&[u8]>, #[case] expected: Vec<Vec<u8>>) {
        let mut acc = LineAccumulator::new(None);
        assert_eq!(drain_all(&mut acc, &pushes), expected);
    }

    #[rstest]
    fn finish_emits_the_trailing_partial_line_once() {
        let mut acc = LineAccumulator::new(None);
        acc.push(b"tail");
        assert_eq!(acc.next_line(), None);
        assert_eq!(acc.finish(), Some(Ok(b"tail".to_vec())));
        assert_eq!(acc.finish(), None);
    }

    #[rstest]
    fn finish_emits_nothing_after_a_clean_newline() {
        let mut acc = LineAccumulator::new(None);
        acc.push(b"a\n");
        assert_eq!(acc.next_line(), Some(Ok(b"a".to_vec())));
        assert_eq!(acc.finish(), None);
    }

    #[rstest]
    fn reset_discards_the_pending_partial() {
        let mut acc = LineAccumulator::new(None);
        acc.push(b"half");
        acc.reset();
        acc.push(b"whole\n");
        assert_eq!(acc.next_line(), Some(Ok(b"whole".to_vec())));
    }

    #[rstest]
    fn a_line_over_the_cap_errors_then_recovers() {
        let mut acc = LineAccumulator::new(Some(3));
        acc.push(b"toolong\nok\n");
        assert_eq!(acc.next_line(), Some(Err(LineTooLong { len: 7 })));
        assert_eq!(acc.next_line(), Some(Ok(b"ok".to_vec())));
    }

    #[rstest]
    fn a_line_exactly_at_the_cap_is_kept() {
        let mut acc = LineAccumulator::new(Some(3));
        acc.push(b"abc\n");
        assert_eq!(acc.next_line(), Some(Ok(b"abc".to_vec())));
    }

    #[rstest]
    fn an_unterminated_over_cap_run_errors_then_discards_until_newline() {
        let mut acc = LineAccumulator::new(Some(3));
        acc.push(b"aaaa");
        assert_eq!(acc.next_line(), Some(Err(LineTooLong { len: 4 })));
        assert_eq!(acc.next_line(), None);
        acc.push(b"more");
        assert_eq!(acc.next_line(), None);
        acc.push(b"junk\nok\n");
        assert_eq!(acc.next_line(), Some(Ok(b"ok".to_vec())));
    }

    fn bytes_biased_to_newlines() -> impl Strategy<Value = Vec<u8>> {
        prop::collection::vec(prop_oneof![3 => any::<u8>(), 1 => Just(b'\n')], 0..80)
    }

    proptest! {
        #[test]
        fn reconstructs_input_regardless_of_chunking(
            input in bytes_biased_to_newlines(),
            chunks in prop::collection::vec(1usize..8, 0..40),
        ) {
            let (lines, tail) = run(&input, &chunks, None);
            let mut rebuilt = Vec::new();
            for line in &lines {
                let line = line.as_ref().expect("no cap set");
                prop_assert!(!line.contains(&b'\n'));
                rebuilt.extend_from_slice(line);
                rebuilt.push(b'\n');
            }
            if let Some(tail) = tail {
                rebuilt.extend_from_slice(&tail.expect("no cap set"));
            }
            prop_assert_eq!(rebuilt, input);
        }

        #[test]
        fn line_splitting_is_independent_of_push_boundaries(
            input in bytes_biased_to_newlines(),
            chunks_a in prop::collection::vec(1usize..8, 0..40),
            chunks_b in prop::collection::vec(1usize..13, 0..40),
        ) {
            prop_assert_eq!(run(&input, &chunks_a, None), run(&input, &chunks_b, None));
        }

        #[test]
        fn capped_ok_lines_never_exceed_the_cap(
            input in bytes_biased_to_newlines(),
            chunks in prop::collection::vec(1usize..8, 0..40),
            max in 1usize..10,
        ) {
            let (lines, tail) = run(&input, &chunks, Some(max));
            for line in lines.into_iter().chain(tail).flatten() {
                prop_assert!(line.len() <= max);
                prop_assert!(!line.contains(&b'\n'));
            }
        }
    }

    async fn collect_lines(stream: impl Stream<Item = io::Result<String>>) -> Vec<String> {
        futures::pin_mut!(stream);
        let mut out = Vec::new();
        while let Some(item) = stream.next().await {
            out.push(item.expect("line decodes"));
        }
        out
    }

    #[rstest]
    #[tokio::test]
    async fn reads_existing_lines_without_following() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"one\ntwo\nthree\n").unwrap();
        let lines = collect_lines(
            Tail::builder().path(&path).read(Read::Once(Anchor::Beginning)).build().lines_utf8(),
        )
        .await;
        assert_eq!(lines, vec!["one", "two", "three"]);
    }

    #[rstest]
    #[tokio::test]
    async fn emits_the_unterminated_trailing_line_when_not_following() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"a\nb\nno-newline").unwrap();
        let lines = collect_lines(
            Tail::builder().path(&path).read(Read::Once(Anchor::Beginning)).build().lines_utf8(),
        )
        .await;
        assert_eq!(lines, vec!["a", "b", "no-newline"]);
    }

    #[rstest]
    fn follow_from_end_skips_existing_content() {
        let (tx, rx) = mpsc::unbounded();
        let (handle, src) = MemHandle::new(b"old\n");
        let cfg = EngineCfg {
            read: Read::Follow(Start::End),
            rotation: Rotation::Fd,
            read_size: NonZeroUsize::new(4).expect("4 is nonzero"),
            backoff: default_backoff(),
        };
        let mut stream: EventStream = Box::pin(follow(src, ManualWaiter { rx }, cfg));
        assert_eq!(data_of(&drain(&mut stream)), b"");
        handle.append(b"new\n");
        tx.unbounded_send(()).unwrap();
        assert_eq!(data_of(&drain(&mut stream)), b"new\n");
    }

    #[rstest]
    #[tokio::test]
    async fn a_missing_file_yields_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope");
        let stream =
            Tail::builder().path(&path).read(Read::Once(Anchor::Beginning)).build().lines_utf8();
        futures::pin_mut!(stream);
        assert!(stream.next().await.expect("one item").is_err());
    }

    #[derive(Debug, Clone)]
    enum Op {
        Append(Vec<u8>),
        Rotate,
    }

    struct MemWorld {
        files: HashMap<FdIdentity, Vec<u8>>,
        path_id: FdIdentity,
        open_id: FdIdentity,
        next: u64,
    }

    #[derive(Clone)]
    struct MemSource {
        world: Arc<Mutex<MemWorld>>,
    }

    struct MemHandle {
        world: Arc<Mutex<MemWorld>>,
    }

    impl MemHandle {
        fn new(initial: &[u8]) -> (Self, MemSource) {
            let id = FdIdentity::from_raw(0, 0);
            let mut files = HashMap::new();
            files.insert(id, initial.to_vec());
            let world = Arc::new(Mutex::new(MemWorld {
                files,
                path_id: id,
                open_id: id,
                next: 1,
            }));
            (
                Self {
                    world: world.clone(),
                },
                MemSource { world },
            )
        }

        fn append(&self, data: &[u8]) {
            let mut world = self.world.lock();
            let id = world.path_id;
            world.files.get_mut(&id).expect("path file exists").extend_from_slice(data);
        }

        fn truncate(&self, len: usize) {
            let mut world = self.world.lock();
            let id = world.path_id;
            world.files.get_mut(&id).expect("path file exists").truncate(len);
        }

        fn rotate(&self) {
            let mut world = self.world.lock();
            let id = FdIdentity::from_raw(0, world.next);
            world.next += 1;
            world.files.insert(id, Vec::new());
            world.path_id = id;
        }

        fn rewrite(&self, data: &[u8]) {
            let mut world = self.world.lock();
            let id = world.path_id;
            *world.files.get_mut(&id).expect("path file exists") = data.to_vec();
        }
    }

    impl TailSource for MemSource {
        async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            let start = usize::try_from(offset).expect("offset fits usize");
            let world = self.world.lock();
            let data = world.files.get(&world.open_id).expect("open file exists");
            let n = buf.len().min(data.len().saturating_sub(start));
            buf[..n].copy_from_slice(&data[start..start + n]);
            drop(world);
            Ok(n)
        }

        async fn size(&mut self) -> io::Result<u64> {
            let len = {
                let world = self.world.lock();
                world.files.get(&world.open_id).expect("open file exists").len()
            };
            Ok(u64::try_from(len).expect("len fits u64"))
        }

        async fn identity(&mut self) -> io::Result<FdIdentity> {
            Ok(self.world.lock().open_id)
        }

        async fn path_identity(&mut self) -> io::Result<Option<FdIdentity>> {
            Ok(Some(self.world.lock().path_id))
        }

        async fn reopen(&mut self) -> io::Result<()> {
            let mut world = self.world.lock();
            world.open_id = world.path_id;
            drop(world);
            Ok(())
        }
    }

    struct ManualWaiter {
        rx: mpsc::UnboundedReceiver<()>,
    }

    impl Waiter for ManualWaiter {
        async fn wait(&mut self) {
            let _ = self.rx.next().await;
        }

        fn reset(&mut self) {}
    }

    type EventStream = Pin<Box<dyn Stream<Item = Event> + Send>>;

    fn test_cfg(rotation: Rotation) -> EngineCfg {
        EngineCfg {
            read: Read::Follow(Start::Beginning),
            rotation,
            read_size: NonZeroUsize::new(4).expect("4 is nonzero"),
            backoff: default_backoff(),
        }
    }

    fn follow_mem(
        src: MemSource,
        rx: mpsc::UnboundedReceiver<()>,
        rotation: Rotation,
    ) -> EventStream {
        Box::pin(follow(src, ManualWaiter { rx }, test_cfg(rotation)))
    }

    fn drain(stream: &mut EventStream) -> Vec<Event> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut out = Vec::new();
        while let Poll::Ready(Some(event)) = stream.as_mut().poll_next(&mut cx) {
            out.push(event);
        }
        out
    }

    type LineStream = Pin<Box<dyn Stream<Item = io::Result<Vec<u8>>> + Send>>;

    fn drain_lines(stream: &mut LineStream) -> Vec<Vec<u8>> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut out = Vec::new();
        while let Poll::Ready(Some(line)) = stream.as_mut().poll_next(&mut cx) {
            out.push(line.expect("line is ok"));
        }
        out
    }

    fn data_of(events: &[Event]) -> Vec<u8> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Data { bytes, .. } => Some(bytes.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn resets(events: &[Event]) -> usize {
        events.iter().filter(|event| matches!(event, Event::Reset)).count()
    }

    fn rotations(events: &[Event]) -> usize {
        events.iter().filter(|event| matches!(event, Event::Rotated)).count()
    }

    #[rstest]
    fn follow_delivers_bytes_appended_after_eof() {
        let (tx, rx) = mpsc::unbounded();
        let (handle, src) = MemHandle::new(b"one\n");
        let mut stream = follow_mem(src, rx, Rotation::Fd);
        assert_eq!(data_of(&drain(&mut stream)), b"one\n");
        handle.append(b"two\n");
        tx.unbounded_send(()).unwrap();
        assert_eq!(data_of(&drain(&mut stream)), b"two\n");
    }

    #[rstest]
    fn follow_resets_and_rereads_on_truncation() {
        let (tx, rx) = mpsc::unbounded();
        let (handle, src) = MemHandle::new(b"aaaa\n");
        let mut stream = follow_mem(src, rx, Rotation::Fd);
        assert_eq!(data_of(&drain(&mut stream)), b"aaaa\n");
        handle.truncate(0);
        handle.append(b"bb\n");
        tx.unbounded_send(()).unwrap();
        let events = drain(&mut stream);
        assert_eq!(resets(&events), 1);
        assert_eq!(data_of(&events), b"bb\n");
    }

    #[rstest]
    fn follow_resets_on_in_place_rewrite_past_the_offset() {
        let (tx, rx) = mpsc::unbounded();
        let (handle, src) = MemHandle::new(b"aaaa\n");
        let mut stream = follow_mem(src, rx, Rotation::Fd);
        assert_eq!(data_of(&drain(&mut stream)), b"aaaa\n");
        handle.truncate(0);
        handle.append(b"bbbbbbbb\n");
        tx.unbounded_send(()).unwrap();
        let events = drain(&mut stream);
        assert_eq!(resets(&events), 1);
        assert_eq!(data_of(&events), b"bbbbbbbb\n");
    }

    #[rstest]
    fn follow_name_drains_the_old_file_before_switching() {
        let (tx, rx) = mpsc::unbounded();
        let (handle, src) = MemHandle::new(b"old1\n");
        let mut stream = follow_mem(src, rx, Rotation::Name);
        assert_eq!(data_of(&drain(&mut stream)), b"old1\n");
        handle.append(b"old2\n");
        handle.rotate();
        handle.append(b"new1\n");
        tx.unbounded_send(()).unwrap();
        let events = drain(&mut stream);
        assert_eq!(data_of(&events), b"old2\nnew1\n");
        assert_eq!(rotations(&events), 1);
        assert_eq!(resets(&events), 0);
    }

    #[rstest]
    fn rotation_flushes_the_old_files_unterminated_final_line() {
        let (tx, rx) = mpsc::unbounded();
        let (handle, src) = MemHandle::new(b"old1\nold2");
        let mut lines: LineStream =
            Box::pin(line_bytes_stream(None, follow_mem(src, rx, Rotation::Name)));
        assert_eq!(drain_lines(&mut lines), vec![b"old1".to_vec()]);
        handle.rotate();
        handle.append(b"new1\n");
        tx.unbounded_send(()).unwrap();
        assert_eq!(drain_lines(&mut lines), vec![b"old2".to_vec(), b"new1".to_vec()]);
    }

    #[rstest]
    fn follow_detects_a_same_length_rewrite() {
        let (tx, rx) = mpsc::unbounded();
        let (handle, src) = MemHandle::new(b"aaaa\n");
        let mut stream = follow_mem(src, rx, Rotation::Fd);
        assert_eq!(data_of(&drain(&mut stream)), b"aaaa\n");
        handle.rewrite(b"bbbbX");
        tx.unbounded_send(()).unwrap();
        let events = drain(&mut stream);
        assert_eq!(resets(&events), 1);
        assert_eq!(data_of(&events), b"bbbbX");
    }

    #[rstest]
    #[tokio::test]
    async fn name_follow_waits_for_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("later.log");
        let stream = Tail::builder().path(&path).rotation(Rotation::Name).build().lines_utf8();
        futures::pin_mut!(stream);
        let create = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            std::fs::write(&path, b"appeared\n").unwrap();
        };
        let (_, line) = tokio::join!(create, stream.next());
        assert_eq!(line.expect("a line").expect("utf8"), "appeared");
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => prop::collection::vec(any::<u8>(), 1..6).prop_map(Op::Append),
            1 => Just(Op::Rotate),
        ]
    }

    proptest! {
        #[test]
        fn append_and_rotate_schedules_lose_no_bytes(
            ops in prop::collection::vec(op_strategy(), 0..40),
        ) {
            let (tx, rx) = mpsc::unbounded();
            let (handle, src) = MemHandle::new(b"");
            let mut stream = follow_mem(src, rx, Rotation::Name);
            let mut written = Vec::new();
            let mut delivered = data_of(&drain(&mut stream));
            for op in ops {
                match op {
                    Op::Append(bytes) => {
                        handle.append(&bytes);
                        written.extend_from_slice(&bytes);
                    }
                    Op::Rotate => handle.rotate(),
                }
                let _ = tx.unbounded_send(());
                delivered.extend_from_slice(&data_of(&drain(&mut stream)));
            }
            prop_assert_eq!(delivered, written);
        }
    }

    #[rstest]
    #[tokio::test]
    async fn lines_positioned_reports_resumable_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"aa\nbbb\nc\n").unwrap();
        let stream = Tail::builder()
            .path(&path)
            .read(Read::Once(Anchor::Beginning))
            .build()
            .lines_positioned();
        futures::pin_mut!(stream);
        let mut got = Vec::new();
        while let Some(Positioned { offset, value }) = stream.next().await {
            got.push((offset, value.unwrap()));
        }
        assert_eq!(got, vec![(3, b"aa".to_vec()), (7, b"bbb".to_vec()), (9, b"c".to_vec())]);
    }

    #[rstest]
    #[tokio::test]
    async fn start_offset_resumes_from_a_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"aa\nbbb\nc\n").unwrap();
        let lines = collect_lines(
            Tail::builder().path(&path).read(Read::Once(Anchor::Offset(7))).build().lines_utf8(),
        )
        .await;
        assert_eq!(lines, vec!["c"]);
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn follows_appends_to_a_real_file() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"one\n").unwrap();
        let stream = Tail::builder().path(&path).build().lines_utf8();
        futures::pin_mut!(stream);
        assert_eq!(stream.next().await.unwrap().unwrap(), "one");
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"two\n").unwrap();
        file.flush().unwrap();
        drop(file);
        assert_eq!(stream.next().await.unwrap().unwrap(), "two");
    }
}
