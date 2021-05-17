use std::ops::Range;

use byteorder::{BigEndian, ByteOrder};
use bytes::Bytes;

use crate::error::Error;
use crate::io::Decode;

/// A row of data from the database.
#[derive(Debug)]
pub struct DataRow {
    pub(crate) storage: Bytes,

    /// Ranges into the stored row data.
    /// This uses `u32` instead of usize to reduce the size of this type. Values cannot be larger
    /// than `i32` in postgres.
    pub(crate) values: Vec<Option<Range<u32>>>,
}

impl DataRow {
    #[inline]
    pub(crate) fn get(&self, index: usize) -> Option<&'_ [u8]> {
        self.values[index]
            .as_ref()
            .map(|col| &self.storage[(col.start as usize)..(col.end as usize)])
    }
}

impl Decode<'_> for DataRow {
    fn decode_with(buf: Bytes, _: ()) -> Result<Self, Error> {
        let cnt = BigEndian::read_u16(&buf) as usize;

        let mut values = Vec::with_capacity(cnt);
        let mut offset = 2;

        for _ in 0..cnt {
            // Length of the column value, in bytes (this count does not include itself).
            // Can be zero. As a special case, -1 indicates a NULL column value.
            // No value bytes follow in the NULL case.
            let length = BigEndian::read_i32(&buf[(offset as usize)..]);
            offset += 4;

            if length < 0 {
                values.push(None);
            } else {
                values.push(Some(offset..(offset + length as u32)));
                offset += length as u32;
            }
        }

        Ok(Self {
            storage: buf,
            values,
        })
    }
}

#[test]
fn test_decode_data_row() {
    const DATA: &[u8] = b"\x00\x08\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00\n\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00\x14\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00(\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00P";

    let row = DataRow::decode(DATA.into()).unwrap();

    assert_eq!(row.values.len(), 8);

    assert!(row.get(0).is_none());
    assert_eq!(row.get(1).unwrap(), &[0_u8, 0, 0, 10][..]);
    assert!(row.get(2).is_none());
    assert_eq!(row.get(3).unwrap(), &[0_u8, 0, 0, 20][..]);
    assert!(row.get(4).is_none());
    assert_eq!(row.get(5).unwrap(), &[0_u8, 0, 0, 40][..]);
    assert!(row.get(6).is_none());
    assert_eq!(row.get(7).unwrap(), &[0_u8, 0, 0, 80][..]);
}

#[cfg(all(test, not(debug_assertions)))]
#[bench]
fn bench_data_row_get(b: &mut test::Bencher) {
    const DATA: &[u8] = b"\x00\x08\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00\n\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00\x14\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00(\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00P";

    let row = DataRow::decode(test::black_box(Bytes::from_static(DATA))).unwrap();

    b.iter(|| {
        let _value = test::black_box(&row).get(3);
    });
}

#[cfg(all(test, not(debug_assertions)))]
#[bench]
fn bench_decode_data_row(b: &mut test::Bencher) {
    const DATA: &[u8] = b"\x00\x08\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00\n\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00\x14\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00(\xff\xff\xff\xff\x00\x00\x00\x04\x00\x00\x00P";

    b.iter(|| {
        let _ = DataRow::decode(test::black_box(Bytes::from_static(DATA)));
    });
}
