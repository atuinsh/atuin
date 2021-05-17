use bytes::Buf;
use h2::SendStream;
use http::header::{
    HeaderName, CONNECTION, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER,
    TRANSFER_ENCODING, UPGRADE,
};
use http::HeaderMap;
use pin_project::pin_project;
use std::error::Error as StdError;
use std::io::IoSlice;

use crate::body::{DecodedLength, HttpBody};
use crate::common::{task, Future, Pin, Poll};
use crate::headers::content_length_parse_all;

pub(crate) mod ping;

cfg_client! {
    pub(crate) mod client;
    pub(crate) use self::client::ClientTask;
}

cfg_server! {
    pub(crate) mod server;
    pub(crate) use self::server::Server;
}

/// Default initial stream window size defined in HTTP2 spec.
pub(crate) const SPEC_WINDOW_SIZE: u32 = 65_535;

fn strip_connection_headers(headers: &mut HeaderMap, is_request: bool) {
    // List of connection headers from:
    // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Connection
    //
    // TE headers are allowed in HTTP/2 requests as long as the value is "trailers", so they're
    // tested separately.
    let connection_headers = [
        HeaderName::from_lowercase(b"keep-alive").unwrap(),
        HeaderName::from_lowercase(b"proxy-connection").unwrap(),
        PROXY_AUTHENTICATE,
        PROXY_AUTHORIZATION,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
    ];

    for header in connection_headers.iter() {
        if headers.remove(header).is_some() {
            warn!("Connection header illegal in HTTP/2: {}", header.as_str());
        }
    }

    if is_request {
        if headers
            .get(TE)
            .map(|te_header| te_header != "trailers")
            .unwrap_or(false)
        {
            warn!("TE headers not set to \"trailers\" are illegal in HTTP/2 requests");
            headers.remove(TE);
        }
    } else if headers.remove(TE).is_some() {
        warn!("TE headers illegal in HTTP/2 responses");
    }

    if let Some(header) = headers.remove(CONNECTION) {
        warn!(
            "Connection header illegal in HTTP/2: {}",
            CONNECTION.as_str()
        );
        let header_contents = header.to_str().unwrap();

        // A `Connection` header may have a comma-separated list of names of other headers that
        // are meant for only this specific connection.
        //
        // Iterate these names and remove them as headers. Connection-specific headers are
        // forbidden in HTTP2, as that information has been moved into frame types of the h2
        // protocol.
        for name in header_contents.split(',') {
            let name = name.trim();
            headers.remove(name);
        }
    }
}

fn decode_content_length(headers: &HeaderMap) -> DecodedLength {
    if let Some(len) = content_length_parse_all(headers) {
        // If the length is u64::MAX, oh well, just reported chunked.
        DecodedLength::checked_new(len).unwrap_or_else(|_| DecodedLength::CHUNKED)
    } else {
        DecodedLength::CHUNKED
    }
}

// body adapters used by both Client and Server

#[pin_project]
struct PipeToSendStream<S>
where
    S: HttpBody,
{
    body_tx: SendStream<SendBuf<S::Data>>,
    data_done: bool,
    #[pin]
    stream: S,
}

impl<S> PipeToSendStream<S>
where
    S: HttpBody,
{
    fn new(stream: S, tx: SendStream<SendBuf<S::Data>>) -> PipeToSendStream<S> {
        PipeToSendStream {
            body_tx: tx,
            data_done: false,
            stream,
        }
    }
}

