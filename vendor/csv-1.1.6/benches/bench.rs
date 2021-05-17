#![feature(test)]

extern crate test;

use std::io;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use test::Bencher;

use csv::{
    ByteRecord, Reader, ReaderBuilder, StringRecord, Trim, Writer,
    WriterBuilder,
};

static NFL: &'static str = include_str!("../examples/data/bench/nfl.csv");
static GAME: &'static str = include_str!("../examples/data/bench/game.csv");
static POP: &'static str =
    include_str!("../examples/data/bench/worldcitiespop.csv");
static MBTA: &'static str =
    include_str!("../examples/data/bench/gtfs-mbta-stop-times.csv");

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct NFLRowOwned {
    gameid: String,
    qtr: i32,
    min: Option<i32>,
    sec: Option<i32>,
    off: String,
    def: String,
    down: Option<i32>,
    togo: Option<i32>,
    ydline: Option<i32>,
    description: String,
    offscore: i32,
    defscore: i32,
    season: i32,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct NFLRowBorrowed<'a> {
    gameid: &'a str,
    qtr: i32,
    min: Option<i32>,
    sec: Option<i32>,
    off: &'a str,
    def: &'a str,
    down: Option<i32>,
    togo: Option<i32>,
    ydline: Option<i32>,
    description: &'a str,
    offscore: i32,
    defscore: i32,
    season: i32,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct GAMERowOwned(String, String, String, String, i32, String);

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct GAMERowBorrowed<'a>(&'a str, &'a str, &'a str, &'a str, i32, &'a str);

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase")]
struct POPRowOwned {
    country: String,
    city: String,
    accent_city: String,
    region: String,
    population: Option<i32>,
    latitude: f64,
    longitude: f64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase")]
struct POPRowBorrowed<'a> {
    country: &'a str,
    city: &'a str,
    accent_city: &'a str,
    region: &'a str,
    population: Option<i32>,
    latitude: f64,
    longitude: f64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct MBTARowOwned {
    trip_id: String,
    arrival_time: String,
    departure_time: String,
    stop_id: String,
    stop_sequence: i32,
    stop_headsign: String,
    pickup_type: i32,
    drop_off_type: i32,
    timepoint: i32,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct MBTARowBorrowed<'a> {
    trip_id: &'a str,
    arrival_time: &'a str,
    departure_time: &'a str,
    stop_id: &'a str,
    stop_sequence: i32,
    stop_headsign: &'a str,
    pickup_type: i32,
    drop_off_type: i32,
    timepoint: i32,
}

#[derive(Default)]
struct ByteCounter {
    count: usize,
}
impl io::Write for ByteCounter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.count += data.len();
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

macro_rules! bench {
    ($name:ident, $data:ident, $counter:ident, $result:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data = $data.as_bytes();
            b.bytes = data.len() as u64;
            b.iter(|| {
                let mut rdr =
                    ReaderBuilder::new().has_headers(false).from_reader(data);
                assert_eq!($counter(&mut rdr), $result);
            })
        }
    };
}

macro_rules! bench_trimmed {
    ($name:ident, $data:ident, $counter:ident, $result:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data = $data.as_bytes();
            b.bytes = data.len() as u64;
            b.iter(|| {
                let mut rdr = ReaderBuilder::new()
                    .has_headers(false)
                    .trim(Trim::All)
                    .from_reader(data);
                assert_eq!($counter(&mut rdr), $result);
            })
        }
    };
}

macro_rules! bench_serde {
    (no_headers,
     $name_de:ident, $name_ser:ident, $data:ident, $counter:ident, $type:ty, $result:expr) => {
        #[bench]
        fn $name_de(b: &mut Bencher) {
            let data = $data.as_bytes();
            b.bytes = data.len() as u64;
            b.iter(|| {
                let mut rdr =
                    ReaderBuilder::new().has_headers(false).from_reader(data);
                assert_eq!($counter::<_, $type>(&mut rdr), $result);
            })
        }
        #[bench]
        fn $name_ser(b: &mut Bencher) {
            let data = $data.as_bytes();
            let values = ReaderBuilder::new()
                .has_headers(false)
                .from_reader(data)
                .deserialize()
                .collect::<Result<Vec<$type>, _>>()
                .unwrap();

            let do_it = || {
                let mut counter = ByteCounter::default();
                {
                    let mut wtr = WriterBuilder::new()
                        .has_headers(false)
                        .from_writer(&mut counter);
                    for val in &values {
                        wtr.serialize(val).unwrap();
                    }
                }
                counter.count
            };
            b.bytes = do_it() as u64;
            b.iter(do_it)
        }
    };
    ($name_de:ident, $name_ser:ident, $data:ident, $counter:ident, $type:ty, $result:expr) => {
        #[bench]
        fn $name_de(b: &mut Bencher) {
            let data = $data.as_bytes();
            b.bytes = data.len() as u64;
            b.iter(|| {
                let mut rdr =
                    ReaderBuilder::new().has_headers(true).from_reader(data);
                assert_eq!($counter::<_, $type>(&mut rdr), $result);
            })
        }
        #[bench]
        fn $name_ser(b: &mut Bencher) {
            let data = $data.as_bytes();
            let values = ReaderBuilder::new()
                .has_headers(true)
                .from_reader(data)
                .deserialize()
                .collect::<Result<Vec<$type>, _>>()
                .unwrap();

            let do_it = || {
                let mut counter = ByteCounter::default();
                {
                    let mut wtr = WriterBuilder::new()
                        .has_headers(true)
                        .from_writer(&mut counter);
                    for val in &values {
                        wtr.serialize(val).unwrap();
                    }
                }
                counter.count
            };
            b.bytes = do_it() as u64;
            b.iter(do_it)
        }
    };
}

