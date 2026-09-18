use std::io::{self, SeekFrom};
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;
use std::time::Duration;

use futures::{Stream, StreamExt};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use typed_builder::TypedBuilder;

use crate::futures::Backoff;
use crate::futures::stream::ChunkedStream;
use crate::os::fs::{FdIdentity, FdIdentityExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineTooLong {
    pub(crate) len: usize,
}

#[derive(Debug)]
pub(crate) struct LineAccumulator {
    carry: Vec<u8>,
    max: Option<usize>,
    discarding: bool,
    consumed: u64,
}

impl LineAccumulator {
    pub(crate) fn new(max: Option<usize>) -> Self {
        Self {
            carry: Vec::new(),
            max,
            discarding: false,
            consumed: 0,
        }
    }

    pub(crate) fn push(&mut self, buf: &[u8]) {
        self.carry.extend_from_slice(buf);
    }

    fn advance(&mut self, bytes: usize) {
        self.consumed += u64::try_from(bytes).expect("consumed byte count fits u64");
    }

    pub(crate) fn consumed(&self) -> u64 {
        self.consumed
    }

    pub(crate) fn next_line(&mut self) -> Option<Result<Vec<u8>, LineTooLong>> {
        loop {
            let newline = self.carry.iter().position(|&b| b == b'\n');

            if self.discarding {
                match newline {
                    Some(pos) => {
                        self.carry.drain(..=pos);
                        self.advance(pos + 1);
                        self.discarding = false;
                    }
                    None => {
                        let len = self.carry.len();
                        self.carry.clear();
                        self.advance(len);
                        return None;
                    }
                }
                continue;
            }

            match newline {
                Some(pos) => {
                    if self.max.is_some_and(|max| pos > max) {
                        self.carry.drain(..=pos);
                        self.advance(pos + 1);
                        return Some(Err(LineTooLong { len: pos }));
                    }
                    let line: Vec<u8> = self.carry.drain(..=pos).take(pos).collect();
                    self.advance(pos + 1);
                    return Some(Ok(line));
                }
                None => {
                    if self.max.is_some_and(|max| self.carry.len() > max) {
                        let len = self.carry.len();
                        self.carry.clear();
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
        if self.discarding {
            let len = self.carry.len();
            self.carry.clear();
            self.advance(len);
            return None;
        }
        if self.carry.is_empty() {
            return None;
        }
        let len = self.carry.len();
        self.advance(len);
        if self.max.is_some_and(|max| len > max) {
            self.carry.clear();
            return Some(Err(LineTooLong { len }));
        }
        Some(Ok(std::mem::take(&mut self.carry)))
    }

    pub(crate) fn reset(&mut self) {
        self.carry.clear();
        self.discarding = false;
        self.consumed = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    Beginning,
    End,
    Offset(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    Fd,
    Name,
}

#[derive(Debug)]
enum Event {
    Data {
        bytes: Vec<u8>,
        offset: u64,
    },
    Reset,
    Error(io::Error),
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
    backoff: Backoff,
    current: Duration,
}

impl BackoffWaiter {
    fn new(backoff: Backoff) -> Self {
        Self {
            backoff,
            current: Self::initial(backoff),
        }
    }

    fn initial(backoff: Backoff) -> Duration {
        match backoff {
            Backoff::Linear(period) => period,
            Backoff::Exponential { initial, max, .. } => initial.min(max),
        }
    }
}

impl Waiter for BackoffWaiter {
    async fn wait(&mut self) {
        tokio::time::sleep(self.current).await;
        if let Backoff::Exponential { max, factor, .. } = self.backoff {
            self.current = self.current.saturating_mul(factor.get()).min(max);
        }
    }

    fn reset(&mut self) {
        self.current = Self::initial(self.backoff);
    }
}

struct FileSource {
    file: tokio::fs::File,
    path: PathBuf,
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
        Ok(Self { file, path })
    }
}

impl TailSource for FileSource {
    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.file.seek(SeekFrom::Start(offset)).await?;
        self.file.read(buf).await
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
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct EngineCfg {
    start: Start,
    follow: bool,
    rotation: Rotation,
    read_size: NonZeroUsize,
    backoff: Backoff,
}

async fn anchor_mismatch<S: TailSource>(src: &mut S, offset: u64, last_byte: Option<u8>) -> bool {
    let Some(expected) = last_byte else {
        return false;
    };
    if offset == 0 {
        return false;
    }
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
        let mut offset: u64 = match cfg.start {
            Start::Beginning => 0,
            Start::Offset(at) => at,
            Start::End => match src.size().await {
                Ok(size) => size,
                Err(e) => {
                    yield Event::Error(e);
                    return;
                }
            },
        };

        let mut open_id = if cfg.rotation == Rotation::Name {
            src.identity().await.ok()
        } else {
            None
        };
        let mut last_byte: Option<u8> = None;
        let mut buf = vec![0u8; cfg.read_size.get()];

        loop {
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

            if !cfg.follow {
                return;
            }
            waiter.reset();

            let reset = loop {
                match src.size().await {
                    Ok(size) if size < offset => break true,
                    Ok(size) if size > offset => {
                        break anchor_mismatch(&mut src, offset, last_byte).await;
                    }
                    _ => {}
                }

                if cfg.rotation == Rotation::Name {
                    let path_id = src.path_identity().await.ok().flatten();
                    if path_id.is_some() && path_id != open_id {
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
                        }
                        open_id = src.identity().await.ok();
                        break true;
                    }
                }

                waiter.wait().await;
            };

            if reset {
                yield Event::Reset;
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

#[derive(Debug, Clone, TypedBuilder)]
pub struct Tail {
    #[builder(setter(into))]
    path: PathBuf,
    #[builder(default = Start::Beginning)]
    start: Start,
    #[builder(default = true)]
    follow: bool,
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
            start: self.start,
            follow: self.follow,
            rotation: self.rotation,
            read_size: self.read_size,
            backoff: self.backoff,
        }
    }

    fn events(self) -> impl Stream<Item = Event> + Send {
        let cfg = self.engine_cfg();
        let path = self.path;
        async_stream::stream! {
            let src = match FileSource::open(path).await {
                Ok(src) => src,
                Err(e) => {
                    yield Event::Error(e);
                    return;
                }
            };
            let inner = follow(src, BackoffWaiter::new(cfg.backoff), cfg);
            futures::pin_mut!(inner);
            while let Some(event) = inner.next().await {
                yield event;
            }
        }
    }

    pub fn chunks(self) -> impl Stream<Item = io::Result<Vec<u8>>> + Send {
        let events = self.events();
        async_stream::stream! {
            futures::pin_mut!(events);
            while let Some(event) = events.next().await {
                match event {
                    Event::Data { bytes, .. } => yield Ok(bytes),
                    Event::Error(e) => yield Err(e),
                    Event::Reset => {}
                }
            }
        }
    }

    pub fn lines(self) -> impl Stream<Item = io::Result<String>> + Send {
        let max = self.max_line_len;
        let events = self.events();
        async_stream::stream! {
            let mut acc = LineAccumulator::new(max);
            futures::pin_mut!(events);
            while let Some(event) = events.next().await {
                match event {
                    Event::Data { bytes, .. } => {
                        acc.push(&bytes);
                        while let Some(line) = acc.next_line() {
                            yield decode_line(line);
                        }
                    }
                    Event::Reset => acc.reset(),
                    Event::Error(e) => yield Err(e),
                }
            }
            if let Some(line) = acc.finish() {
                yield decode_line(line);
            }
        }
    }

    pub fn lines_positioned(self) -> impl Stream<Item = (u64, io::Result<String>)> + Send {
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
                            let at = epoch_start.unwrap_or(0) + acc.consumed();
                            yield (at, decode_line(line));
                        }
                    }
                    Event::Reset => {
                        acc.reset();
                        epoch_start = None;
                    }
                    Event::Error(e) => {
                        let at = epoch_start.unwrap_or(0) + acc.consumed();
                        yield (at, Err(e));
                    }
                }
            }
            if let Some(line) = acc.finish() {
                let at = epoch_start.unwrap_or(0) + acc.consumed();
                yield (at, decode_line(line));
            }
        }
    }

    #[must_use]
    pub fn bytes(self) -> ChunkedStream<io::Result<u8>> {
        ChunkedStream::new(self.chunks().map(|item| match item {
            Ok(buf) => buf.into_iter().map(Ok).collect(),
            Err(e) => vec![Err(e)],
        }))
    }
}

fn decode_line(line: Result<Vec<u8>, LineTooLong>) -> io::Result<String> {
    match line {
        Ok(bytes) => {
            String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
        Err(LineTooLong { len }) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("line exceeds maximum length ({len} bytes)"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};

    use futures::channel::mpsc;
    use futures::task::noop_waker;
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
    #[case(vec![b"hel".as_slice(), b"lo\n".as_slice()], vec![b"hello".to_vec()])]
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
        let lines = collect_lines(Tail::builder().path(&path).follow(false).build().lines()).await;
        assert_eq!(lines, vec!["one", "two", "three"]);
    }

    #[rstest]
    #[tokio::test]
    async fn emits_the_unterminated_trailing_line_when_not_following() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"a\nb\nno-newline").unwrap();
        let lines = collect_lines(Tail::builder().path(&path).follow(false).build().lines()).await;
        assert_eq!(lines, vec!["a", "b", "no-newline"]);
    }

    #[rstest]
    #[tokio::test]
    async fn start_end_skips_existing_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"old\n").unwrap();
        let lines = collect_lines(
            Tail::builder().path(&path).follow(false).start(Start::End).build().lines(),
        )
        .await;
        assert_eq!(lines, Vec::<String>::new());
    }

    #[rstest]
    #[tokio::test]
    async fn a_missing_file_yields_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope");
        let stream = Tail::builder().path(&path).follow(false).build().lines();
        futures::pin_mut!(stream);
        assert!(stream.next().await.expect("one item").is_err());
    }

    #[rstest]
    #[tokio::test]
    async fn bytes_streams_raw_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"abc").unwrap();
        let got: Vec<u8> =
            Tail::builder().path(&path).follow(false).build().bytes().try_collect().await.unwrap();
        assert_eq!(got, b"abc");
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
            let mut world = self.world.lock().unwrap();
            let id = world.path_id;
            world.files.get_mut(&id).expect("path file exists").extend_from_slice(data);
        }

        fn truncate(&self, len: usize) {
            let mut world = self.world.lock().unwrap();
            let id = world.path_id;
            world.files.get_mut(&id).expect("path file exists").truncate(len);
        }

        fn rotate(&self) {
            let mut world = self.world.lock().unwrap();
            let id = FdIdentity::from_raw(0, world.next);
            world.next += 1;
            world.files.insert(id, Vec::new());
            world.path_id = id;
        }
    }

    impl TailSource for MemSource {
        async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            let world = self.world.lock().unwrap();
            let data = world.files.get(&world.open_id).expect("open file exists");
            let start = usize::try_from(offset).expect("offset fits usize");
            if start >= data.len() {
                return Ok(0);
            }
            let n = buf.len().min(data.len() - start);
            buf[..n].copy_from_slice(&data[start..start + n]);
            Ok(n)
        }

        async fn size(&mut self) -> io::Result<u64> {
            let world = self.world.lock().unwrap();
            let len = world.files.get(&world.open_id).expect("open file exists").len();
            Ok(u64::try_from(len).expect("len fits u64"))
        }

        async fn identity(&mut self) -> io::Result<FdIdentity> {
            Ok(self.world.lock().unwrap().open_id)
        }

        async fn path_identity(&mut self) -> io::Result<Option<FdIdentity>> {
            Ok(Some(self.world.lock().unwrap().path_id))
        }

        async fn reopen(&mut self) -> io::Result<()> {
            let mut world = self.world.lock().unwrap();
            world.open_id = world.path_id;
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
            start: Start::Beginning,
            follow: true,
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
        loop {
            match stream.as_mut().poll_next(&mut cx) {
                Poll::Ready(Some(event)) => out.push(event),
                Poll::Ready(None) | Poll::Pending => break,
            }
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
        assert_eq!(resets(&events), 1);
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
        let stream = Tail::builder().path(&path).follow(false).build().lines_positioned();
        futures::pin_mut!(stream);
        let mut got = Vec::new();
        while let Some((at, line)) = stream.next().await {
            got.push((at, line.unwrap()));
        }
        assert_eq!(got, vec![(3, "aa".to_string()), (7, "bbb".to_string()), (9, "c".to_string())]);
    }

    #[rstest]
    #[tokio::test]
    async fn start_offset_resumes_from_a_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"aa\nbbb\nc\n").unwrap();
        let lines = collect_lines(
            Tail::builder().path(&path).follow(false).start(Start::Offset(7)).build().lines(),
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
        let stream = Tail::builder().path(&path).build().lines();
        futures::pin_mut!(stream);
        assert_eq!(stream.next().await.unwrap().unwrap(), "one");
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"two\n").unwrap();
        file.flush().unwrap();
        drop(file);
        assert_eq!(stream.next().await.unwrap().unwrap(), "two");
    }
}