impl<S> Future for PipeToSendStream<S>
where
    S: HttpBody,
    S::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    type Output = crate::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let mut me = self.project();
        loop {
            if !*me.data_done {
                // we don't have the next chunk of data yet, so just reserve 1 byte to make
                // sure there's some capacity available. h2 will handle the capacity management
                // for the actual body chunk.
                me.body_tx.reserve_capacity(1);

                if me.body_tx.capacity() == 0 {
                    loop {
                        match ready!(me.body_tx.poll_capacity(cx)) {
                            Some(Ok(0)) => {}
                            Some(Ok(_)) => break,
                            Some(Err(e)) => {
                                return Poll::Ready(Err(crate::Error::new_body_write(e)))
                            }
                            None => {
                                // None means the stream is no longer in a
                                // streaming state, we either finished it
                                // somehow, or the remote reset us.
                                return Poll::Ready(Err(crate::Error::new_body_write(
                                    "send stream capacity unexpectedly closed",
                                )));
                            }
                        }
                    }
                } else if let Poll::Ready(reason) = me
                    .body_tx
                    .poll_reset(cx)
                    .map_err(crate::Error::new_body_write)?
                {
                    debug!("stream received RST_STREAM: {:?}", reason);
                    return Poll::Ready(Err(crate::Error::new_body_write(::h2::Error::from(
                        reason,
                    ))));
                }

                match ready!(me.stream.as_mut().poll_data(cx)) {
                    Some(Ok(chunk)) => {
                        let is_eos = me.stream.is_end_stream();
                        trace!(
                            "send body chunk: {} bytes, eos={}",
                            chunk.remaining(),
                            is_eos,
                        );

                        let buf = SendBuf(Some(chunk));
                        me.body_tx
                            .send_data(buf, is_eos)
                            .map_err(crate::Error::new_body_write)?;

                        if is_eos {
                            return Poll::Ready(Ok(()));
                        }
                    }
                    Some(Err(e)) => return Poll::Ready(Err(me.body_tx.on_user_err(e))),
                    None => {
                        me.body_tx.reserve_capacity(0);
                        let is_eos = me.stream.is_end_stream();
                        if is_eos {
                            return Poll::Ready(me.body_tx.send_eos_frame());
                        } else {
                            *me.data_done = true;
                            // loop again to poll_trailers
                        }
                    }
                }
            } else {
                if let Poll::Ready(reason) = me
                    .body_tx
                    .poll_reset(cx)
                    .map_err(crate::Error::new_body_write)?
                {
                    debug!("stream received RST_STREAM: {:?}", reason);
                    return Poll::Ready(Err(crate::Error::new_body_write(::h2::Error::from(
                        reason,
                    ))));
                }

                match ready!(me.stream.poll_trailers(cx)) {
                    Ok(Some(trailers)) => {
                        me.body_tx
                            .send_trailers(trailers)
                            .map_err(crate::Error::new_body_write)?;
                        return Poll::Ready(Ok(()));
                    }
                    Ok(None) => {
                        // There were no trailers, so send an empty DATA frame...
                        return Poll::Ready(me.body_tx.send_eos_frame());
                    }
                    Err(e) => return Poll::Ready(Err(me.body_tx.on_user_err(e))),
                }
            }
        }
    }
}

trait SendStreamExt {
    fn on_user_err<E>(&mut self, err: E) -> crate::Error
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>;
    fn send_eos_frame(&mut self) -> crate::Result<()>;
}

impl<B: Buf> SendStreamExt for SendStream<SendBuf<B>> {
    fn on_user_err<E>(&mut self, err: E) -> crate::Error
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let err = crate::Error::new_user_body(err);
        debug!("send body user stream error: {}", err);
        self.send_reset(err.h2_reason());
        err
    }

    fn send_eos_frame(&mut self) -> crate::Result<()> {
        trace!("send body eos");
        self.send_data(SendBuf(None), true)
            .map_err(crate::Error::new_body_write)
    }
}

struct SendBuf<B>(Option<B>);

impl<B: Buf> Buf for SendBuf<B> {
    #[inline]
    fn remaining(&self) -> usize {
        self.0.as_ref().map(|b| b.remaining()).unwrap_or(0)
    }

    #[inline]
    fn chunk(&self) -> &[u8] {
        self.0.as_ref().map(|b| b.chunk()).unwrap_or(&[])
    }

    #[inline]
    fn advance(&mut self, cnt: usize) {
        if let Some(b) = self.0.as_mut() {
            b.advance(cnt)
        }
    }

    fn chunks_vectored<'a>(&'a self, dst: &mut [IoSlice<'a>]) -> usize {
        self.0.as_ref().map(|b| b.chunks_vectored(dst)).unwrap_or(0)
    }
}
