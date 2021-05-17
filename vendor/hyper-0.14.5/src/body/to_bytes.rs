use bytes::{Buf, BufMut, Bytes};

use super::HttpBody;

/// Concatenate the buffers from a body into a single `Bytes` asynchronously.
///
/// This may require copying the data into a single buffer. If you don't need
/// a contiguous buffer, prefer the [`aggregate`](crate::body::aggregate())
/// function.
///
/// # Note
///
/// Care needs to be taken if the remote is untrusted. The function doesn't implement any length
/// checks and an malicious peer might make it consume arbitrary amounts of memory. Checking the
/// `Content-Length` is a possibility, but it is not strictly mandated to be present.
pub async fn to_bytes<T>(body: T) -> Result<Bytes, T::Error>
where
    T: HttpBody,
{
    futures_util::pin_mut!(body);

    // If there's only 1 chunk, we can just return Buf::to_bytes()
    let mut first = if let Some(buf) = body.data().await {
        buf?
    } else {
        return Ok(Bytes::new());
    };

    let second = if let Some(buf) = body.data().await {
        buf?
    } else {
        return Ok(first.copy_to_bytes(first.remaining()));
    };

    // With more than 1 buf, we gotta flatten into a Vec first.
    let cap = first.remaining() + second.remaining() + body.size_hint().lower() as usize;
    let mut vec = Vec::with_capacity(cap);
    vec.put(first);
    vec.put(second);

    while let Some(buf) = body.data().await {
        vec.put(buf?);
    }

    Ok(vec.into())
}
