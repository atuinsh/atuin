// There's a lot of scary concurrent code in this module, but it is copied from
// `std::sync::Once` with two changes:
//   * no poisoning
//   * init function can fail

use std::{
    cell::{Cell, UnsafeCell},
    hint::unreachable_unchecked,
    marker::PhantomData,
    panic::{RefUnwindSafe, UnwindSafe},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread::{self, Thread},
};

use crate::take_unchecked;

#[derive(Debug)]
pub(crate) struct OnceCell<T> {
    // This `state` word is actually an encoded version of just a pointer to a
    // `Waiter`, so we add the `PhantomData` appropriately.
    state_and_queue: AtomicUsize,
    _marker: PhantomData<*mut Waiter>,
    value: UnsafeCell<Option<T>>,
}

// Why do we need `T: Send`?
// Thread A creates a `OnceCell` and shares it with
// scoped thread B, which fills the cell, which is
// then destroyed by A. That is, destructor observes
// a sent value.
unsafe impl<T: Sync + Send> Sync for OnceCell<T> {}
unsafe impl<T: Send> Send for OnceCell<T> {}

impl<T: RefUnwindSafe + UnwindSafe> RefUnwindSafe for OnceCell<T> {}
impl<T: UnwindSafe> UnwindSafe for OnceCell<T> {}

// Three states that a OnceCell can be in, encoded into the lower bits of `state` in
// the OnceCell structure.
const INCOMPLETE: usize = 0x0;
const RUNNING: usize = 0x1;
const COMPLETE: usize = 0x2;

// Mask to learn about the state. All other bits are the queue of waiters if
// this is in the RUNNING state.
const STATE_MASK: usize = 0x3;

// Representation of a node in the linked list of waiters in the RUNNING state.
#[repr(align(4))] // Ensure the two lower bits are free to use as state bits.
struct Waiter {
    thread: Cell<Option<Thread>>,
    signaled: AtomicBool,
    next: *const Waiter,
}

// Head of a linked list of waiters.
// Every node is a struct on the stack of a waiting thread.
// Will wake up the waiters when it gets dropped, i.e. also on panic.
struct WaiterQueue<'a> {
    state_and_queue: &'a AtomicUsize,
    set_state_on_drop_to: usize,
}

impl<T> OnceCell<T> {
    pub(crate) const fn new() -> OnceCell<T> {
        OnceCell {
            state_and_queue: AtomicUsize::new(INCOMPLETE),
            _marker: PhantomData,
            value: UnsafeCell::new(None),
        }
    }

    /// Safety: synchronizes with store to value via Release/(Acquire|SeqCst).
    #[inline]
    pub(crate) fn is_initialized(&self) -> bool {
        // An `Acquire` load is enough because that makes all the initialization
        // operations visible to us, and, this being a fast path, weaker
        // ordering helps with performance. This `Acquire` synchronizes with
        // `SeqCst` operations on the slow path.
        self.state_and_queue.load(Ordering::Acquire) == COMPLETE
    }

    /// Safety: synchronizes with store to value via SeqCst read from state,
    /// writes value only once because we never get to INCOMPLETE state after a
    /// successful write.
    #[cold]
    pub(crate) fn initialize<F, E>(&self, f: F) -> Result<(), E>
    where
        F: FnOnce() -> Result<T, E>,
    {
        let mut f = Some(f);
        let mut res: Result<(), E> = Ok(());
        let slot: *mut Option<T> = self.value.get();
        initialize_inner(&self.state_and_queue, &mut || {
            let f = unsafe { take_unchecked(&mut f) };
            match f() {
                Ok(value) => {
                    unsafe { *slot = Some(value) };
                    true
                }
                Err(err) => {
                    res = Err(err);
                    false
                }
            }
        });
        res
    }

    /// Get the reference to the underlying value, without checking if the cell
    /// is initialized.
    ///
    /// # Safety
    ///
    /// Caller must ensure that the cell is in initialized state, and that
    /// the contents are acquired by (synchronized to) this thread.
    pub(crate) unsafe fn get_unchecked(&self) -> &T {
        debug_assert!(self.is_initialized());
        let slot: &Option<T> = &*self.value.get();
        match slot {
            Some(value) => value,
            // This unsafe does improve performance, see `examples/bench`.
            None => {
                debug_assert!(false);
                unreachable_unchecked()
            }
        }
    }

    /// Gets the mutable reference to the underlying value.
    /// Returns `None` if the cell is empty.
    pub(crate) fn get_mut(&mut self) -> Option<&mut T> {
        // Safe b/c we have a unique access.
        unsafe { &mut *self.value.get() }.as_mut()
    }

    /// Consumes this `OnceCell`, returning the wrapped value.
    /// Returns `None` if the cell was empty.
    #[inline]
    pub(crate) fn into_inner(self) -> Option<T> {
        // Because `into_inner` takes `self` by value, the compiler statically
        // verifies that it is not currently borrowed.
        // So, it is safe to move out `Option<T>`.
        self.value.into_inner()
    }
}