macro_rules! bench_serde_borrowed_bytes {
    ($name:ident, $data:ident, $type:ty, $headers:expr, $result:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data = $data.as_bytes();
            b.bytes = data.len() as u64;
            b.iter(|| {
                let mut rdr = ReaderBuilder::new()
                    .has_headers($headers)
                    .from_reader(data);
                let mut count = 0;
                let mut rec = ByteRecord::new();
                while rdr.read_byte_record(&mut rec).unwrap() {
                    let _: $type = rec.deserialize(None).unwrap();
                    count += 1;
                }
                count
            })
        }
    };
}

macro_rules! bench_serde_borrowed_str {
    ($name:ident, $data:ident, $type:ty, $headers:expr, $result:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data = $data.as_bytes();
            b.bytes = data.len() as u64;
            b.iter(|| {
                let mut rdr = ReaderBuilder::new()
                    .has_headers($headers)
                    .from_reader(data);
                let mut count = 0;
                let mut rec = StringRecord::new();
                while rdr.read_record(&mut rec).unwrap() {
                    let _: $type = rec.deserialize(None).unwrap();
                    count += 1;
                }
                count
            })
        }
    };
}

bench_serde!(
    count_nfl_deserialize_owned_bytes,
    count_nfl_serialize_owned_bytes,
    NFL,
    count_deserialize_owned_bytes,
    NFLRowOwned,
    9999
);
bench_serde!(
    count_nfl_deserialize_owned_str,
    count_nfl_serialize_owned_str,
    NFL,
    count_deserialize_owned_str,
    NFLRowOwned,
    9999
);
bench_serde_borrowed_bytes!(
    count_nfl_deserialize_borrowed_bytes,
    NFL,
    NFLRowBorrowed,
    true,
    9999
);
bench_serde_borrowed_str!(
    count_nfl_deserialize_borrowed_str,
    NFL,
    NFLRowBorrowed,
    true,
    9999
);
bench!(count_nfl_iter_bytes, NFL, count_iter_bytes, 130000);
bench_trimmed!(count_nfl_iter_bytes_trimmed, NFL, count_iter_bytes, 130000);
bench!(count_nfl_iter_str, NFL, count_iter_str, 130000);
bench_trimmed!(count_nfl_iter_str_trimmed, NFL, count_iter_str, 130000);
bench!(count_nfl_read_bytes, NFL, count_read_bytes, 130000);
bench!(count_nfl_read_str, NFL, count_read_str, 130000);
bench_serde!(
    no_headers,
    count_game_deserialize_owned_bytes,
    count_game_serialize_owned_bytes,
    GAME,
    count_deserialize_owned_bytes,
    GAMERowOwned,
    100000
);
bench_serde!(
    no_headers,
    count_game_deserialize_owned_str,
    count_game_serialize_owned_str,
    GAME,
    count_deserialize_owned_str,
    GAMERowOwned,
    100000
);
bench_serde_borrowed_bytes!(
    count_game_deserialize_borrowed_bytes,
    GAME,
    GAMERowBorrowed,
    true,
    100000
);
bench_serde_borrowed_str!(
    count_game_deserialize_borrowed_str,
    GAME,
    GAMERowBorrowed,
    true,
    100000
);
bench!(count_game_iter_bytes, GAME, count_iter_bytes, 600000);
bench!(count_game_iter_str, GAME, count_iter_str, 600000);
bench!(count_game_read_bytes, GAME, count_read_bytes, 600000);
bench!(count_game_read_str, GAME, count_read_str, 600000);
bench_serde!(
    count_pop_deserialize_owned_bytes,
    count_pop_serialize_owned_bytes,
    POP,
    count_deserialize_owned_bytes,
    POPRowOwned,
    20000
);
bench_serde!(
    count_pop_deserialize_owned_str,
    count_pop_serialize_owned_str,
    POP,
    count_deserialize_owned_str,
    POPRowOwned,
    20000
);
bench_serde_borrowed_bytes!(
    count_pop_deserialize_borrowed_bytes,
    POP,
    POPRowBorrowed,
    true,
    20000
);
bench_serde_borrowed_str!(
    count_pop_deserialize_borrowed_str,
    POP,
    POPRowBorrowed,
    true,
    20000
);
bench!(count_pop_iter_bytes, POP, count_iter_bytes, 140007);
bench!(count_pop_iter_str, POP, count_iter_str, 140007);
bench!(count_pop_read_bytes, POP, count_read_bytes, 140007);
bench!(count_pop_read_str, POP, count_read_str, 140007);
bench_serde!(
    count_mbta_deserialize_owned_bytes,
    count_mbta_serialize_owned_bytes,
    MBTA,
    count_deserialize_owned_bytes,
    MBTARowOwned,
    9999
);
bench_serde!(
    count_mbta_deserialize_owned_str,
    count_mbta_serialize_owned_str,
    MBTA,
    count_deserialize_owned_str,
    MBTARowOwned,
    9999
);
bench_serde_borrowed_bytes!(
    count_mbta_deserialize_borrowed_bytes,
    MBTA,
    MBTARowBorrowed,
    true,
    9999
);
bench_serde_borrowed_str!(
    count_mbta_deserialize_borrowed_str,
    MBTA,
    MBTARowBorrowed,
    true,
    9999
);
bench!(count_mbta_iter_bytes, MBTA, count_iter_bytes, 90000);
bench!(count_mbta_iter_str, MBTA, count_iter_str, 90000);
bench!(count_mbta_read_bytes, MBTA, count_read_bytes, 90000);
bench!(count_mbta_read_str, MBTA, count_read_str, 90000);

