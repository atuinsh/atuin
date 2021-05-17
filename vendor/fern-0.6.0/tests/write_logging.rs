//! Tests for the raw write logging functionality.
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use log::Level::*;

mod support;

use support::manual_log;

#[test]
fn test_raw_write_logging() {
    struct TestWriter {
        buf: Vec<u8>,
        flag: Arc<AtomicBool>,
    }

    impl io::Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.buf.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.buf.flush()?;

            let expected = b"[INFO] Test information message\n";

            if self.buf == expected {
                self.flag.store(true, Ordering::SeqCst);
            } else {
                eprintln!("{:?} does not match {:?}", self.buf, expected);
            }

            Ok(())
        }
    }

    let flag = Arc::new(AtomicBool::new(false));

    // Create a basic logger configuration
    let (_max_level, logger) = fern::Dispatch::new()
        .format(|out, msg, record| out.finish(format_args!("[{}] {}", record.level(), msg)))
        .level(log::LevelFilter::Info)
        .chain(io::stdout())
        .chain(Box::new(TestWriter {
            buf: Vec::new(),
            flag: flag.clone(),
        }) as Box<dyn io::Write + Send>)
        .into_log();

    let l = &*logger;
    manual_log(l, Info, "Test information message");

    // ensure all File objects are dropped and OS buffers are flushed.
    log::logger().flush();

    assert!(
        flag.load(Ordering::SeqCst),
        "raw Write test failed: did not match buffer"
    );
}
