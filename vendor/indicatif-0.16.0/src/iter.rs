use crate::progress_bar::ProgressBar;
use std::convert::TryFrom;
use std::io::{self, IoSliceMut};
use std::iter::FusedIterator;

/// Wraps an iterator to display its progress.
pub trait ProgressIterator
where
    Self: Sized + Iterator,
{
    /// Wrap an iterator with default styling. Uses `Iterator::size_hint` to get length.
    /// Returns `Some(..)` only if `size_hint.1` is `Some`. If you want to create a progress bar
    /// even if `size_hint.1` returns `None` use `progress_count` or `progress_with` instead.
    fn try_progress(self) -> Option<ProgressBarIter<Self>> {
        self.size_hint()
            .1
            .map(|len| self.progress_count(u64::try_from(len).unwrap()))
    }

    /// Wrap an iterator with default styling.
    fn progress(self) -> ProgressBarIter<Self>
    where
        Self: ExactSizeIterator,
    {
        let len = u64::try_from(self.len()).unwrap();
        self.progress_count(len)
    }

    /// Wrap an iterator with an explicit element count.
    fn progress_count(self, len: u64) -> ProgressBarIter<Self> {
        self.progress_with(ProgressBar::new(len))
    }

    /// Wrap an iterator with a custom progress bar.
    fn progress_with(self, progress: ProgressBar) -> ProgressBarIter<Self>;
}

/// Wraps an iterator to display its progress.
#[derive(Debug)]
pub struct ProgressBarIter<T> {
    pub(crate) it: T,
    pub progress: ProgressBar,
}

impl<S, T: Iterator<Item = S>> Iterator for ProgressBarIter<T> {
    type Item = S;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.it.next();

        if item.is_some() {
            self.progress.inc(1);
        } else if !self.progress.is_finished() {
            self.progress.finish_using_style();
        }

        item
    }
}

impl<T: ExactSizeIterator> ExactSizeIterator for ProgressBarIter<T> {
    fn len(&self) -> usize {
        self.it.len()
    }
}

impl<T: DoubleEndedIterator> DoubleEndedIterator for ProgressBarIter<T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let item = self.it.next_back();

        if item.is_some() {
            self.progress.inc(1);
        } else if !self.progress.is_finished() {
            self.progress.finish_using_style();
        }

        item
    }
}

impl<T: FusedIterator> FusedIterator for ProgressBarIter<T> {}

impl<R: io::Read> io::Read for ProgressBarIter<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let inc = self.it.read(buf)?;
        self.progress.inc(inc as u64);
        Ok(inc)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let inc = self.it.read_vectored(bufs)?;
        self.progress.inc(inc as u64);
        Ok(inc)
    }

    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        let inc = self.it.read_to_string(buf)?;
        self.progress.inc(inc as u64);
        Ok(inc)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.it.read_exact(buf)?;
        self.progress.inc(buf.len() as u64);
        Ok(())
    }
}

impl<R: io::BufRead> io::BufRead for ProgressBarIter<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.it.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.it.consume(amt);
        self.progress.inc(amt as u64);
    }
}

impl<S: io::Seek> io::Seek for ProgressBarIter<S> {
    fn seek(&mut self, f: io::SeekFrom) -> io::Result<u64> {
        self.it.seek(f).map(|pos| {
            self.progress.set_position(pos);
            pos
        })
    }
}

impl<W: io::Write> io::Write for ProgressBarIter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.it.write(buf).map(|inc| {
            self.progress.inc(inc as u64);
            inc
        })
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice]) -> io::Result<usize> {
        self.it.write_vectored(bufs).map(|inc| {
            self.progress.inc(inc as u64);
            inc
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.it.flush()
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.it.write_all(buf).map(|()| {
            self.progress.inc(buf.len() as u64);
        })
    }

    // write_fmt can not be captured with reasonable effort.
    // as it uses write_all internally by default that should not be a problem.
    // fn write_fmt(&mut self, fmt: fmt::Arguments) -> io::Result<()>;
}

impl<S, T: Iterator<Item = S>> ProgressIterator for T {
    fn progress_with(self, progress: ProgressBar) -> ProgressBarIter<Self> {
        ProgressBarIter { it: self, progress }
    }
}

#[cfg(test)]
mod test {
    use crate::iter::{ProgressBarIter, ProgressIterator};
    use crate::progress_bar::ProgressBar;

    #[test]
    fn it_can_wrap_an_iterator() {
        let v = vec![1, 2, 3];
        let wrap = |it: ProgressBarIter<_>| {
            assert_eq!(it.map(|x| x * 2).collect::<Vec<_>>(), vec![2, 4, 6]);
        };

        wrap(v.iter().progress());
        wrap(v.iter().progress_count(3));
        wrap({
            let pb = ProgressBar::new(v.len() as u64);
            v.iter().progress_with(pb)
        });
    }
}
