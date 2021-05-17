#![allow(dead_code)]

use std::io;
use std::ops::{Deref, DerefMut};

use bytes::BytesMut;
use sqlx_rt::{AsyncRead, AsyncReadExt, AsyncWrite};

use crate::error::Error;
use crate::io::write_and_flush::WriteAndFlush;
use crate::io::{decode::Decode, encode::Encode};
use std::io::Cursor;

pub struct BufStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream: S,

    // writes with `write` to the underlying stream are buffered
    // this can be flushed with `flush`
    pub(crate) wbuf: Vec<u8>,

    // we read into the read buffer using 100% safe code
    rbuf: BytesMut,
}

impl<S> BufStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            wbuf: Vec::with_capacity(512),
            rbuf: BytesMut::with_capacity(4096),
        }
    }

    pub fn write<'en, T>(&mut self, value: T)
    where
        T: Encode<'en, ()>,
    {
        self.write_with(value, ())
    }

    pub fn write_with<'en, T, C>(&mut self, value: T, context: C)
    where
        T: Encode<'en, C>,
    {
        value.encode_with(&mut self.wbuf, context);
    }

    pub fn flush(&mut self) -> WriteAndFlush<'_, S> {
        WriteAndFlush {
            stream: &mut self.stream,
            buf: Cursor::new(&mut self.wbuf),
        }
    }

    pub async fn read<'de, T>(&mut self, cnt: usize) -> Result<T, Error>
    where
        T: Decode<'de, ()>,
    {
        self.read_with(cnt, ()).await
    }

    pub async fn read_with<'de, T, C>(&mut self, cnt: usize, context: C) -> Result<T, Error>
    where
        T: Decode<'de, C>,
    {
        T::decode_with(self.read_raw(cnt).await?.freeze(), context)
    }

    pub async fn read_raw(&mut self, cnt: usize) -> Result<BytesMut, Error> {
        read_raw_into(&mut self.stream, &mut self.rbuf, cnt).await?;
        let buf = self.rbuf.split_to(cnt);

        Ok(buf)
    }

    pub async fn read_raw_into(&mut self, buf: &mut BytesMut, cnt: usize) -> Result<(), Error> {
        read_raw_into(&mut self.stream, buf, cnt).await
    }
}

impl<S> Deref for BufStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    type Target = S;

    fn deref(&self) -> &Self::Target {
        &self.stream
    }
}

impl<S> DerefMut for BufStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.stream
    }
}

// Holds a buffer which has been temporarily extended, so that
// we can read into it. Automatically shrinks the buffer back
// down if the read is cancelled.
struct BufTruncator<'a> {
    buf: &'a mut BytesMut,
    filled_len: usize,
}

impl<'a> BufTruncator<'a> {
    fn new(buf: &'a mut BytesMut) -> Self {
        let filled_len = buf.len();
        Self { buf, filled_len }
    }
    fn reserve(&mut self, space: usize) {
        self.buf.resize(self.filled_len + space, 0);
    }
    async fn read<S: AsyncRead + Unpin>(&mut self, stream: &mut S) -> Result<usize, Error> {
        let n = stream.read(&mut self.buf[self.filled_len..]).await?;
        self.filled_len += n;
        Ok(n)
    }
    fn is_full(&self) -> bool {
        self.filled_len >= self.buf.len()
    }
}

impl Drop for BufTruncator<'_> {
    fn drop(&mut self) {
        self.buf.truncate(self.filled_len);
    }
}

async fn read_raw_into<S: AsyncRead + Unpin>(
    stream: &mut S,
    buf: &mut BytesMut,
    cnt: usize,
) -> Result<(), Error> {
    let mut buf = BufTruncator::new(buf);
    buf.reserve(cnt);

    while !buf.is_full() {
        let n = buf.read(stream).await?;

        if n == 0 {
            // a zero read when we had space in the read buffer
            // should be treated as an EOF

            // and an unexpected EOF means the server told us to go away

            return Err(io::Error::from(io::ErrorKind::ConnectionAborted).into());
        }
    }

    Ok(())
}