macro_rules! bench_write {
    ($name:ident, $data:ident) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data = $data.as_bytes();
            b.bytes = data.len() as u64;
            let records = collect_records(data);

            b.iter(|| {
                let mut wtr = Writer::from_writer(vec![]);
                for r in &records {
                    wtr.write_record(r).unwrap();
                }
                assert!(wtr.flush().is_ok());
            })
        }
    };
}

macro_rules! bench_write_bytes {
    ($name:ident, $data:ident) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let data = $data.as_bytes();
            b.bytes = data.len() as u64;
            let records = collect_records(data);

            b.iter(|| {
                let mut wtr = Writer::from_writer(vec![]);
                for r in &records {
                    wtr.write_byte_record(r).unwrap();
                }
                assert!(wtr.flush().is_ok());
            })
        }
    };
}

bench_write!(write_nfl_record, NFL);
bench_write_bytes!(write_nfl_bytes, NFL);

fn count_deserialize_owned_bytes<R, D>(rdr: &mut Reader<R>) -> u64
where
    R: io::Read,
    D: DeserializeOwned,
{
    let mut count = 0;
    let mut rec = ByteRecord::new();
    while rdr.read_byte_record(&mut rec).unwrap() {
        let _: D = rec.deserialize(None).unwrap();
        count += 1;
    }
    count
}

fn count_deserialize_owned_str<R, D>(rdr: &mut Reader<R>) -> u64
where
    R: io::Read,
    D: DeserializeOwned,
{
    let mut count = 0;
    for rec in rdr.deserialize::<D>() {
        let _ = rec.unwrap();
        count += 1;
    }
    count
}

fn count_iter_bytes<R: io::Read>(rdr: &mut Reader<R>) -> u64 {
    let mut count = 0;
    for rec in rdr.byte_records() {
        count += rec.unwrap().len() as u64;
    }
    count
}

fn count_iter_str<R: io::Read>(rdr: &mut Reader<R>) -> u64 {
    let mut count = 0;
    for rec in rdr.records() {
        count += rec.unwrap().len() as u64;
    }
    count
}

fn count_read_bytes<R: io::Read>(rdr: &mut Reader<R>) -> u64 {
    let mut count = 0;
    let mut rec = ByteRecord::new();
    while rdr.read_byte_record(&mut rec).unwrap() {
        count += rec.len() as u64;
    }
    count
}

fn count_read_str<R: io::Read>(rdr: &mut Reader<R>) -> u64 {
    let mut count = 0;
    let mut rec = StringRecord::new();
    while rdr.read_record(&mut rec).unwrap() {
        count += rec.len() as u64;
    }
    count
}

fn collect_records(data: &[u8]) -> Vec<ByteRecord> {
    let mut rdr = ReaderBuilder::new().has_headers(false).from_reader(data);
    rdr.byte_records().collect::<Result<Vec<_>, _>>().unwrap()
}
