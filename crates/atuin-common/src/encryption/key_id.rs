use std::str::FromStr;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use blake2::Blake2b;
use blake2::digest::Digest as _;
use blake2::digest::consts::U33;
use zeroize::Zeroizing;

/// A key's PASERK `k4.lid`: `k4.lid.` and 44 base64url characters; public, not secret.
#[derive(Clone, Copy, PartialEq, Eq, derive_more::Display, derive_more::Debug)]
#[display("{}{}", Self::PREFIX, URL_SAFE_NO_PAD.encode(_0))]
#[debug("{self}")]
pub struct KeyId([u8; Self::LEN]);

impl KeyId {
    pub(super) const LEN: usize = 33;
    const PREFIX: &'static str = "k4.lid.";

    /// Unkeyed BLAKE2b-264 of `k4.lid.` || `k4.local.` || base64url(key), as PASERK ID.md defines it.
    pub(super) fn of(key: &[u8; 32]) -> Self {
        let mut encoded = Zeroizing::new([0; 43]);
        URL_SAFE_NO_PAD
            .encode_slice(key, encoded.as_mut_slice())
            .expect("32 bytes are 43 unpadded base64 characters");
        Self(
            Blake2b::<U33>::new()
                .chain_update(Self::PREFIX)
                .chain_update("k4.local.")
                .chain_update(encoded.as_slice())
                .finalize()
                .into(),
        )
    }

    pub(super) const fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }

    pub(super) const fn from_bytes(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }
}

impl FromStr for KeyId {
    type Err = KeyIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let encoded =
            s.strip_prefix(Self::PREFIX).ok_or_else(|| KeyIdParseError::Prefix(s.to_owned()))?;
        let bytes = URL_SAFE_NO_PAD.decode(encoded)?;
        let len = bytes.len();
        bytes.try_into().map(Self).map_err(|_| KeyIdParseError::Length(len))
    }
}

/// Why a string is not a `k4.lid`.
#[derive(Debug, thiserror::Error)]
pub enum KeyIdParseError {
    #[error("`{0}` does not start with `k4.lid.`")]
    Prefix(String),
    #[error("the key id is not canonical unpadded base64url: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("a key id is 33 bytes, not {0}")]
    Length(usize),
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::zeroes(
        "0000000000000000000000000000000000000000000000000000000000000000",
        "k4.lid.bqltbNc4JLUAmc9Xtpok-fBuI0dQN5_m3CD9W_nbh559"
    )]
    #[case::ascending(
        "707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f",
        "k4.lid.iVtYQDjr5gEijCSjJC3fQaJm7nCeQSeaty0Jixy8dbsk"
    )]
    #[case::last_byte_differs(
        "707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e90",
        "k4.lid.-v0wjDR1FVxNT2to41Ay1P4_8X6HIxnybX1nZ1a4FCTm"
    )]
    #[case::golden_record(
        "5555555555555555555555555555555555555555555555555555555555555555",
        "k4.lid.2LzCmxDtbwu2tK5T1X1VLEth8umaI9vbKgTDkkt7ARR0"
    )]
    fn matches_the_paserk_vectors(#[case] key: &str, #[case] expected: &str) {
        let key: [u8; 32] = hex::decode(key).unwrap().try_into().unwrap();
        let id = KeyId::of(&key);

        assert_eq!(id.to_string(), expected);
        assert_eq!(expected.parse::<KeyId>().unwrap(), id);
    }

    #[rstest]
    fn agrees_with_rusty_paserk() {
        proptest!(|(key in any::<[u8; 32]>())| {
            let rusty = rusty_paserk::Key::<rusty_paserk::V4, rusty_paserk::Local>::from_bytes(key);
            prop_assert_eq!(KeyId::of(&key).to_string(), rusty.to_id().to_string());
        });
    }

    #[rstest]
    fn display_round_trips_through_from_str() {
        proptest!(|(bytes in any::<[u8; 33]>())| {
            let id = KeyId::from_bytes(bytes);
            prop_assert_eq!(id.to_string().parse::<KeyId>().unwrap(), id);
        });
    }

    #[rstest]
    #[case::no_prefix("2LzCmxDtbwu2tK5T1X1VLEth8umaI9vbKgTDkkt7ARR0")]
    #[case::other_type("k4.pid.2LzCmxDtbwu2tK5T1X1VLEth8umaI9vbKgTDkkt7ARR0")]
    #[case::chars_43("k4.lid.2LzCmxDtbwu2tK5T1X1VLEth8umaI9vbKgTDkkt7ARR")]
    #[case::chars_45("k4.lid.2LzCmxDtbwu2tK5T1X1VLEth8umaI9vbKgTDkkt7ARR0A")]
    #[case::padded("k4.lid.2LzCmxDtbwu2tK5T1X1VLEth8umaI9vbKgTDkkt7ARR0=")]
    #[case::standard_alphabet("k4.lid.2LzCmxDtbwu2tK5T1X1VLEth8umaI9vbKgTDkkt7ARR+")]
    #[case::empty("")]
    fn rejects_what_is_not_a_k4_lid(#[case] s: &str) {
        assert!(s.parse::<KeyId>().is_err(), "{s}");
    }
}
