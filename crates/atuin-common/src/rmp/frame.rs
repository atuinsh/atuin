//! Length-prefixed MessagePack frames on a byte stream.
//!
//! A frame is a `u32` length in big-endian byte order, followed by that many bytes of
//! MessagePack with named fields.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tracing::warn;

/// The longest frame body that [`read_frame`] accepts and that [`write_frame`] writes.
pub const MAX_FRAME_BYTES: u32 = 64 * 1024 * 1024;

/// The number of bytes in the length prefix of a frame.
const PREFIX_BYTES: usize = size_of::<u32>();

/// The most bytes that a frame buffer keeps between frames, and the most that a read reserves
/// before the body of a frame arrives.
const KEPT_BUFFER_BYTES: usize = 1 << 20;

/// What can go wrong while frames are written or read.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("the connection failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("a frame of {bytes} bytes is larger than the limit of {MAX_FRAME_BYTES} bytes")]
    TooLarge {
        bytes: u32,
    },
    #[error("the connection ended in the middle of a frame")]
    Truncated,
    #[error("a frame is not a valid message: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
    #[error("a message cannot be written as a frame: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
}

/// Writes one frame and flushes the writer.
///
/// # Errors
///
/// Fails when the frame does not serialize, when its body is longer than [`MAX_FRAME_BYTES`],
/// or when the writer fails.
pub async fn write_frame<W, F>(writer: &mut W, frame: &F) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    F: Serialize,
{
    write_frame_with(writer, frame, &mut Vec::new()).await
}

/// Writes one frame through `buffer`, which keeps its capacity for the next frame.
async fn write_frame_with<W, F>(
    writer: &mut W,
    frame: &F,
    buffer: &mut Vec<u8>,
) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    F: Serialize,
{
    buffer.clear();
    buffer.shrink_to(KEPT_BUFFER_BYTES);
    buffer.extend_from_slice(&[0; PREFIX_BYTES]);
    rmp_serde::encode::write_named(buffer, frame)?;
    let body = buffer.len() - PREFIX_BYTES;
    let length = u32::try_from(body).ok().filter(|&length| length <= MAX_FRAME_BYTES).ok_or(
        FrameError::TooLarge {
            bytes: u32::try_from(body).unwrap_or(u32::MAX),
        },
    )?;
    buffer[..PREFIX_BYTES].copy_from_slice(&length.to_be_bytes());
    writer.write_all(buffer).await?;
    writer.flush().await?;
    Ok(())
}

/// Writes frames to a writer, and reuses one buffer for them.
pub struct FrameWriter<W> {
    writer: W,
    buffer: Vec<u8>,
}

impl<W: AsyncWrite + Unpin> FrameWriter<W> {
    pub const fn new(writer: W) -> Self {
        Self {
            writer,
            buffer: Vec::new(),
        }
    }

    /// Writes one frame and flushes the writer.
    ///
    /// # Errors
    ///
    /// Fails as [`write_frame`] does.
    pub async fn send<F: Serialize>(&mut self, frame: &F) -> Result<(), FrameError> {
        write_frame_with(&mut self.writer, frame, &mut self.buffer).await
    }
}

/// Reads one frame.
///
/// Returns `Ok(None)` at a clean end of stream between frames.
///
/// # Errors
///
/// Fails when the length prefix exceeds [`MAX_FRAME_BYTES`], when the stream ends inside a
/// frame, when the body is not a valid message, or when the reader fails.
pub async fn read_frame<R, F>(reader: &mut R) -> Result<Option<F>, FrameError>
where
    R: AsyncRead + Unpin,
    F: DeserializeOwned,
{
    read_frame_with(reader, &mut Vec::new()).await
}

/// Reads one frame through `buffer`, which keeps its capacity for the next frame.
async fn read_frame_with<R, F>(
    reader: &mut R,
    buffer: &mut Vec<u8>,
) -> Result<Option<F>, FrameError>
where
    R: AsyncRead + Unpin,
    F: DeserializeOwned,
{
    let mut prefix = [0; PREFIX_BYTES];
    let mut filled = 0;
    while filled < PREFIX_BYTES {
        match reader.read(&mut prefix[filled..]).await? {
            0 if filled == 0 => return Ok(None),
            0 => return Err(FrameError::Truncated),
            read => filled += read,
        }
    }
    let length = u32::from_be_bytes(prefix);
    if length > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { bytes: length });
    }
    buffer.clear();
    buffer.shrink_to(KEPT_BUFFER_BYTES);
    // A prefix can promise more than the stream delivers, so the reservation is capped, and
    // the buffer grows with the bytes that arrive beyond it.
    let length = usize::try_from(length).expect("a frame length fits in usize");
    buffer.reserve(length.min(KEPT_BUFFER_BYTES));
    let read = reader.take(length as u64).read_to_end(buffer).await?;
    if read != length {
        return Err(FrameError::Truncated);
    }
    Ok(Some(rmp_serde::from_slice(buffer)?))
}

