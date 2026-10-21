//! File descriptor read and write helpers.

use std::os::fd::{AsFd, BorrowedFd};

use rustix::event::{PollFd, PollFlags, poll};
use rustix::io::{Errno, read, write};

/// Write an entire buffer to a file descriptor.
pub trait WriteAllExt: AsFd {
    /// Write all of `buf`, looping over short writes and retrying on `EINTR` and `EAGAIN`.
    ///
    /// If the file descriptor is in non-blocking mode and is temporarily full (`EAGAIN`), this
    /// function blocks until the file descriptor can accept more data (repeatedly, until the entire
    /// buffer is written). This is unlike [`std::io::Stdout`], which immediately returns after
    /// receiving `EAGAIN`.
    fn write_all_retrying(&self, buf: &[u8]) -> Result<(), Errno> {
        let fd = self.as_fd();
        let mut buf = buf;

        while !buf.is_empty() {
            match write(fd, buf) {
                // A 0-byte write on a non-empty buffer makes no progress.
                Ok(0) => return Err(Errno::IO),
                Ok(n) => buf = &buf[n..],
                Err(Errno::INTR) => {}
                Err(Errno::AGAIN) => wait_until(fd, PollFlags::OUT)?,
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }
}

impl<Fd: AsFd + ?Sized> WriteAllExt for Fd {}

/// Read from a file descriptor, waiting for input.
pub trait ReadRetryingExt: AsFd {
    /// Read into `buf`, retrying on `EINTR` and `EAGAIN`; `0` means the end of input.
    ///
    /// If the file descriptor is in non-blocking mode and has nothing to read yet (`EAGAIN`), this
    /// function blocks until it does, as a blocking read would. This is unlike [`std::io::Stdin`],
    /// which immediately returns after receiving `EAGAIN`.
    fn read_retrying(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        let fd = self.as_fd();

        loop {
            match read(fd, &mut *buf) {
                Ok(n) => return Ok(n),
                Err(Errno::INTR) => {}
                Err(Errno::AGAIN) => wait_until(fd, PollFlags::IN)?,
                Err(e) => return Err(e),
            }
        }
    }
}

impl<Fd: AsFd + ?Sized> ReadRetryingExt for Fd {}

/// Block until a file descriptor is ready for `events`.
fn wait_until(fd: BorrowedFd<'_>, events: PollFlags) -> Result<(), Errno> {
    let mut fds = [PollFd::new(&fd, events)];

    loop {
        match poll(&mut fds, None) {
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => {}
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write, pipe};
    use std::thread;
    use std::time::Duration;

    use rstest::rstest;
    use rustix::io::ioctl_fionbio;

    use super::*;

    /// Larger than any pipe's kernel capacity, so writing it forces multiple
    /// `write` calls (and, on a full non-blocking pipe, `EAGAIN`).
    const BIG: usize = 512 * 1024;

    #[rstest]
    #[case::single_write(4 * 1024)]
    #[case::short_write_loop(BIG)]
    fn writes_everything(#[case] size: usize, #[values(false, true)] nonblocking: bool) {
        let (mut reader, writer) = pipe().unwrap();
        ioctl_fionbio(&writer, nonblocking).unwrap();
        let collector = thread::spawn(move || {
            let mut got = Vec::new();
            reader.read_to_end(&mut got).unwrap();
            got
        });

        let data = vec![0xACu8; size];
        writer.write_all_retrying(&data).unwrap();
        drop(writer); // EOF for the reader

        assert_eq!(collector.join().unwrap(), data);
    }

    /// A non-blocking sink that is full *right now* is not a failure: the write waits for it
    /// to drain. The stall is far longer than the 150ms the pty-proxy forwarder used to give
    /// up after, which is the whole point of having no deadline.
    #[rstest]
    #[timeout(Duration::from_secs(30))]
    fn waits_out_a_stalled_nonblocking_fd() {
        let (mut reader, writer) = pipe().unwrap();
        ioctl_fionbio(&writer, true).unwrap();

        let collector = thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            let mut got = Vec::new();
            reader.read_to_end(&mut got).unwrap();
            got
        });

        let data = vec![0xACu8; BIG];
        writer.write_all_retrying(&data).unwrap();
        drop(writer); // EOF for the reader

        assert_eq!(collector.join().unwrap(), data);
    }

    /// Waiting forever is only safe if a descriptor that will never drain still ends the
    /// loop. Fill the pipe so the write parks in `poll`, then close the read end under it.
    #[rstest]
    #[timeout(Duration::from_secs(30))]
    fn a_reader_that_vanishes_mid_write_errors_instead_of_hanging() {
        let (reader, writer) = pipe().unwrap();
        ioctl_fionbio(&writer, true).unwrap();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            drop(reader);
        });

        let err = writer.write_all_retrying(&vec![0u8; BIG]).unwrap_err();
        assert_eq!(err, Errno::PIPE);
    }

    /// The read-side twin of `waits_out_a_stalled_nonblocking_fd`: an empty non-blocking pipe
    /// is input that hasn't arrived yet, not the end of it.
    #[rstest]
    #[timeout(Duration::from_secs(30))]
    fn a_read_waits_for_input(#[values(false, true)] nonblocking: bool) {
        let (reader, mut writer) = pipe().unwrap();
        ioctl_fionbio(&reader, nonblocking).unwrap();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            writer.write_all(b"typed").unwrap();
        });

        let mut buf = [0u8; 64];
        let n = reader.read_retrying(&mut buf).unwrap();

        assert_eq!(&buf[..n], b"typed");
    }

    #[rstest]
    fn a_read_at_the_end_of_input_returns_zero(#[values(false, true)] nonblocking: bool) {
        let (reader, writer) = pipe().unwrap();
        ioctl_fionbio(&reader, nonblocking).unwrap();
        drop(writer);

        assert_eq!(reader.read_retrying(&mut [0u8; 64]), Ok(0));
    }
}
