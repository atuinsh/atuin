//! File descriptor write helpers.

use std::os::fd::{AsFd, BorrowedFd};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Secs, Timespec, poll};
use rustix::io::{Errno, write};

/// Write an entire buffer to a file descriptor.
pub trait WriteAllExt: AsFd {
    /// Write all of `buf`, looping over short writes. Retries `EINTR`; on
    /// `EAGAIN` waits (via `poll`) for the fd to become writable, up to
    /// `timeout` per stall, else `ETIMEDOUT`. Other errors propagate.
    fn write_all_retrying(&self, mut buf: &[u8], timeout: Duration) -> Result<(), Errno> {
        let fd = self.as_fd();
        while !buf.is_empty() {
            match write(fd, buf) {
                // A 0-byte write on a non-empty buffer makes no progress.
                Ok(0) => return Err(Errno::IO),
                Ok(n) => buf = &buf[n..],
                Err(Errno::INTR) => {}
                Err(Errno::AGAIN) => wait_writable(fd, timeout)?,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl<Fd: AsFd + ?Sized> WriteAllExt for Fd {}

/// Block until `fd` is writable, for at most `timeout` in total.
fn wait_writable(fd: BorrowedFd<'_>, timeout: Duration) -> Result<(), Errno> {
    // `None` (a `checked_add` overflow) means an unbounded wait.
    let deadline = Instant::now().checked_add(timeout);
    loop {
        let ts = deadline.map(|d| {
            let remaining = d.saturating_duration_since(Instant::now());
            Timespec {
                tv_sec: remaining.as_secs().try_into().unwrap_or(Secs::MAX),
                tv_nsec: remaining.subsec_nanos().into(),
            }
        });
        let mut fds = [PollFd::new(&fd, PollFlags::OUT)];
        match poll(&mut fds, ts.as_ref()) {
            Ok(0) => return Err(Errno::TIMEDOUT),
            // Ready (or a revents error, which the next `write` will surface).
            Ok(_) => return Ok(()),
            // Re-poll with the time that remains.
            Err(Errno::INTR) => {}
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Read;
    use std::thread;

    use rustix::io::ioctl_fionbio;
    use rustix::pipe::pipe;

    use super::*;

    /// Larger than any pipe's kernel capacity, so writing it forces multiple
    /// `write` calls (and, on a full non-blocking pipe, `EAGAIN`).
    const BIG: usize = 512 * 1024;

    #[test]
    fn writes_everything_across_short_writes() {
        let (read, write_end) = pipe().unwrap();
        let reader = thread::spawn(move || {
            let mut got = Vec::new();
            File::from(read).read_to_end(&mut got).unwrap();
            got
        });

        let data = vec![0xACu8; BIG];
        write_end.write_all_retrying(&data, Duration::from_secs(10)).unwrap();
        drop(write_end); // EOF for the reader

        assert_eq!(reader.join().unwrap(), data);
    }

    #[test]
    fn times_out_when_the_fd_never_drains() {
        // Nobody reads `_read`, so a non-blocking write fills the pipe and stalls.
        let (_read, write_end) = pipe().unwrap();
        ioctl_fionbio(&write_end, true).unwrap();
        let err = write_end
            .write_all_retrying(&vec![0u8; BIG], Duration::from_millis(50))
            .unwrap_err();
        assert_eq!(err, Errno::TIMEDOUT);
    }
}
