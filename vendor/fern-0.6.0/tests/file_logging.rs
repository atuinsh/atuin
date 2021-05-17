//! Tests!
use std::{fs, io, io::prelude::*};

use log::Level::*;

mod support;

use support::manual_log;

#[test]
fn test_basic_logging_file_logging() {
    // Create a temporary directory to put a log file into for testing
    let temp_log_dir = tempdir::TempDir::new("fern").expect("Failed to set up temporary directory");
    let log_file = temp_log_dir.path().join("test.log");

    {
        // Create a basic logger configuration
        let (_max_level, logger) = fern::Dispatch::new()
            .format(|out, msg, record| out.finish(format_args!("[{}] {}", record.level(), msg)))
            .level(log::LevelFilter::Info)
            .chain(io::stdout())
            .chain(fern::log_file(log_file).expect("Failed to open log file"))
            .into_log();

        let l = &*logger;
        manual_log(l, Trace, "SHOULD NOT DISPLAY");
        manual_log(l, Debug, "SHOULD NOT DISPLAY");
        manual_log(l, Info, "Test information message");
        manual_log(l, Warn, "Test warning message");
        manual_log(l, Error, "Test error message");

        // ensure all File objects are dropped and OS buffers are flushed.
        log::logger().flush();

        {
            let result = {
                let mut log_read = fs::File::open(&temp_log_dir.path().join("test.log")).unwrap();
                let mut buf = String::new();
                log_read.read_to_string(&mut buf).unwrap();
                buf
            };
            assert!(
                !result.contains("SHOULD NOT DISPLAY"),
                "expected result not including \"SHOULD_NOT_DISPLAY\", found:\n```\n{}\n```\n",
                result
            );
            assert!(
                result.contains("[INFO] Test information message"),
                "expected result including \"[INFO] Test information message\", found:\n```\n{}\n```\n",
                result
            );
            assert!(
                result.contains("[WARN] Test warning message"),
                "expected result including \"[WARN] Test warning message\", found:\n```\n{}\n```\n",
                result
            );
            assert!(
                result.contains("[ERROR] Test error message"),
                "expected result to not include \"[ERROR] Test error message\", found:\n```\n{}\n```\n",
                result
            );
        }
    } // ensure logger is dropped before temp dir

    temp_log_dir
        .close()
        .expect("Failed to clean up temporary directory");
}

#[test]
fn test_custom_line_separators() {
    // Create a temporary directory to put a log file into for testing
    let temp_log_dir = tempdir::TempDir::new("fern").expect("Failed to set up temporary directory");
    let log_file = temp_log_dir.path().join("test_custom_line_sep.log");

    {
        // Create a basic logger configuration
        let (_max_level, logger) = fern::Dispatch::new()
            // default format is just the message if not specified
            // default log level is 'trace' if not specified (logs all messages)
            // output to the log file with the "\r\n" line separator.
            .chain(fern::Output::file(
                fern::log_file(&log_file).expect("Failed to open log file"),
                "\r\n",
            ))
            .into_log();

        let l = &*logger;
        manual_log(l, Info, "message1");
        manual_log(l, Info, "message2");

        // ensure all File objects are dropped and OS buffers are flushed.
        logger.flush();

        {
            let result = {
                let mut log_read =
                    fs::File::open(&temp_log_dir.path().join("test_custom_line_sep.log")).unwrap();
                let mut buf = String::new();
                log_read.read_to_string(&mut buf).unwrap();
                buf
            };
            assert_eq!(&result, "message1\r\nmessage2\r\n");
        }
    } // ensure logger is dropped before temp dir

    temp_log_dir
        .close()
        .expect("Failed to clean up temporary directory");
}
