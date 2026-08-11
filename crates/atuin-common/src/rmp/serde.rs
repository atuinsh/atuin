//! Serialize iterators of [`Serialize`] values straight into a MessagePack array.
//!
//! Unlike [`rmp_serde::to_vec`], these helpers do not require the whole collection
//! to be materialized as a `Vec<T>` (or otherwise be [`Serialize`]) first -- an
//! [`ExactSizeIterator`] is enough, so the caller can stream elements out of an
//! iterator adapter without an intermediate allocation.

use serde::ser::{Serialize, SerializeSeq, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum TryToVecError<E: std::error::Error> {
    #[error(transparent)]
    Encoding(#[from] rmp_serde::encode::Error),
    #[error(transparent)]
    Given(E),
    #[error("iterator yielded a different number of items than its reported length")]
    BadLength,
}

/// The same as [`to_vec`] except its items are [`Result`]s.
///
/// Eagerly returns the error, if one is found in the iterator.
pub fn try_to_vec<T, E, It>(it: It) -> Result<Vec<u8>, TryToVecError<E>>
where
    T: Serialize,
    E: std::error::Error,
    It: IntoIterator<IntoIter: ExactSizeIterator<Item = Result<T, E>>>,
{
    let mut buf = Vec::new();
    let mut ser = rmp_serde::Serializer::new(&mut buf);

    // The array header is written up front from `len()`, so a misbehaving
    // `ExactSizeIterator` whose `len()` disagrees with what it yields would emit a
    // corrupt array. Count the elements actually serialized and reject a mismatch.
    let it = it.into_iter();
    let expected = it.len();
    let mut seq = ser.serialize_seq(Some(expected))?;
    let mut count = 0usize;
    for r in it {
        let elem = r.map_err(TryToVecError::Given)?;
        seq.serialize_element(&elem)?;
        count += 1;
    }
    seq.end()?;

    if count != expected {
        return Err(TryToVecError::BadLength);
    }

    Ok(buf)
}

#[derive(Debug, thiserror::Error)]
pub enum ToVecError {
    #[error(transparent)]
    Encoding(#[from] rmp_serde::encode::Error),
    #[error("iterator yielded a different number of items than its reported length")]
    BadLength,
}

impl From<TryToVecError<std::convert::Infallible>> for ToVecError {
    fn from(value: TryToVecError<std::convert::Infallible>) -> Self {
        match value {
            TryToVecError::Encoding(e) => ToVecError::Encoding(e),
            TryToVecError::BadLength => ToVecError::BadLength,
            TryToVecError::Given(_) => unreachable!(),
        }
    }
}

/// Given anything convertible into an [`ExactSizeIterator`], serialize it into an
/// `rmp`-encoded vector of bytes.
///
/// Note that unlike [`rmp_serde::to_vec`] which requires that the argument passed is
/// [`serde::Serialize`], this makes no such requirement, only requiring an [`ExactSizeIterator`]
/// of [`serde::Serialize`] types.
pub fn to_vec<T, It>(it: It) -> Result<Vec<u8>, ToVecError>
where
    T: Serialize + Sized,
    It: IntoIterator<IntoIter: ExactSizeIterator<Item = T>>,
{
    try_to_vec(it.into_iter().map(Ok::<T, std::convert::Infallible>)).map_err(ToVecError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rstest::{fixture, rstest};
    use serde::{Deserialize, Serialize};
    use std::convert::Infallible;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Sample {
        id: i64,
        name: String,
        flag: bool,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("boom")]
    struct Boom;

    /// An [`ExactSizeIterator`] whose reported length deliberately disagrees with the
    /// number of items it actually yields, used to exercise the `BadLength` guard.
    struct LyingLen<I> {
        inner: I,
        claimed: usize,
    }

    impl<I: Iterator> Iterator for LyingLen<I> {
        type Item = I::Item;

        fn next(&mut self) -> Option<Self::Item> {
            self.inner.next()
        }

        fn size_hint(&self) -> (usize, Option<usize>) {
            (self.claimed, Some(self.claimed))
        }
    }

    impl<I: Iterator> ExactSizeIterator for LyingLen<I> {
        fn len(&self) -> usize {
            self.claimed
        }
    }

    #[fixture]
    fn samples() -> Vec<Sample> {
        vec![
            Sample {
                id: 1,
                name: "alpha".into(),
                flag: true,
            },
            Sample {
                id: -2,
                name: "beta".into(),
                flag: false,
            },
            Sample {
                id: 0,
                name: String::new(),
                flag: true,
            },
        ]
    }

    #[rstest]
    #[case(vec![])]
    #[case(vec![0])]
    #[case(vec![1, 2, 3])]
    #[case(vec![u64::MIN, u64::MAX, 12_345])]
    fn to_vec_equals_standard_path_and_round_trips(#[case] items: Vec<u64>) {
        let streamed = to_vec(items.iter().copied()).expect("to_vec should succeed");
        let standard = rmp_serde::to_vec(&items).expect("rmp_serde::to_vec should succeed");
        assert_eq!(
            streamed, standard,
            "streamed bytes must equal the standard path"
        );

        let decoded: Vec<u64> = rmp_serde::from_slice(&streamed).expect("round-trip should decode");
        assert_eq!(decoded, items);
    }

    #[rstest]
    fn to_vec_handles_structs(samples: Vec<Sample>) {
        let streamed = to_vec(samples.iter().cloned()).expect("to_vec should succeed");
        let standard = rmp_serde::to_vec(&samples).expect("rmp_serde::to_vec should succeed");
        assert_eq!(streamed, standard);

        let decoded: Vec<Sample> =
            rmp_serde::from_slice(&streamed).expect("round-trip should decode");
        assert_eq!(decoded, samples);
    }

    #[rstest]
    fn to_vec_accepts_borrowed_collection() {
        let items = vec![10u64, 20, 30];
        // The `IntoIterator` bound lets callers pass a `&Vec` directly, without `.iter()`.
        let streamed = to_vec(&items).expect("to_vec should accept a &Vec");
        let standard = rmp_serde::to_vec(&items).expect("rmp_serde::to_vec should succeed");
        assert_eq!(streamed, standard);
    }

    #[rstest]
    #[case(vec![])]
    #[case(vec![7])]
    #[case(vec![1, 2, 3, 4])]
    fn try_to_vec_all_ok_matches_to_vec(#[case] items: Vec<u64>) {
        let via_try = try_to_vec(items.iter().copied().map(Ok::<u64, Infallible>))
            .expect("try_to_vec should succeed");
        let via_plain = to_vec(items.iter().copied()).expect("to_vec should succeed");
        assert_eq!(via_try, via_plain);
    }

    #[rstest]
    fn try_to_vec_returns_given_on_first_err() {
        let items: Vec<Result<u64, Boom>> = vec![Ok(1), Ok(2), Err(Boom), Ok(4)];
        let err = try_to_vec(items.into_iter()).expect_err("the given error should surface");
        assert!(matches!(err, TryToVecError::Given(Boom)));
    }

    #[rstest]
    #[case(5, vec![1, 2, 3])] // reports more than it yields
    #[case(2, vec![1, 2, 3, 4])] // reports fewer than it yields
    fn try_to_vec_rejects_length_mismatch(#[case] claimed: usize, #[case] items: Vec<u64>) {
        let it = LyingLen {
            inner: items.into_iter().map(Ok::<u64, Infallible>),
            claimed,
        };
        let err = try_to_vec(it).expect_err("a length mismatch must be rejected");
        assert!(matches!(err, TryToVecError::BadLength));
    }

    #[rstest]
    #[case(5, vec![1, 2, 3])]
    #[case(2, vec![1, 2, 3, 4])]
    fn to_vec_rejects_length_mismatch(#[case] claimed: usize, #[case] items: Vec<u64>) {
        let it = LyingLen {
            inner: items.into_iter(),
            claimed,
        };
        let err = to_vec(it).expect_err("a length mismatch must be rejected");
        assert!(matches!(err, ToVecError::BadLength));
    }

    #[rstest]
    fn to_vec_error_maps_bad_length() {
        let converted: ToVecError = TryToVecError::<Infallible>::BadLength.into();
        assert!(matches!(converted, ToVecError::BadLength));
    }

    proptest! {
        #[test]
        fn to_vec_matches_standard_path_u64(items in prop::collection::vec(any::<u64>(), 0..64)) {
            let streamed = to_vec(items.iter().copied()).unwrap();
            let standard = rmp_serde::to_vec(&items).unwrap();
            prop_assert_eq!(streamed, standard);
        }

        #[test]
        fn to_vec_round_trips_u64(items in prop::collection::vec(any::<u64>(), 0..64)) {
            let streamed = to_vec(items.iter().copied()).unwrap();
            let decoded: Vec<u64> = rmp_serde::from_slice(&streamed).unwrap();
            prop_assert_eq!(decoded, items);
        }

        #[test]
        fn to_vec_matches_standard_path_string(
            items in prop::collection::vec(any::<String>(), 0..32),
        ) {
            let streamed = to_vec(items.iter().cloned()).unwrap();
            let standard = rmp_serde::to_vec(&items).unwrap();
            prop_assert_eq!(streamed, standard);
        }

        #[test]
        fn to_vec_round_trips_structs(
            raw in prop::collection::vec((any::<i64>(), any::<String>(), any::<bool>()), 0..32),
        ) {
            let items: Vec<Sample> = raw
                .into_iter()
                .map(|(id, name, flag)| Sample { id, name, flag })
                .collect();
            let streamed = to_vec(items.iter().cloned()).unwrap();
            let standard = rmp_serde::to_vec(&items).unwrap();
            prop_assert_eq!(&streamed, &standard);

            let decoded: Vec<Sample> = rmp_serde::from_slice(&streamed).unwrap();
            prop_assert_eq!(decoded, items);
        }

        #[test]
        fn try_to_vec_all_ok_matches_to_vec_prop(
            items in prop::collection::vec(any::<u64>(), 0..64),
        ) {
            let via_try = try_to_vec(items.iter().copied().map(Ok::<u64, Infallible>)).unwrap();
            let via_plain = to_vec(items.iter().copied()).unwrap();
            prop_assert_eq!(via_try, via_plain);
        }
    }
}
