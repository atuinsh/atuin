//! The `time` 0.3 serde layout of an `OffsetDateTime`, for records that were written with it.
//!
//! **Be very, very careful changing this.** Synced records carry these bytes, and every client
//! that reads them, including older releases, expects exactly this shape.

use core::fmt;

use jiff::Timestamp;
use jiff::civil::{Date, DateTime, Time};
use jiff::tz::Offset;
use serde::de::{self, SeqAccess, Unexpected, Visitor};
use serde::ser::SerializeTuple;
use serde::{Deserialize, Deserializer, Serializer};
use serde_with::{DeserializeAs, SerializeAs};

/// `serde_with` marker that encodes a [`Timestamp`] as `time` 0.3 encodes an `OffsetDateTime`.
///
/// Binary formats get the 9-tuple `(year, day of year, hour, minute, second, nanosecond, offset
/// hours, offset minutes, offset seconds)`, written in UTC. Any offset reads back. Human-readable
/// formats get an RFC 3339 string. Use as `#[serde_as(as = "OffsetDateTimeTuple")]`.
pub struct OffsetDateTimeTuple;

const FIELDS: usize = 9;

impl SerializeAs<Timestamp> for OffsetDateTimeTuple {
    fn serialize_as<S: Serializer>(source: &Timestamp, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            return serializer.collect_str(source);
        }

        let utc = Offset::UTC.to_datetime(*source);
        let mut tuple = serializer.serialize_tuple(FIELDS)?;
        tuple.serialize_element(&i32::from(utc.year()))?;
        tuple.serialize_element(&utc.day_of_year().cast_unsigned())?;
        tuple.serialize_element(&utc.hour().cast_unsigned())?;
        tuple.serialize_element(&utc.minute().cast_unsigned())?;
        tuple.serialize_element(&utc.second().cast_unsigned())?;
        tuple.serialize_element(&utc.subsec_nanosecond().cast_unsigned())?;
        tuple.serialize_element(&0i8)?;
        tuple.serialize_element(&0i8)?;
        tuple.serialize_element(&0i8)?;
        tuple.end()
    }
}

impl<'de> DeserializeAs<'de, Timestamp> for OffsetDateTimeTuple {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<Timestamp, D::Error> {
        if deserializer.is_human_readable() {
            deserializer.deserialize_str(TupleVisitor)
        } else {
            deserializer.deserialize_tuple(FIELDS, TupleVisitor)
        }
    }
}

struct TupleVisitor;

fn element<'de, T: Deserialize<'de>, A: SeqAccess<'de>>(
    seq: &mut A,
    index: usize,
    expected: &dyn de::Expected,
) -> Result<T, A::Error> {
    seq.next_element()?.ok_or_else(|| de::Error::invalid_length(index, expected))
}

impl<'de> Visitor<'de> for TupleVisitor {
    type Value = Timestamp;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an `OffsetDateTime` tuple or an RFC 3339 timestamp")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Timestamp, E> {
        value.parse().map_err(E::custom)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Timestamp, A::Error> {
        let year: i32 = element(&mut seq, 0, &self)?;
        let day_of_year: u16 = element(&mut seq, 1, &self)?;
        let hour: u8 = element(&mut seq, 2, &self)?;
        let minute: u8 = element(&mut seq, 3, &self)?;
        let second: u8 = element(&mut seq, 4, &self)?;
        let nanosecond: u32 = element(&mut seq, 5, &self)?;
        let offset_hours: i8 = element(&mut seq, 6, &self)?;
        let offset_minutes: i8 = element(&mut seq, 7, &self)?;
        let offset_seconds: i8 = element(&mut seq, 8, &self)?;

        let year = i16::try_from(year).map_err(|_| {
            de::Error::invalid_value(Unexpected::Signed(year.into()), &"a year in -9999..=9999")
        })?;
        let day_of_year = i16::try_from(day_of_year).map_err(|_| {
            de::Error::invalid_value(Unexpected::Unsigned(day_of_year.into()), &"a day of year")
        })?;
        let nanosecond = i32::try_from(nanosecond).map_err(|_| {
            de::Error::invalid_value(Unexpected::Unsigned(nanosecond.into()), &"a nanosecond")
        })?;
        let offset_seconds = i32::from(offset_hours) * 3600
            + i32::from(offset_minutes) * 60
            + i32::from(offset_seconds);

        let date = Date::new(year, 1, 1)
            .and_then(|start| start.with().day_of_year(day_of_year).build())
            .map_err(de::Error::custom)?;
        let time =
            Time::new(hour.cast_signed(), minute.cast_signed(), second.cast_signed(), nanosecond)
                .map_err(de::Error::custom)?;
        let offset = Offset::from_seconds(offset_seconds).map_err(de::Error::custom)?;

        offset.to_timestamp(DateTime::from_parts(date, time)).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde::{Deserialize, Serialize};
    use serde_with::serde_as;

    use super::*;

    #[serde_as]
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Holder(#[serde_as(as = "OffsetDateTimeTuple")] Timestamp);

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    /// The bytes `time` 0.3.55 writes through `rmp_serde` for these instants.
    #[rstest]
    #[case::utc(
        "2024-01-22T14:35:07.5Z",
        &[153, 205, 7, 232, 22, 14, 35, 7, 206, 29, 205, 101, 0, 0, 0, 0]
    )]
    #[case::epoch("1970-01-01T00:00:00Z", &[153, 205, 7, 178, 1, 0, 0, 0, 0, 0, 0, 0])]
    fn writes_what_time_writes(#[case] instant: &str, #[case] bytes: &[u8]) {
        assert_eq!(rmp_serde::to_vec(&Holder(ts(instant))).unwrap(), bytes);
        assert_eq!(rmp_serde::from_slice::<Holder>(bytes).unwrap(), Holder(ts(instant)));
    }

    /// `time` keeps the offset it was given, so records written before the move to jiff can
    /// carry any offset.
    #[rstest]
    fn reads_a_non_utc_offset() {
        // 2024-01-22 14:35:07.5 +02:00, as `time` 0.3.55 writes it.
        let bytes = [153, 205, 7, 232, 22, 14, 35, 7, 206, 29, 205, 101, 0, 2, 0, 0];
        assert_eq!(
            rmp_serde::from_slice::<Holder>(&bytes).unwrap(),
            Holder(ts("2024-01-22T12:35:07.5Z"))
        );
    }

    #[rstest]
    #[case::day_of_year_zero(&[153, 205, 7, 232, 0, 0, 0, 0, 0, 0, 0, 0])]
    #[case::hour_out_of_range(&[153, 205, 7, 232, 1, 24, 0, 0, 0, 0, 0, 0])]
    #[case::too_short(&[147, 205, 7, 232, 1, 0])]
    fn rejects_malformed_tuples(#[case] bytes: &[u8]) {
        assert!(rmp_serde::from_slice::<Holder>(bytes).is_err());
    }

    #[rstest]
    fn human_readable_formats_get_rfc3339() {
        let holder = Holder(ts("2024-01-22T14:35:07.5Z"));
        let json = serde_json::to_string(&holder).unwrap();
        assert_eq!(json, r#""2024-01-22T14:35:07.5Z""#);
        assert_eq!(serde_json::from_str::<Holder>(&json).unwrap(), holder);
    }
}
