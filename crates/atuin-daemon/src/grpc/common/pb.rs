use atuin_domain::record::RecordId as DomainRecordId;

mod codegen {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]
    tonic::include_proto!("common");
}

pub use codegen::*;

impl From<DomainRecordId> for RecordId {
    fn from(value: DomainRecordId) -> Self {
        Self {
            uuid: Some(Uuid {
                value: value.0.into_bytes().to_vec(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::range::PyStyleIdxRange;
    use prost::Message as _;
    use rstest::rstest;

    /// `common.PyStyleIdxRange` is not generated: codegen maps it onto the `atuin-common` type,
    /// whose `prost` attributes are written by hand. These tests pin those attributes to what
    /// `common.proto` declares, so the two cannot drift apart unnoticed.
    #[rstest]
    #[case::negative_bounds(PyStyleIdxRange::new(-100, -1))]
    #[case::zero_value(PyStyleIdxRange::new(0, 0))]
    #[case::extremes(PyStyleIdxRange::new(i64::MIN, i64::MAX))]
    fn py_style_idx_range_round_trips_over_the_wire(#[case] range: PyStyleIdxRange) {
        let encoded = range.encode_to_vec();
        assert_eq!(PyStyleIdxRange::decode(&*encoded).expect("decode"), range);
    }

    #[rstest]
    fn py_style_idx_range_uses_the_field_numbers_from_common_proto() {
        // Field 1 (varint) = 3, field 2 (varint) = 7: the tags `common.proto` assigns to
        // `start` and `end`.
        let decoded = PyStyleIdxRange::decode([0x08, 0x03, 0x10, 0x07].as_slice()).expect("decode");
        assert_eq!(decoded, PyStyleIdxRange::new(3, 7));
    }
}
