use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_MSG_LEN: u32 = 128 * 1024 * 1024;
pub const LEN_PREFIX_BYTES: usize = 4;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("message too large: {0} bytes")]
    TooLarge(usize),

    #[error("failed to encode message: {0}")]
    Encode(postcard::Error),

    #[error("failed to decode message: {0}")]
    Decode(postcard::Error),
}

pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, FrameError> {
    let body = postcard::to_stdvec(value).map_err(FrameError::Encode)?;

    let len = u32::try_from(body.len())
        .ok()
        .filter(|len| *len <= MAX_MSG_LEN)
        .ok_or(FrameError::TooLarge(body.len()))?;

    let mut framed = Vec::with_capacity(LEN_PREFIX_BYTES + body.len());
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

pub fn parse_len(prefix: [u8; LEN_PREFIX_BYTES]) -> Result<usize, FrameError> {
    let len = u32::from_be_bytes(prefix);
    if len > MAX_MSG_LEN {
        return Err(FrameError::TooLarge(len as usize));
    }
    Ok(len as usize)
}

pub fn decode_body<'a, T: Deserialize<'a>>(buf: &'a [u8]) -> Result<T, FrameError> {
    postcard::from_bytes(buf).map_err(FrameError::Decode)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Sample {
        a: u32,
        b: String,
        c: Vec<u8>,
    }

    #[rstest]
    fn frame_round_trips() {
        let value = Sample {
            a: 42,
            b: "hello".to_string(),
            c: vec![1, 2, 3],
        };

        let framed = encode_frame(&value).expect("encode");
        let prefix: [u8; LEN_PREFIX_BYTES] = framed[..LEN_PREFIX_BYTES].try_into().unwrap();
        let len = parse_len(prefix).expect("parse len");
        let body = &framed[LEN_PREFIX_BYTES..];

        assert_eq!(len, body.len());
        let decoded: Sample = decode_body(body).expect("decode");
        assert_eq!(decoded, value);
    }

    #[rstest]
    fn encoded_length_prefix_matches_body() {
        let value = Sample {
            a: 0,
            b: String::new(),
            c: Vec::new(),
        };
        let framed = encode_frame(&value).expect("encode");
        let prefix: [u8; LEN_PREFIX_BYTES] = framed[..LEN_PREFIX_BYTES].try_into().unwrap();
        assert_eq!(parse_len(prefix).unwrap(), framed.len() - LEN_PREFIX_BYTES);
    }

    #[rstest]
    fn parse_len_rejects_oversized() {
        let prefix = (MAX_MSG_LEN + 1).to_be_bytes();
        assert!(matches!(parse_len(prefix), Err(FrameError::TooLarge(_))));
    }

    #[rstest]
    fn parse_len_accepts_max() {
        assert_eq!(parse_len(MAX_MSG_LEN.to_be_bytes()).unwrap(), MAX_MSG_LEN as usize);
    }

    #[rstest]
    fn decode_body_rejects_garbage() {
        let garbage = [0xffu8; 8];
        assert!(matches!(decode_body::<Sample>(&garbage), Err(FrameError::Decode(_))));
    }
}
