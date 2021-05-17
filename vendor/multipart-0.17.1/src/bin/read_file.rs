#[macro_use] extern crate log;
extern crate multipart;
extern crate rand;

use multipart::server::Multipart;

use rand::{Rng, ThreadRng};

use std::fs::File;
use std::env;
use std::io::{self, Read};

const LOG_LEVEL: log::LevelFilter = log::LevelFilter::Debug;

struct SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        LOG_LEVEL.to_level()
            .map_or(false, |level| metadata.level() <= level)
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            println!("{} - {}", record.level(), record.args());
        }
    }
    
    fn flush(&self) {}
}

static LOGGER: SimpleLogger = SimpleLogger;

fn main() {
    log::set_logger(&LOGGER).expect("Could not initialize logger");

    let mut args = env::args().skip(1);

    let boundary = args.next().expect("Boundary must be provided as the first argument");

    let file = args.next().expect("Filename must be provided as the second argument");

    let file = File::open(file).expect("Could not open file");

    let reader = RandomReader {
        inner: file,
        rng: rand::thread_rng()
    };

    let mut multipart = Multipart::with_body(reader, boundary);

    while let Some(field) = multipart.read_entry().unwrap() {
        println!("Read field: {:?}", field.headers.name);
    }

    println!("All entries read!");
}

struct RandomReader<R> {
    inner: R,
    rng: ThreadRng,
}

impl<R: Read> Read for RandomReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.len() == 0 {
            debug!("RandomReader::read() passed a zero-sized buffer.");
            return Ok(0);
        }

        let len = self.rng.gen_range(1, buf.len() + 1);

        self.inner.read(&mut buf[..len])
    }
}