// Corresponds to `std::sync::Once::call_inner`
// Note: this is intentionally monomorphic
#[inline(never)]
fn initialize_inner(my_state_and_queue: &AtomicUsize, init: &mut dyn FnMut() -> bool) -> bool {
    let mut state_and_queue = my_state_and_queue.load(Ordering::Acquire);

    loop {
        match state_and_queue {
            COMPLETE => return true,
            INCOMPLETE => {
                let exchange = my_state_and_queue.compare_exchange(
                    state_and_queue,
                    RUNNING,
                    Ordering::Acquire,
                    Ordering::Acquire,
                );
                if let Err(old) = exchange {
                    state_and_queue = old;
                    continue;
                }
                let mut waiter_queue = WaiterQueue {
                    state_and_queue: my_state_and_queue,
                    set_state_on_drop_to: INCOMPLETE, // Difference, std uses `POISONED`
                };
                let success = init();

                // Difference, std always uses `COMPLETE`
                waiter_queue.set_state_on_drop_to = if success { COMPLETE } else { INCOMPLETE };
                return success;
            }
            _ => {
                assert!(state_and_queue & STATE_MASK == RUNNING);
                wait(&my_state_and_queue, state_and_queue);
                state_and_queue = my_state_and_queue.load(Ordering::Acquire);
            }
        }
    }
}

// Copy-pasted from std exactly.
fn wait(state_and_queue: &AtomicUsize, mut current_state: usize) {
    loop {
        if current_state & STATE_MASK != RUNNING {
            return;
        }

        let node = Waiter {
            thread: Cell::new(Some(thread::current())),
            signaled: AtomicBool::new(false),
            next: (current_state & !STATE_MASK) as *const Waiter,
        };
        let me = &node as *const Waiter as usize;

        let exchange = state_and_queue.compare_exchange(
            current_state,
            me | RUNNING,
            Ordering::Release,
            Ordering::Relaxed,
        );
        if let Err(old) = exchange {
            current_state = old;
            continue;
        }

        while !node.signaled.load(Ordering::Acquire) {
            thread::park();
        }
        break;
    }
}

// Copy-pasted from std exactly.
impl Drop for WaiterQueue<'_> {
    fn drop(&mut self) {
        let state_and_queue =
            self.state_and_queue.swap(self.set_state_on_drop_to, Ordering::AcqRel);

        assert_eq!(state_and_queue & STATE_MASK, RUNNING);

        unsafe {
            let mut queue = (state_and_queue & !STATE_MASK) as *const Waiter;
            while !queue.is_null() {
                let next = (*queue).next;
                let thread = (*queue).thread.replace(None).unwrap();
                (*queue).signaled.store(true, Ordering::Release);
                queue = next;
                thread.unpark();
            }
        }
    }
}

// These test are snatched from std as well.
#[cfg(test)]
mod tests {
    use std::panic;
    use std::{sync::mpsc::channel, thread};

    use super::OnceCell;

    impl<T> OnceCell<T> {
        fn init(&self, f: impl FnOnce() -> T) {
            enum Void {}
            let _ = self.initialize(|| Ok::<T, Void>(f()));
        }
    }

    #[test]
    fn smoke_once() {
        static O: OnceCell<()> = OnceCell::new();
        let mut a = 0;
        O.init(|| a += 1);
        assert_eq!(a, 1);
        O.init(|| a += 1);
        assert_eq!(a, 1);
    }

    #[test]
    #[cfg(not(miri))]
    fn stampede_once() {
        static O: OnceCell<()> = OnceCell::new();
        static mut RUN: bool = false;

        let (tx, rx) = channel();
        for _ in 0..10 {
            let tx = tx.clone();
            thread::spawn(move || {
                for _ in 0..4 {
                    thread::yield_now()
                }
                unsafe {
                    O.init(|| {
                        assert!(!RUN);
                        RUN = true;
                    });
                    assert!(RUN);
                }
                tx.send(()).unwrap();
            });
        }

        unsafe {
            O.init(|| {
                assert!(!RUN);
                RUN = true;
            });
            assert!(RUN);
        }

        for _ in 0..10 {
            rx.recv().unwrap();
        }
    }

    #[test]
    fn poison_bad() {
        static O: OnceCell<()> = OnceCell::new();

        // poison the once
        let t = panic::catch_unwind(|| {
            O.init(|| panic!());
        });
        assert!(t.is_err());

        // we can subvert poisoning, however
        let mut called = false;
        O.init(|| {
            called = true;
        });
        assert!(called);

        // once any success happens, we stop propagating the poison
        O.init(|| {});
    }

    #[test]
    fn wait_for_force_to_finish() {
        static O: OnceCell<()> = OnceCell::new();

        // poison the once
        let t = panic::catch_unwind(|| {
            O.init(|| panic!());
        });
        assert!(t.is_err());

        // make sure someone's waiting inside the once via a force
        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();
        let t1 = thread::spawn(move || {
            O.init(|| {
                tx1.send(()).unwrap();
                rx2.recv().unwrap();
            });
        });

        rx1.recv().unwrap();

        // put another waiter on the once
        let t2 = thread::spawn(|| {
            let mut called = false;
            O.init(|| {
                called = true;
            });
            assert!(!called);
        });

        tx2.send(()).unwrap();

        assert!(t1.join().is_ok());
        assert!(t2.join().is_ok());
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn test_size() {
        use std::mem::size_of;

        assert_eq!(size_of::<OnceCell<u32>>(), 4 * size_of::<u32>());
    }
}
