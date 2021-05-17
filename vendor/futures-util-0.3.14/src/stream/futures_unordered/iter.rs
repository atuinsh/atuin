use super::FuturesUnordered;
use super::task::Task;
use core::marker::PhantomData;
use core::pin::Pin;
use core::sync::atomic::Ordering::Relaxed;

#[derive(Debug)]
/// Mutable iterator over all futures in the unordered set.
pub struct IterPinMut<'a, Fut> {
    pub(super) task: *const Task<Fut>,
    pub(super) len: usize,
    pub(super) _marker: PhantomData<&'a mut FuturesUnordered<Fut>>
}

#[derive(Debug)]
/// Mutable iterator over all futures in the unordered set.
pub struct IterMut<'a, Fut: Unpin> (pub(super) IterPinMut<'a, Fut>);

#[derive(Debug)]
/// Immutable iterator over all futures in the unordered set.
pub struct IterPinRef<'a, Fut> {
    pub(super) task: *const Task<Fut>,
    pub(super) len: usize,
    pub(super) pending_next_all: *mut Task<Fut>,
    pub(super) _marker: PhantomData<&'a FuturesUnordered<Fut>>
}

#[derive(Debug)]
/// Immutable iterator over all the futures in the unordered set.
pub struct Iter<'a, Fut: Unpin> (pub(super) IterPinRef<'a, Fut>);

impl<'a, Fut> Iterator for IterPinMut<'a, Fut> {
    type Item = Pin<&'a mut Fut>;

    fn next(&mut self) -> Option<Pin<&'a mut Fut>> {
        if self.task.is_null() {
            return None;
        }
        unsafe {
            let future = (*(*self.task).future.get()).as_mut().unwrap();

            // Mutable access to a previously shared `FuturesUnordered` implies
            // that the other threads already released the object before the
            // current thread acquired it, so relaxed ordering can be used and
            // valid `next_all` checks can be skipped.
            let next = (*self.task).next_all.load(Relaxed);
            self.task = next;
            self.len -= 1;
            Some(Pin::new_unchecked(future))
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl<Fut> ExactSizeIterator for IterPinMut<'_, Fut> {}

impl<'a, Fut: Unpin> Iterator for IterMut<'a, Fut> {
    type Item = &'a mut Fut;

    fn next(&mut self) -> Option<&'a mut Fut> {
        self.0.next().map(Pin::get_mut)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<Fut: Unpin> ExactSizeIterator for IterMut<'_, Fut> {}

impl<'a, Fut> Iterator for IterPinRef<'a, Fut> {
    type Item = Pin<&'a Fut>;

    fn next(&mut self) -> Option<Pin<&'a Fut>> {
        if self.task.is_null() {
            return None;
        }
        unsafe {
            let future = (*(*self.task).future.get()).as_ref().unwrap();

            // Relaxed ordering can be used since acquire ordering when
            // `head_all` was initially read for this iterator implies acquire
            // ordering for all previously inserted nodes (and we don't need to
            // read `len_all` again for any other nodes).
            let next = (*self.task).spin_next_all(
                self.pending_next_all,
                Relaxed,
            );
            self.task = next;
            self.len -= 1;
            Some(Pin::new_unchecked(future))
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl<Fut> ExactSizeIterator for IterPinRef<'_, Fut> {}

impl<'a, Fut: Unpin> Iterator for Iter<'a, Fut> {
    type Item = &'a Fut;

    fn next(&mut self) -> Option<&'a Fut> {
        self.0.next().map(Pin::get_ref)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<Fut: Unpin> ExactSizeIterator for Iter<'_, Fut> {}
