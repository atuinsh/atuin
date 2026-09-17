//! File descriptor write helpers.

use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::os::fd::AsFd;
use std::time::Duration;

use rustix::io::{Errno, write};

use crate::futures::Backoff;

/// Write an entire buffer to a file descriptor.
pub trait WriteAllExt: AsFd {
    /// Write all of `buf`, looping over short writes and retrying `EINTR`. On `EAGAIN` it backs off
    /// (sleeping the thread) and retries up to `timeout`, then returns `ETIMEDOUT`; other errors
    /// propagate.
    fn write_all_retrying(&self, buf: &[u8], timeout: Duration) -> Result<(), Errno> {
        if buf.len() == 0 {
            return Ok(());
        }

        let fd = self.as_fd();
        let mut buf = buf;

        let backoff = Backoff::Exponential {
            initial: Duration::from_millis(1),
            max: Duration::from_millis(50),
            factor: NonZeroU32::new(2).unwrap(),
        };

        let done = backoff.retry_blocking(
            || loop {
                match write(fd, buf) {
                    // A 0-byte write on a non-empty buffer makes no progress.
                    Ok(0) => return ControlFlow::Break(Err(Errno::IO)),
                    Ok(n) => {
                        buf = &buf[n..];
                        if buf.is_empty() {
                            return ControlFlow::Break(Ok(()));
                        }
                    }
                    Err(Errno::INTR) => {}
                    Err(Errno::AGAIN) => return ControlFlow::Continue(()),
                    Err(e) => return ControlFlow::Break(Err(e)),
                }
            },
            timeout,
        );

        done.unwrap_or(Err(Errno::TIMEDOUT))
    }
}

impl<Fd: AsFd + ?Sized> WriteAllExt for Fd {}

#[cfg(test)]
mod tests {
    use std::io::{Read, pipe};
    use std::thread;

    use rstest::rstest;
    use rustix::io::ioctl_fionbio;

    use super::*;

    /// Larger than any pipe's kernel capacity, so writing it forces multiple
    /// `write` calls (and, on a full non-blocking pipe, `EAGAIN`).
    const BIG: usize = 512 * 1024;

    #[rstest]
    #[case::single_write(4 * 1024)]
    #[case::short_write_loop(BIG)]
    fn writes_everything(#[case] size: usize) {
        let (mut reader, writer) = pipe().unwrap();
        let collector = thread::spawn(move || {
            let mut got = Vec::new();
            reader.read_to_end(&mut got).unwrap();
            got
        });

        let data = vec![0xACu8; size];
        writer.write_all_retrying(&data, Duration::from_secs(10)).unwrap();
        drop(writer); // EOF for the reader

        assert_eq!(collector.join().unwrap(), data);
    }

    #[rstest]
    fn times_out_when_the_fd_never_drains() {
        // Nobody reads `_reader`, so a non-blocking write fills the pipe and stalls.
        let (_reader, writer) = pipe().unwrap();
        ioctl_fionbio(&writer, true).unwrap();
        let err =
            writer.write_all_retrying(&vec![0u8; BIG], Duration::from_millis(50)).unwrap_err();
        assert_eq!(err, Errno::TIMEDOUT);
    }
}
