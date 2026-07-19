pub mod decode;
pub mod encode;

#[cfg(feature = "eyre")]
mod eyre;

pub use decode::DecodeError;
pub use encode::EncodeError;

#[cfg(test)]
mod tests {
    use super::decode::DecodeExt;
    use super::{decode, encode};
    use decode::{Bytes, DecodeError};
    use proptest::prelude::*;
    use rstest::rstest;

    fn enc(f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut v = Vec::new();
        f(&mut v);
        v
    }

    proptest! {
        #[test]
        fn u8_loopback(x in any::<u8>()) {
            let buf = enc(|v| { encode::write_u8(v, x).unwrap(); });
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(rmp::decode::read_int::<u8, _>(&mut b).decode().unwrap(), x);
            prop_assert!(b.remaining_slice().is_empty());
        }

        #[test]
        fn u16_loopback(x in any::<u16>()) {
            let buf = enc(|v| { encode::write_u16(v, x).unwrap(); });
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(rmp::decode::read_int::<u16, _>(&mut b).decode().unwrap(), x);
            prop_assert!(b.remaining_slice().is_empty());
        }

        #[test]
        fn u64_loopback(x in any::<u64>()) {
            let buf = enc(|v| { encode::write_u64(v, x).unwrap(); });
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(rmp::decode::read_int::<u64, _>(&mut b).decode().unwrap(), x);
            prop_assert!(b.remaining_slice().is_empty());
        }

        #[test]
        fn i64_loopback(x in any::<i64>()) {
            let buf = enc(|v| { encode::write_sint(v, x).unwrap(); });
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(rmp::decode::read_int::<i64, _>(&mut b).decode().unwrap(), x);
            prop_assert!(b.remaining_slice().is_empty());
        }

        #[test]
        fn bool_loopback(x in any::<bool>()) {
            let buf = enc(|v| { encode::write_bool(v, x).unwrap(); });
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(rmp::decode::read_bool(&mut b).decode().unwrap(), x);
            prop_assert!(b.remaining_slice().is_empty());
        }

        #[test]
        fn string_loopback(x in "(?s).*") {
            let buf = enc(|v| { encode::write_str(v, &x).unwrap(); });
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(decode::read_string(&mut b).decode().unwrap(), x);
            prop_assert!(b.remaining_slice().is_empty());
        }

        #[test]
        fn optional_u64_loopback(x in proptest::option::of(any::<u64>())) {
            let buf = enc(|v| { encode::write_optional(v, x, encode::write_u64).unwrap(); });
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(decode::read_optional(&mut b, rmp::decode::read_int::<u64, _>).unwrap(), x);
            prop_assert!(b.remaining_slice().is_empty());
        }

        #[test]
        fn optional_string_loopback(x in proptest::option::of("(?s).*")) {
            let buf = enc(|v| { encode::write_optional(v, x.as_deref(), encode::write_str).unwrap(); });
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(decode::read_optional(&mut b, decode::read_string).unwrap(), x);
            prop_assert!(b.remaining_slice().is_empty());
        }

        #[test]
        fn array_of_string_loopback(xs in proptest::collection::vec("(?s).*", 0..8)) {
            let buf = enc(|v| {
                encode::write_array_len(v, xs.len() as u32).unwrap();
                for s in &xs {
                    encode::write_str(v, s).unwrap();
                }
            });
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(decode::read_array_of(&mut b, |b| decode::read_string(b).decode()).unwrap(), xs);
            prop_assert!(b.remaining_slice().is_empty());
        }

        #[test]
        fn total_array_record_loopback(name in "(?s).*", ts in any::<u64>(), flag in any::<bool>()) {
            let buf = enc(|v| {
                encode::write_array_len(v, 3).unwrap();
                encode::write_str(v, &name).unwrap();
                encode::write_u64(v, ts).unwrap();
                encode::write_bool(v, flag).unwrap();
            });
            let mut b = Bytes::new(&buf);
            let out = decode::read_total_array(&mut b, 3, |b| {
                Ok::<_, DecodeError>((
                    decode::read_string(b)?,
                    rmp::decode::read_int::<u64, _>(b)?,
                    rmp::decode::read_bool(b)?,
                ))
            })
            .unwrap();
            prop_assert_eq!(out, (name, ts, flag));
        }
    }

    #[rstest]
    #[case(3, 3, true)]
    #[case(3, 2, false)]
    #[case(1, 5, false)]
    fn expect_array_len_loopback(#[case] actual: u32, #[case] expected: u32, #[case] ok: bool) {
        let buf = enc(|v| {
            encode::write_array_len(v, actual).unwrap();
        });
        let mut b = Bytes::new(&buf);
        assert_eq!(decode::expect_array_len(&mut b, expected).is_ok(), ok);
    }

    #[rstest]
    #[case(&[][..], true)]
    #[case(&[0x01][..], false)]
    #[case(&[0x01, 0x02, 0x03][..], false)]
    fn expect_eof_loopback(#[case] raw: &[u8], #[case] ok: bool) {
        let b = Bytes::new(raw);
        assert_eq!(decode::expect_eof(&b).is_ok(), ok);
    }
}