/// Reads frames from `reader` and sends them to `sender` until the stream ends, the stream
/// fails, or the receiver is dropped.
///
/// A frame whose body is not a valid message is skipped with a warning. The length prefix
/// says where the next frame starts, so the stream stays aligned after such a frame. Every
/// other error leaves the stream at an unknown position, so `forward_frames` sends that error
/// and returns.
///
/// A read of a frame cannot be cancelled without losing the part of the frame that it
/// consumed, so callers run this in a task of its own and wait on the channel.
pub async fn forward_frames<R, F>(mut reader: R, sender: mpsc::Sender<Result<F, FrameError>>)
where
    R: AsyncRead + Unpin,
    F: DeserializeOwned,
{
    let mut buffer = Vec::new();
    loop {
        let frame = match read_frame_with(&mut reader, &mut buffer).await {
            Ok(Some(frame)) => Ok(frame),
            Ok(None) => return,
            Err(FrameError::Decode(error)) => {
                warn!("skipping a frame that is not a valid message: {error}");
                continue;
            }
            Err(error) => Err(error),
        };
        let failed = frame.is_err();
        if sender.send(frame).await.is_err() || failed {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Hello {
        build: String,
    }

    fn hello() -> Hello {
        Hello {
            build: "build-1".to_owned(),
        }
    }

    #[rstest]
    #[tokio::test]
    async fn frames_round_trip_through_one_writer() {
        let mut bytes = Vec::new();
        let mut writer = FrameWriter::new(&mut bytes);
        writer.send(&hello()).await.unwrap();
        writer.send(&hello()).await.unwrap();
        let mut reader = bytes.as_slice();
        for _ in 0..2 {
            assert_eq!(read_frame::<_, Hello>(&mut reader).await.unwrap(), Some(hello()));
        }
        assert_eq!(read_frame::<_, Hello>(&mut reader).await.unwrap(), None);
    }

    #[rstest]
    #[tokio::test]
    async fn truncated_frame() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &hello()).await.unwrap();
        for cut in 1..bytes.len() {
            let result = read_frame::<_, Hello>(&mut &bytes[..cut]).await;
            assert!(matches!(result, Err(FrameError::Truncated)), "cut at {cut}: {result:?}");
        }
        assert!(read_frame::<_, Hello>(&mut &[][..]).await.unwrap().is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn oversized_frame() {
        let prefix = (MAX_FRAME_BYTES + 1).to_be_bytes();
        let result = read_frame::<_, Hello>(&mut &prefix[..]).await;
        assert!(
            matches!(result, Err(FrameError::TooLarge { bytes }) if bytes == MAX_FRAME_BYTES + 1),
            "{result:?}"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn invalid_body() {
        let bytes = [0, 0, 0, 1, 0xc1];
        let result = read_frame::<_, Hello>(&mut &bytes[..]).await;
        assert!(matches!(result, Err(FrameError::Decode(_))), "{result:?}");
    }

    #[rstest]
    #[tokio::test]
    async fn forwarding_skips_an_invalid_body() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &hello()).await.unwrap();
        // 0xc1 is a byte that MessagePack never uses.
        bytes.extend([0, 0, 0, 1, 0xc1]);
        write_frame(&mut bytes, &hello()).await.unwrap();
        let (sender, mut frames) = mpsc::channel::<Result<Hello, FrameError>>(4);

        forward_frames(bytes.as_slice(), sender).await;

        let mut forwarded = Vec::new();
        while let Some(frame) = frames.recv().await {
            forwarded.push(frame.unwrap());
        }
        assert_eq!(forwarded, [hello(), hello()]);
    }

    #[rstest]
    #[tokio::test]
    async fn forwarding_ends_with_a_truncated_frame() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &hello()).await.unwrap();
        bytes.extend([0, 0, 0, 100, 0x01]);
        write_frame(&mut bytes, &hello()).await.unwrap();
        let (sender, mut frames) = mpsc::channel::<Result<Hello, FrameError>>(4);

        forward_frames(bytes.as_slice(), sender).await;

        assert_eq!(frames.recv().await.unwrap().unwrap(), hello());
        let next = frames.recv().await;
        assert!(matches!(next, Some(Err(FrameError::Truncated))), "{next:?}");
        assert!(frames.recv().await.is_none());
    }
}
