# Changelog

## 1.7.2

- Improve code size when using parking_lot feature.

## 1.7.1

- Fix `race::OnceBox<T>` to also impl `Default` even if `T` doesn't impl `Default`.

## 1.7.0

- Hide the `race` module behind (default) `race` feature.
  Turns out that adding `race` by default was a breaking change on some platforms without atomics.
  In this release, we make the module opt-out.
  Technically, this is a breaking change for those who use `race` with `no_default_features`.
  Given that the `race` module itself only several days old, the breakage is deemed acceptable.

## 1.6.0

- Add `Lazy::into_value`
- Stabilize `once_cell::race` module for "first one wins" no_std-compatible initialization flavor.
- Migrate from deprecated `compare_and_swap` to `compare_exchange`.

## 1.5.2

- `OnceBox` API uses `Box<T>`.
  This a breaking change to unstable API.

## 1.5.1

- MSRV is increased to `1.36.0`.
- document `once_cell::race` module.
- introduce `alloc` feature for `OnceBox`.
- fix `OnceBox::set`.

## 1.5.0

- add new `once_cell::race` module for "first one wins" no_std-compatible initialization flavor.
  The API is provisional, subject to change and is gated by the `unstable` cargo feature.

## 1.4.1

- upgrade `parking_lot` to `0.11.0`
- make `sync::OnceCell<T>` pass https://doc.rust-lang.org/nomicon/dropck.html#an-escape-hatch[dropck] with `parking_lot` feature enabled.
  This fixes a (minor) semver-incompatible changed introduced in `1.4.0`

## 1.4.0

- upgrade `parking_lot` to `0.10` (note that this bumps MSRV with `parking_lot` feature enabled to `1.36.0`).
- add `OnceCell::take`.
- upgrade crossbeam utils (private dependency) to `0.7`.

## 1.3.1

- remove unnecessary `F: fmt::Debug` bound from `impl fmt::Debug for Lazy<T, F>`.

## 1.3.0

- `Lazy<T>` now implements `DerefMut`.
- update implementation according to the latest changes in `std`.

## 1.2.0

- add `sync::OnceCell::get_unchecked`.

## 1.1.0

- implement `Default` for `Lazy`: it creates an empty `Lazy<T>` which is initialized with `T::default` on first access.
- add `OnceCell::get_mut`.

## 1.0.2

- actually add `#![no_std]` attribute if std feature is not enabled.

## 1.0.1

- fix unsoundness in `Lazy<T>` if the initializing function panics. Thanks [@xfix](https://github.com/xfix)!
- implement `RefUnwindSafe` for `Lazy`.
- share more code between `std` and `parking_lot` implementations.
- add F.A.Q section to the docs.

## 1.0.0

- remove `parking_lot` from the list of default features.
- add `std` default feature. Without `std`, only `unsync` module is supported.
- implement `Eq` for `OnceCell`.
- fix wrong `Sync` bound on `sync::Lazy`.
- run the whole test suite with miri.

## 0.2.7

- New implementation of `sync::OnceCell` if `parking_lot` feature is disabled.
  It now employs a hand-rolled variant of `std::sync::Once`.
- `sync::OnceCell::get_or_try_init` works without `parking_lot` as well!
- document the effects of `parking_lot` feature: same performance but smaller types.

## 0.2.6

- Updated `Lazy`'s `Deref` impl to requires only `FnOnce` instead of `Fn`

## 0.2.5

- `Lazy` requires only `FnOnce` instead of `Fn`

## 0.2.4

- nicer `fmt::Debug` implementation

## 0.2.3

- update `parking_lot` to `0.9.0`
- fix stacked borrows violation in `unsync::OnceCell::get`
- implement `Clone` for `sync::OnceCell<T> where T: Clone`

## 0.2.2

- add `OnceCell::into_inner` which consumes a cell and returns an option

## 0.2.1

- implement `sync::OnceCell::get_or_try_init` if `parking_lot` feature is enabled
- switch internal `unsafe` implementation of `sync::OnceCell` from `Once` to `Mutex`
- `sync::OnceCell::get_or_init` is twice as fast if cell is already initialized
- implement `std::panic::RefUnwindSafe` and `std::panic::UnwindSafe` for `OnceCell`
- better document behavior around panics

## 0.2.0

- MSRV is now 1.31.1
- `Lazy::new` and `OnceCell::new` are now const-fns
- `unsync_lazy` and `sync_lazy` macros are removed

## 0.1.8

- update crossbeam-utils to 0.6
- enable bors-ng

## 0.1.7

- cells implement `PartialEq` and `From`
- MSRV is down to 1.24.1
- update `parking_lot` to `0.7.1`

## 0.1.6

- `unsync::OnceCell<T>` is `Clone` if `T` is `Clone`.

## 0.1.5

- No changelog until this point :(
