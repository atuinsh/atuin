## parking_lot 0.11.1, lock_api 0.4.2 (2020-11-18)

- Fix bounds on Send and Sync impls for lock guards. (#262)
- Fix incorrect memory ordering in `RwLock`. (#260)

## lock_api 0.4.1 (2020-07-06)

- Add `data_ptr` method to lock types to allow unsafely accessing the inner data
  without a guard. (#247)

## parking_lot 0.11.0, parking_lot_core 0.8.0, lock_api 0.4.0 (2020-06-23)

- Add `is_locked` method to mutex types. (#235)
- Make `RawReentrantMutex` public. (#233)
- Allow lock guard to be sent to another thread with the `send_guard` feature. (#240)
- Use `Instant` type from the `instant` crate on wasm32-unknown-unknown. (#231)
- Remove deprecated and unsound `MappedRwLockWriteGuard::downgrade`. (#244)
- Most methods on the `Raw*` traits have been made unsafe since they assume
  the current thread holds the lock. (#243)

## parking_lot_core 0.7.2 (2020-04-21)

- Add support for `wasm32-unknown-unknown` under the "nightly" feature. (#226)

## parking_lot 0.10.2 (2020-04-10)

- Update minimum version of `lock_api`.

## parking_lot 0.10.1, parking_lot_core 0.7.1, lock_api 0.3.4 (2020-04-10)

- Add methods to construct `Mutex`, `RwLock`, etc in a `const` context. (#217)
- Add `FairMutex` which always uses fair unlocking. (#204)
- Fixed panic with deadlock detection on macOS. (#203)
- Fixed incorrect synchronization in `create_hashtable`. (#210)
- Use `llvm_asm!` instead of the deprecated `asm!`. (#223)

## lock_api 0.3.3 (2020-01-04)

- Deprecate unsound `MappedRwLockWriteGuard::downgrade` (#198)

## parking_lot 0.10.0, parking_lot_core 0.7.0, lock_api 0.3.2 (2019-11-25)

- Upgrade smallvec dependency to 1.0 in parking_lot_core.
- Replace all usage of `mem::uninitialized` with `mem::MaybeUninit`.
- The minimum required Rust version is bumped to 1.36. Because of the above two changes.
- Make methods on `WaitTimeoutResult` and `OnceState` take `self` by value instead of reference.

## parking_lot_core 0.6.2 (2019-07-22)

- Fixed compile error on Windows with old cfg_if version. (#164)

## parking_lot_core 0.6.1 (2019-07-17)

- Fixed Android build. (#163)

## parking_lot 0.9.0, parking_lot_core 0.6.0, lock_api 0.3.1 (2019-07-14)

- Re-export lock_api (0.3.1) from parking_lot (#150)
- Removed (non-dev) dependency on rand crate for fairness mechanism, by
  including a simple xorshift PRNG in core (#144)
- Android now uses the futex-based ThreadParker. (#140)
- Fixed CloudABI ThreadParker. (#140)
- Fix race condition in lock_api::ReentrantMutex (da16c2c7)

## lock_api 0.3.0 (2019-07-03, _yanked_)

- Use NonZeroUsize in GetThreadId::nonzero_thread_id (#148)
- Debug assert lock_count in ReentrantMutex (#148)
- Tag as `unsafe` and document some internal methods (#148)
- This release was _yanked_ due to a regression in ReentrantMutex (da16c2c7)

## parking_lot 0.8.1 (2019-07-03, _yanked_)

- Re-export lock_api (0.3.0) from parking_lot (#150)
- This release was _yanked_ from crates.io due to unexpected breakage (#156)

## parking_lot 0.8.0, parking_lot_core 0.5.0, lock_api 0.2.0 (2019-05-04)

- Fix race conditions in deadlock detection.
- Support for more platforms by adding ThreadParker implementations for
  Wasm, Redox, SGX and CloudABI.
- Drop support for older Rust. parking_lot now requires 1.31 and is a
  Rust 2018 edition crate (#122).
- Disable the owning_ref feature by default.
- Fix was_last_thread value in the timeout callback of park() (#129).
- Support single byte Mutex/Once on stable Rust when compiler is at least
  version 1.34.
- Make Condvar::new and Once::new const fns on stable Rust and remove
  ONCE_INIT (#134).
- Add optional Serde support (#135).

## parking_lot 0.7.1 (2019-01-01)

- Fixed potential deadlock when upgrading a RwLock.
- Fixed overflow panic on very long timeouts (#111).

## parking_lot 0.7.0, parking_lot_core 0.4.0 (2018-11-26)

- Return if or how many threads were notified from `Condvar::notify_*`

## parking_lot 0.6.3 (2018-07-18)

- Export `RawMutex`, `RawRwLock` and `RawThreadId`.

## parking_lot 0.6.2 (2018-06-18)

- Enable `lock_api/nightly` feature from `parking_lot/nightly` (#79)

## parking_lot 0.6.1 (2018-06-08)

Added missing typedefs for mapped lock guards:

- `MappedMutexGuard`
- `MappedReentrantMutexGuard`
- `MappedRwLockReadGuard`
- `MappedRwLockWriteGuard`

## parking_lot 0.6.0 (2018-06-08)

This release moves most of the code for type-safe `Mutex` and `RwLock` types
into a separate crate called `lock_api`. This new crate is compatible with
`no_std` and provides `Mutex` and `RwLock` type-safe wrapper types from a raw
mutex type which implements the `RawMutex` or `RawRwLock` trait. The API
provided by the wrapper types can be extended by implementing more traits on
the raw mutex type which provide more functionality (e.g. `RawMutexTimed`). See
the crate documentation for more details.

There are also several major changes:

- The minimum required Rust version is bumped to 1.26.
- All methods on `MutexGuard` (and other guard types) are no longer inherent
  methods and must be called as `MutexGuard::method(self)`. This avoids
  conflicts with methods from the inner type.
- `MutexGuard` (and other guard types) add the `unlocked` method which
  temporarily unlocks a mutex, runs the given closure, and then re-locks the
   mutex.
- `MutexGuard` (and other guard types) add the `bump` method which gives a
  chance for other threads to acquire the mutex by temporarily unlocking it and
  re-locking it. However this is optimized for the common case where there are
  no threads waiting on the lock, in which case no unlocking is performed.
- `MutexGuard` (and other guard types) add the `map` method which returns a
  `MappedMutexGuard` which holds only a subset of the original locked type. The
  `MappedMutexGuard` type is identical to `MutexGuard` except that it does not
  support the `unlocked` and `bump` methods, and can't be used with `CondVar`.
