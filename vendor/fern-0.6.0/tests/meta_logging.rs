//! This provides testing of the 'meta-logging' feature, which allows for
//! deadlock-free logging within logging formatters.
//!
//! These tests *will* deadlock if the feature is not enabled, so they're
//! disabled by default.
#![cfg(feature = "meta-logging-in-format")]
use std::{fmt, fs, io, io::prelude::*};

use log::{Level::*, Log};

mod support;

use support::manual_log;

// in order to actually trigger the situation that deadlocks, we need a custom
// Display implementation which performs logging:
struct VerboseDisplayThing<'a> {
    log_copy: &'a dyn Log,
    msg: &'a str,
}

impl<'a> fmt::Display for VerboseDisplayThing<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        manual_log(
            self.log_copy,
            Debug,
            format_args!(
                "VerboseDisplayThing is being displayed! [contents: {}]",
                self.msg
            ),
        );
        f.write_str(self.msg)
    }
}

#[test]
fn file_deadlock() {
    // Create a temporary directory to put a log file into for testing
    let temp_log_dir = tempdir::TempDir::new("fern").expect("Failed to set up temporary directory");
    let log_file = temp_log_dir.path().join("test.log");

    {
        let (_max_level, logger) = fern::Dispatch::new()
            .format(|out, msg, record| out.finish(format_args!("[{}] {}", record.level(), msg)))
            .chain(io::stdout())
            .chain(fern::log_file(log_file).expect("Failed to open log file"))
            .into_log();

        let l = &*logger;

        manual_log(
            l,
            Info,
            format_args!(
                "Hello, world! {}",
                VerboseDisplayThing {
                    log_copy: l,
                    msg: "it's verbose!",
                }
            ),
        );

        // ensure all File objects are dropped and OS buffers are flushed.
        log::logger().flush();

        {
            let contents = {
                let mut log_read = fs::File::open(&temp_log_dir.path().join("test.log")).unwrap();
                let mut buf = String::new();
                log_read.read_to_string(&mut buf).unwrap();
                buf
            };
            assert_eq!(
                contents,
                // double logs because we're logging to stdout & the file
                "[DEBUG] VerboseDisplayThing is being displayed! [contents: it's verbose!]\
                 \n[DEBUG] VerboseDisplayThing is being displayed! [contents: it's verbose!]\
                 \n[INFO] Hello, world! it's verbose!\n"
            );
        }
    } // ensure logger is dropped before temp dir

    temp_log_dir
        .close()
        .expect("Failed to clean up temporary directory");
}
