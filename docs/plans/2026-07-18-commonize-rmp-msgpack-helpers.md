# Commonize MessagePack (`rmp`) Helpers into `atuin-common` Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace `atuin-client`'s private `utils/rmp.rs` with a rewritten, extension-trait-based `atuin_common::rmp` module (feature-gated behind `rmp`), give it rich rstest/proptest coverage and good docs, and migrate every caller onto it — deleting six copies of hand-rolled `error_report`/trailing-bytes/array-length boilerplate.

**Architecture:** Two extension traits — `RmpDecodeExt<'a>` implemented for `rmp::decode::Bytes<'a>` (the one decode cursor used everywhere) and `RmpEncodeExt` blanket-implemented for every `RmpWrite` — plus two error types `DecodeError`/`EncodeError` that carry legible `Display` messages and convert into `eyre::Report`. All decode reads return `DecodeError` so callers use `?` throughout instead of `.map_err(error_report)` at every read. The module lives in `atuin-common` (a leaf crate) so `atuin-client`, `atuin-scripts`, `atuin-dotfiles`, and `atuin-kv` can all share it with no dependency cycle.

**Tech Stack:** Rust 2024, `rmp = 0.8.14`, `derive_more = 2` (`display`, `from`, `error`), `thiserror`, `eyre`; tests use `rstest`, `proptest`, `pretty_assertions` (already `atuin-common` dev-deps).

---

## Background & context (read before starting)

**What exists today.** `crates/atuin-client/src/utils/rmp.rs` (123 lines, declared `pub(crate) mod rmp;` in `utils.rs:1`) provides `EncodeError`, `DecodeError`, `read_string`, `read_optional`, `write_optional`. Its **only** real consumer is `crates/atuin-client/src/history.rs`.

**The duplication we are killing.** Six other files never import that module — they call raw `rmp` and each re-declare an identical local helper:
```rust
fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report { eyre!("{err:?}") }
```
Copies live in: `atuin-scripts/src/store/record.rs:52`, `atuin-client/src/history/store.rs:76`, `atuin-dotfiles/src/shell.rs:45`, `atuin-dotfiles/src/store.rs:58`, `atuin-dotfiles/src/store/var.rs:52`, `atuin-kv/src/store/record.rs:39`. (`encryption.rs` inlines the same as `.map_err(|err| eyre!("{err:?}"))`.) Alongside it, the same two idioms recur ~7× each:
- **trailing-bytes guard:** `if !bytes.is_empty() { bail!("trailing bytes ... malformed") }`
- **array-length assertion:** `let n = read_array_len(..)?; ensure!(n == N, "too many entries ...")`

`DecodeError` already converts to `eyre::Report`, so the shared fix for `error_report` is simply "make decode reads return `DecodeError` and use `?`". `expect_array_len`/`expect_eof` absorb the other two idioms.

**Concrete usage the interface must serve** (verified against source):
- Decode cursor is `rmp::decode::Bytes<'a>` in every file. (One kv site at `atuin-kv/src/store/record.rs:78` feeds a raw `&mut &[u8]` to `read_bool`; we normalize it to `Bytes` during migration.)
- Encode writer is `&mut Vec<u8>` everywhere. `Vec<u8>: RmpWrite<Error = std::io::Error>` (proven: `history.rs:217` returns `Result<_, EncodeError>` — default `EncodeError<std::io::Error>` — while `?`-converting `write_optional`'s error).
- `history.rs:256-260` needs a **range** field-count check (version-dependent min/max), not exact — so it keeps its own logic via `read_array_len()`; `expect_array_len` is exact-only by design (YAGNI).
- `history.rs:274,284,290` passes `read_string` as a bare fn into `read_optional`. As a method, this becomes the closure `|b| b.read_string()`.
- `write_optional` is called with `encode::write_u64` and `encode::write_str` as the writer fn (`history.rs:233-240`).

**Naming.** Match the existing high-quality modules (`EllipsizeExt`, `EscapeNonPrintablePosixExt`, `UrlAppendExt`): traits are `RmpDecodeExt` / `RmpEncodeExt`, `...Ext` suffix, blanket/receiver impls, doc-comment-per-public-item but no filler docs.

**Module-vs-crate name.** Inside `atuin-common`, the module is `crate::rmp` and the crate is `rmp`. A leading `rmp::` path resolves to the extern crate (extern prelude), so `use rmp::decode` inside the module file is unambiguous. Consumers must `use atuin_common::rmp::RmpDecodeExt;` (import the *items*, never `use atuin_common::rmp;` as a whole, which would shadow the crate).

**Staging.** Stage A (Tasks 1–8) rewrites + moves the module and migrates its one real consumer (`history.rs`), then deletes the old file — this alone satisfies "completely rewrite and move it," and the tree is green at the end of Task 8. Stage B (Tasks 9–13) is the commonization payoff: migrate the six raw-`rmp` files off their local `error_report`/idioms. Stage B is independently valuable and can be deferred, but it is why the module is worth commonizing.

**Reference — full target module.** The complete intended contents of `crates/atuin-common/src/rmp.rs` (module code, not tests) are in **Appendix A** at the end of this document. Tasks 2–5 build it up test-first; use Appendix A as the definitive source for signatures, bounds, and doc comments.

---

## Stage A — rewrite, move, and land the module

### Task 1: Add the `rmp` feature, dependency, and empty module

**Files:**
- Modify: `crates/atuin-common/Cargo.toml`
- Modify: `crates/atuin-common/src/lib.rs:60-63` (the `pub mod ...;` block)
- Create: `crates/atuin-common/src/rmp.rs`

**Step 1: Add the feature and optional dependency**

In `crates/atuin-common/Cargo.toml`, under `[features]` add (after the existing `unicode = [...]` line):
```toml
# The `rmp` module: MessagePack encode/decode extension traits and error types.
rmp = ["dep:rmp"]
```
Under `[dependencies]` add (matching the `0.8.14` pin used by the other crates — there is no workspace `rmp` entry):
```toml
rmp = { version = "0.8.14", optional = true }
```

**Step 2: Create a stub module**

`crates/atuin-common/src/rmp.rs`:
```rust
//! MessagePack encode/decode helpers built on [`rmp`].
```

**Step 3: Wire it into the crate root**

In `crates/atuin-common/src/lib.rs`, add alongside the other `pub mod` declarations:
```rust
#[cfg(feature = "rmp")]
pub mod rmp;
```

**Step 4: Verify it builds both ways**

Run: `cargo build -p atuin-common --features rmp`
Expected: PASS.
Run: `cargo build -p atuin-common`
Expected: PASS (module absent, `rmp` crate not compiled).

**Step 5: Commit**
```bash
git add crates/atuin-common/Cargo.toml crates/atuin-common/src/lib.rs crates/atuin-common/src/rmp.rs
git commit -m "feat(common): add feature-gated rmp module scaffold"
```

---

### Task 2: Port the error types (`EncodeError`, `DecodeError`)

**Files:**
- Modify: `crates/atuin-common/src/rmp.rs`

**Step 1: Write failing tests**

Append to `crates/atuin-common/src/rmp.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rmp::decode::bytes::Bytes;
    use rstest::rstest;

    // Build a real DecodeError by asking rmp to read the wrong type.
    fn type_mismatch_error<'a>() -> DecodeError<'a> {
        // 0xc0 is the nil marker; reading it as a u64 is a type mismatch.
        let mut bytes = Bytes::new(&[0xc0]);
        rmp::decode::read_u64(&mut bytes)
            .map_err(DecodeError::from)
            .unwrap_err()
    }

    #[test]
    fn type_mismatch_exposes_marker() {
        assert_eq!(type_mismatch_error().type_mismatch(), Some(rmp::Marker::Null));
    }

    #[rstest]
    #[case::array_len(
        DecodeError::<'static>::UnexpectedArrayLen { expected: 3, actual: 5 },
        None
    )]
    #[case::trailing(DecodeError::<'static>::TrailingBytes { remaining: 2 }, None)]
    fn structural_variants_have_no_marker(
        #[case] err: DecodeError<'static>,
        #[case] expected: Option<rmp::Marker>,
    ) {
        assert_eq!(err.type_mismatch(), expected);
    }

    #[test]
    fn decode_error_converts_to_eyre_with_message() {
        let report: eyre::Report =
            DecodeError::<'static>::TrailingBytes { remaining: 4 }.into();
        assert!(report.to_string().contains("trailing"));
    }

    #[test]
    fn display_messages_are_legible() {
        assert_eq!(
            DecodeError::<'static>::UnexpectedArrayLen { expected: 3, actual: 5 }.to_string(),
            "expected a MessagePack array of length 3, found 5",
        );
    }
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p atuin-common --features rmp rmp::tests`
Expected: FAIL to compile (`DecodeError`/`EncodeError` not defined).

**Step 3: Add the imports and error types**

Insert the imports block and both error types from **Appendix A** (everything from the `use` lines through the `impl ... From<DecodeError...> for eyre::Report` block) above the `#[cfg(test)]` module.

**Step 4: Run to verify pass**

Run: `cargo test -p atuin-common --features rmp rmp::tests`
Expected: PASS.
Run: `cargo clippy -p atuin-common --features rmp -- -D warnings`
Expected: PASS.

**Step 5: Commit**
```bash
git add crates/atuin-common/src/rmp.rs
git commit -m "feat(common): add rmp EncodeError/DecodeError types"
```

---

### Task 3: `RmpDecodeExt` core reads — `read_with`, `read_string`, `read_optional`

**Files:**
- Modify: `crates/atuin-common/src/rmp.rs`

**Step 1: Write failing tests**

Add to the `tests` module:
```rust
    // Encode helpers used only by tests, to build inputs.
    fn enc<F: FnOnce(&mut Vec<u8>)>(f: F) -> Vec<u8> {
        let mut v = Vec::new();
        f(&mut v);
        v
    }

    #[test]
    fn read_string_round_trips() {
        let buf = enc(|v| rmp::encode::write_str(v, "héllo 🦀").unwrap());
        let mut bytes = Bytes::new(&buf);
        assert_eq!(bytes.read_string().unwrap(), "héllo 🦀");
        assert!(bytes.remaining_slice().is_empty());
    }

    #[test]
    fn read_string_on_wrong_type_errors_and_consumes_marker() {
        // A lone nil marker: read_string must fail *and* consume the marker so a
        // following read_optional can observe end-of-input correctly.
        let mut bytes = Bytes::new(&[0xc0]);
        assert!(bytes.read_string().is_err());
        assert!(bytes.remaining_slice().is_empty(), "marker byte must be consumed");
    }

    #[test]
    fn read_with_converts_rmp_errors() {
        let buf = enc(|v| rmp::encode::write_u64(v, 42).unwrap());
        let mut bytes = Bytes::new(&buf);
        assert_eq!(bytes.read_with(rmp::decode::read_u64).unwrap(), 42);
    }

    #[test]
    fn read_optional_some_and_none() {
        let some = enc(|v| rmp::encode::write_u64(v, 7).unwrap());
        let mut b = Bytes::new(&some);
        assert_eq!(b.read_optional(rmp::decode::read_u64).unwrap(), Some(7));

        let none = enc(|v| rmp::encode::write_nil(v).unwrap());
        let mut b = Bytes::new(&none);
        assert_eq!(b.read_optional(rmp::decode::read_u64).unwrap(), None);
        assert!(b.remaining_slice().is_empty());
    }

    #[test]
    fn read_optional_string_via_closure() {
        let buf = enc(|v| rmp::encode::write_str(v, "x").unwrap());
        let mut b = Bytes::new(&buf);
        assert_eq!(b.read_optional(|b| b.read_string()).unwrap(), Some("x".to_string()));
    }

    #[test]
    fn read_optional_forwards_non_nil_errors() {
        // A bool where a u64 is expected is a type mismatch that is NOT nil.
        let buf = enc(|v| rmp::encode::write_bool(v, true).unwrap());
        let mut b = Bytes::new(&buf);
        assert!(b.read_optional(rmp::decode::read_u64).is_err());
    }
```

**Step 2: Run to verify failure**

Run: `cargo test -p atuin-common --features rmp rmp::tests`
Expected: FAIL (`RmpDecodeExt` / methods not defined).

**Step 3: Implement the trait + these three methods**

Add the `RmpDecodeExt<'a>` trait declaration and the `impl<'a> RmpDecodeExt<'a> for Bytes<'a>` block from **Appendix A**, but for this task include **only** `read_with`, `read_string`, and `read_optional` (leave `read_array_len`/`expect_array_len`/`expect_eof` for Task 4). Keep the trait declaration complete (all six method signatures) and add a `todo!()`-free partial impl — i.e. add the three later methods' signatures to the trait now but implement the remaining three in Task 4. To keep the crate compiling between tasks, add all six method bodies now using Appendix A (implementing all six here is fine; Task 4 just adds their tests). **Recommended:** implement all six method bodies in this task, and add the validation *tests* in Task 4.

**Step 4: Run to verify pass**

Run: `cargo test -p atuin-common --features rmp rmp::tests`
Expected: PASS.
Run: `cargo clippy -p atuin-common --features rmp -- -D warnings`
Expected: PASS.

**Step 5: Commit**
```bash
git add crates/atuin-common/src/rmp.rs
git commit -m "feat(common): add RmpDecodeExt read_with/read_string/read_optional"
```

---

### Task 4: `RmpDecodeExt` validation — `read_array_len`, `expect_array_len`, `expect_eof`

**Files:**
- Modify: `crates/atuin-common/src/rmp.rs`

**Step 1: Write failing tests**

Add to the `tests` module:
```rust
    fn array_of(len: u32) -> Vec<u8> {
        enc(|v| rmp::encode::write_array_len(v, len).unwrap())
    }

    #[test]
    fn expect_array_len_exact_ok() {
        let buf = array_of(3);
        let mut b = Bytes::new(&buf);
        assert_eq!(b.expect_array_len(3).unwrap(), 3);
    }

    #[test]
    fn expect_array_len_mismatch_reports_expected_and_actual() {
        let buf = array_of(5);
        let mut b = Bytes::new(&buf);
        match b.expect_array_len(3) {
            Err(DecodeError::UnexpectedArrayLen { expected, actual }) => {
                assert_eq!((expected, actual), (3, 5));
            }
            other => panic!("expected UnexpectedArrayLen, got {other:?}"),
        }
    }

    #[test]
    fn read_array_len_returns_count_for_manual_range_checks() {
        let buf = array_of(9);
        let mut b = Bytes::new(&buf);
        assert_eq!(b.read_array_len().unwrap(), 9);
    }

    #[test]
    fn expect_eof_ok_when_consumed() {
        let buf = enc(|v| rmp::encode::write_u8(v, 1).unwrap());
        let mut b = Bytes::new(&buf);
        b.read_with(rmp::decode::read_u8).unwrap();
        assert!(b.expect_eof().is_ok());
    }

    #[test]
    fn expect_eof_reports_remaining() {
        let b = Bytes::new(&[0x01, 0x02, 0x03]);
        match b.expect_eof() {
            Err(DecodeError::TrailingBytes { remaining }) => assert_eq!(remaining, 3),
            other => panic!("expected TrailingBytes, got {other:?}"),
        }
    }
```

**Step 2: Run to verify failure**

Run: `cargo test -p atuin-common --features rmp rmp::tests`
Expected: PASS if Task 3 already implemented all six bodies (recommended path); otherwise FAIL until you add the three method bodies from Appendix A. If FAIL, add them now.

**Step 3: Ensure the three method bodies are present**

Confirm `read_array_len`, `expect_array_len`, `expect_eof` match Appendix A.

**Step 4: Run to verify pass**

Run: `cargo test -p atuin-common --features rmp rmp::tests`
Expected: PASS.
Run: `cargo clippy -p atuin-common --features rmp -- -D warnings`
Expected: PASS.

**Step 5: Commit**
```bash
git add crates/atuin-common/src/rmp.rs
git commit -m "feat(common): add RmpDecodeExt array-len and eof checks"
```

---

### Task 5: `RmpEncodeExt::write_optional`

**Files:**
- Modify: `crates/atuin-common/src/rmp.rs`

**Step 1: Write failing tests**

Add to the `tests` module:
```rust
    #[test]
    fn write_optional_some_then_read_back() {
        let mut out = Vec::new();
        out.write_optional(Some(99u64), rmp::encode::write_u64).unwrap();
        let mut b = Bytes::new(&out);
        assert_eq!(b.read_optional(rmp::decode::read_u64).unwrap(), Some(99));
    }

    #[test]
    fn write_optional_none_writes_nil() {
        let mut out = Vec::new();
        out.write_optional::<u64, _>(None, rmp::encode::write_u64).unwrap();
        let mut b = Bytes::new(&out);
        assert_eq!(b.read_optional(rmp::decode::read_u64).unwrap(), None);
    }

    #[test]
    fn write_optional_str() {
        let mut out = Vec::new();
        out.write_optional(Some("hi"), rmp::encode::write_str).unwrap();
        let mut b = Bytes::new(&out);
        assert_eq!(b.read_optional(|b| b.read_string()).unwrap(), Some("hi".to_string()));
    }
```

**Step 2: Run to verify failure**

Run: `cargo test -p atuin-common --features rmp rmp::tests`
Expected: FAIL (`write_optional` not defined).

**Step 3: Implement**

Add the `RmpEncodeExt` trait and its blanket `impl<W: RmpWrite>` from **Appendix A**.

**Step 4: Run to verify pass**

Run: `cargo test -p atuin-common --features rmp rmp::tests`
Expected: PASS.
Run: `cargo clippy -p atuin-common --features rmp -- -D warnings`
Expected: PASS.

**Step 5: Commit**
```bash
git add crates/atuin-common/src/rmp.rs
git commit -m "feat(common): add RmpEncodeExt write_optional"
```

---

### Task 6: Property tests (round-trips + never-panics)

**Files:**
- Modify: `crates/atuin-common/src/rmp.rs`

**Step 1: Write the proptests**

Add to the `tests` module:
```rust
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]

        // A string survives an encode -> read_string round trip exactly.
        #[test]
        fn string_round_trips(s in r"(?s).*") {
            let buf = enc(|v| rmp::encode::write_str(v, &s).unwrap());
            let mut b = Bytes::new(&buf);
            prop_assert_eq!(b.read_string().unwrap(), s);
            prop_assert!(b.remaining_slice().is_empty());
        }

        // Option<u64> survives write_optional -> read_optional.
        #[test]
        fn optional_u64_round_trips(v in proptest::option::of(any::<u64>())) {
            let mut out = Vec::new();
            out.write_optional(v, rmp::encode::write_u64).unwrap();
            let mut b = Bytes::new(&out);
            prop_assert_eq!(b.read_optional(rmp::decode::read_u64).unwrap(), v);
        }

        // Option<String> survives the closure-based optional round trip.
        #[test]
        fn optional_string_round_trips(v in proptest::option::of(r"(?s).*")) {
            let mut out = Vec::new();
            out.write_optional(v.as_deref(), rmp::encode::write_str).unwrap();
            let mut b = Bytes::new(&out);
            prop_assert_eq!(b.read_optional(|b| b.read_string()).unwrap(), v);
        }

        // A full array record (len + fields + optional tail) round trips, and the
        // cursor is exactly exhausted afterwards.
        #[test]
        fn record_round_trips(
            id in r"[a-z]{0,16}",
            ts in any::<u64>(),
            deleted in proptest::option::of(any::<u64>()),
        ) {
            let mut out = Vec::new();
            rmp::encode::write_array_len(&mut out, 3).unwrap();
            rmp::encode::write_str(&mut out, &id).unwrap();
            rmp::encode::write_u64(&mut out, ts).unwrap();
            out.write_optional(deleted, rmp::encode::write_u64).unwrap();

            let mut b = Bytes::new(&out);
            prop_assert_eq!(b.expect_array_len(3).unwrap(), 3);
            prop_assert_eq!(b.read_string().unwrap(), id);
            prop_assert_eq!(b.read_with(rmp::decode::read_u64).unwrap(), ts);
            prop_assert_eq!(b.read_optional(rmp::decode::read_u64).unwrap(), deleted);
            prop_assert!(b.expect_eof().is_ok());
        }

        // Reads never panic on arbitrary bytes — they return Err instead.
        #[test]
        fn reads_never_panic(raw in proptest::collection::vec(any::<u8>(), 0..64)) {
            let mut b = Bytes::new(&raw);
            let _ = b.read_string();
            let mut b = Bytes::new(&raw);
            let _ = b.read_array_len();
            let mut b = Bytes::new(&raw);
            let _ = b.read_optional(rmp::decode::read_u64);
        }
    }
```

**Step 2: Run**

Run: `cargo test -p atuin-common --features rmp rmp::tests`
Expected: PASS.

**Step 3: Commit**
```bash
git add crates/atuin-common/src/rmp.rs
git commit -m "test(common): property tests for rmp round-trips and panic-safety"
```

---

### Task 7: Migrate `history.rs` onto `atuin_common::rmp`

**Files:**
- Modify: `crates/atuin-client/Cargo.toml` (enable the `rmp` feature on `atuin-common`)
- Modify: `crates/atuin-client/src/history.rs:1-13` (imports) and `:217-315` (serialize/deserialize)

**Step 1: Enable the feature**

In `crates/atuin-client/Cargo.toml`, under `[dependencies.atuin-common]`, add a features line:
```toml
[dependencies.atuin-common]
path = "../atuin-common"
features = ["rmp"]
```
(Leave the `[dev-dependencies.atuin-common]` block as-is; cargo unifies features.)

**Step 2: Swap the import**

In `crates/atuin-client/src/history.rs`, replace:
```rust
use crate::utils::rmp::{DecodeError, EncodeError, read_optional, read_string, write_optional};
```
with:
```rust
use atuin_common::rmp::{DecodeError, EncodeError, RmpDecodeExt as _, RmpEncodeExt as _};
```
Keep the existing `use rmp::decode::{self, Bytes};` and `use rmp::encode;` lines — the raw crate is still used directly.

**Step 3: Rewrite `serialize` (lines ~217-242)**

Change the three `write_optional(&mut output, VALUE, WRITER)?` free-function calls to method calls:
```rust
        output.write_optional(
            self.deleted_at.map(|d| d.unix_timestamp_nanos() as u64),
            encode::write_u64,
        )?;
        encode::write_str(&mut output, self.author.as_str())?;
        output.write_optional(self.intent.as_deref(), encode::write_str)?;
        output.write_optional(self.shell.as_deref(), encode::write_str)?;
```

**Step 4: Rewrite `deserialize` (lines ~244-297)**

Replace free-function/`map_err(DecodeError::from)` calls with method calls. Note `read_string` becomes a method, so where it was passed as a bare fn to `read_optional`, use a closure:
```rust
        let real_version = bytes.read_with(decode::read_u16)?;
        // ... unchanged range check on real_version ...

        let nfields = bytes.read_array_len()?;
        // ... unchanged version-dependent min/max range check + bail! ...

        let id = bytes.read_string()?;
        let timestamp = bytes.read_with(decode::read_u64)?;
        let duration = bytes.read_with(decode::read_int)?;
        let exit = bytes.read_with(decode::read_int)?;

        let command = bytes.read_string()?;
        let cwd = bytes.read_string()?;
        let session = bytes.read_string()?;
        let hostname = bytes.read_string()?;
        let deleted_at = bytes.read_optional(decode::read_u64)?;

        let author = if version >= Version::One {
            bytes.read_optional(|b| b.read_string())?
        } else {
            None
        };
        // ... intent: same closure form ...
        // ... shell: same closure form ...

        if version < Version::Two && !bytes.remaining_slice().is_empty() {
            bail!("trailing bytes in encoded history. malformed")
        }
```
Keep the version-dependent trailing-bytes check as-is (it is conditional, not a plain `expect_eof`).

**Step 5: Verify**

Run: `cargo test -p atuin-client --lib history`
Expected: PASS (existing history serialize/deserialize round-trip tests still pass).
Run: `cargo clippy -p atuin-client -- -D warnings`
Expected: PASS.

**Step 6: Commit**
```bash
git add crates/atuin-client/Cargo.toml crates/atuin-client/src/history.rs
git commit -m "refactor(client): use atuin_common::rmp in history serde"
```

---

### Task 8: Delete the old module

**Files:**
- Delete: `crates/atuin-client/src/utils/rmp.rs`
- Modify: `crates/atuin-client/src/utils.rs:1` (remove `pub(crate) mod rmp;`)

**Step 1: Confirm no remaining references**

Run: `rg 'utils::rmp|utils/rmp' crates/`
Expected: no matches.

**Step 2: Delete + unwire**
```bash
git rm crates/atuin-client/src/utils/rmp.rs
```
Remove the `pub(crate) mod rmp;` line from `crates/atuin-client/src/utils.rs`. If that leaves `utils.rs` empty or `utils` unused, check `rg 'mod utils|utils::' crates/atuin-client/src` and clean up only if genuinely dead (do not remove other utils).

**Step 3: Verify**

Run: `cargo build -p atuin-client`
Expected: PASS.
Run: `cargo test -p atuin-client`
Expected: PASS.

**Step 4: Commit**
```bash
git add -A
git commit -m "refactor(client): delete now-unused utils/rmp module"
```

**Stage A is complete here. The tree is green and the module is moved.**

---

## Stage B — commonization payoff: migrate the raw-`rmp` callers

Each Stage B task follows the same shape: enable `features = ["rmp"]` on the crate's `atuin-common` dependency (skip if already set), delete the local `error_report` fn, and replace the idioms:

| Old idiom | New |
|---|---|
| `decode::read_x(&mut b).map_err(error_report)?` | `b.read_with(decode::read_x)?` |
| `let (s, rest) = read_str_from_slice(..).map_err(error_report)?; s.to_owned()` | `b.read_string()?` (owned) |
| `let n = read_array_len(..)?; ensure!(n == N, "...")` | `b.expect_array_len(N)?` |
| `if !bytes.is_empty() { bail!("trailing ...") }` | `b.expect_eof()?` (optionally `.wrap_err("<record>")`) |

Import `use atuin_common::rmp::{RmpDecodeExt as _, RmpEncodeExt as _};` (add `EncodeError`/`DecodeError` only where a type is named). Keep each file's `use rmp::{decode, encode};`. Normalize any `&mut &[u8]` reader to a `Bytes` cursor so the trait applies.

> Behavior note: `expect_eof`/`expect_array_len` produce generic-but-informative messages (they include the actual counts/bytes). This replaces record-specific strings like "too many entries in v0 kv record". Where the specific context matters for debugging, append `.wrap_err("v0 kv record")?`. Confirm each crate's existing (de)serialize round-trip tests still pass after migration — the wire format must not change.

### Task 9: `atuin-client` — `encryption.rs` and `history/store.rs`

**Files:**
- Modify: `crates/atuin-client/src/encryption.rs:71-107` (`decode_key`) — replace the inline `.map_err(|err| eyre!("{err:?}"))` closures with `b.read_with(...)`; replace `ensure!(len == 32, ...)` on `read_array_len` with `b.expect_array_len(32)?` (the `Bin8`/`read_bin_len` branch keeps its own `ensure!` since `read_bin_len` isn't an array). Feature already enabled in Task 7.
- Modify: `crates/atuin-client/src/history/store.rs:73-114` (`deserialize`) — delete `error_report` (lines 76-78); use `b.read_with(decode::read_u8)`, `b.read_with(decode::read_bin_len)`; in the `Delete` arm replace `read_str_from_slice(..).map_err(error_report)?` + trailing check with `let id = bytes.read_string()?; bytes.expect_eof()?;`.

**Step 1-2:** Make the edits above; keep the `Create` arm handing `bytes.remaining_slice()` to `History::deserialize` unchanged.

**Step 3: Verify**

Run: `cargo test -p atuin-client`
Expected: PASS.
Run: `cargo clippy -p atuin-client -- -D warnings`
Expected: PASS.

**Step 4: Commit**
```bash
git add crates/atuin-client/src/encryption.rs crates/atuin-client/src/history/store.rs
git commit -m "refactor(client): use atuin_common::rmp in encryption and history store"
```

---

### Task 10: `atuin-scripts` — `store/record.rs` and `store/script.rs`

**Files:**
- Modify: `crates/atuin-scripts/Cargo.toml` — add `features = ["rmp"]` to the `atuin-common` dependency.
- Modify: `crates/atuin-scripts/src/store/record.rs:49-91` — delete `error_report`; use `bytes.read_with(decode::read_u8)`, `bytes.read_with(decode::read_bin_len)`; in the delete arm, `bytes.read_string()?` instead of `read_str_from_slice`.
- Modify: `crates/atuin-scripts/src/store/script.rs:65-104` — this file uses `.unwrap()` throughout; convert to `?` via the trait: `bytes.expect_array_len(6)?`, `bytes.read_string()?` (×4 + the tags loop + script), and the tags nested `read_array_len` via `bytes.read_array_len()?`; replace the trailing `if !bytes.is_empty() { bail! }` with `bytes.expect_eof()?`.

> `script.rs` currently `.unwrap()`s — migrating to `?` is a strict improvement (malformed data becomes an error instead of a panic). Keep the `ensure!(nfields == 6, ...)` semantics via `expect_array_len(6)`.

**Step 1-2:** Make the edits. Normalize the alternating `Bytes`/`&[u8]` handling to a single `Bytes` cursor throughout (`read_string` advances it), removing the `Bytes::new(rest)` re-wrapping.

**Step 3: Verify**

Run: `cargo test -p atuin-scripts`
Expected: PASS.
Run: `cargo clippy -p atuin-scripts -- -D warnings`
Expected: PASS.

**Step 4: Commit**
```bash
git add crates/atuin-scripts/Cargo.toml crates/atuin-scripts/src/store/record.rs crates/atuin-scripts/src/store/script.rs
git commit -m "refactor(scripts): use atuin_common::rmp in record/script serde"
```

---

### Task 11: `atuin-dotfiles` — `shell.rs`, `store.rs`, `store/var.rs`

**Files:**
- Modify: `crates/atuin-dotfiles/Cargo.toml` — add `features = ["rmp"]` to `atuin-common`.
- Modify: `crates/atuin-dotfiles/src/shell.rs:44-76` (`Var::deserialize(bytes: &mut decode::Bytes)`) — delete `error_report`; `bytes.expect_array_len(3)?`; `bytes.read_string()?` for name/value; `bytes.read_with(decode::read_bool)?`; `bytes.expect_eof()?`. `Var::serialize(&mut Vec<u8>)` writer stays raw `encode::*`.
- Modify: `crates/atuin-dotfiles/src/store.rs:55-123` — delete `error_report`; `read_u8` via `read_with`; `expect_array_len(2)`/`expect_array_len(1)`; `read_string()`; `expect_eof()`.
- Modify: `crates/atuin-dotfiles/src/store/var.rs:49-99` — delete `error_report`; `read_u8` via `read_with`; delete-arm `expect_array_len(1)` + `read_string()` + `expect_eof()`; the create arm still delegates to `Var::deserialize(&mut bytes)`.

**Step 1-2:** Make the edits. `shell.rs`'s `&mut decode::Bytes` receiver is exactly what `RmpDecodeExt for Bytes` needs — no signature change.

**Step 3: Verify**

Run: `cargo test -p atuin-dotfiles`
Expected: PASS.
Run: `cargo clippy -p atuin-dotfiles -- -D warnings`
Expected: PASS.

**Step 4: Commit**
```bash
git add crates/atuin-dotfiles/Cargo.toml crates/atuin-dotfiles/src/shell.rs crates/atuin-dotfiles/src/store.rs crates/atuin-dotfiles/src/store/var.rs
git commit -m "refactor(dotfiles): use atuin_common::rmp across var/alias serde"
```

---

### Task 12: `atuin-kv` — `store/record.rs`

**Files:**
- Modify: `crates/atuin-kv/Cargo.toml` — add `features = ["rmp"]` to `atuin-common`.
- Modify: `crates/atuin-kv/src/store/record.rs:36-102` — delete `error_report`; both version arms use `expect_array_len(3)` / `expect_array_len(4)`, `read_string()`, `expect_eof()`. Critically, normalize the `v1` branch's `read_bool` on a raw `&mut &[u8]` (line 78) to operate on the `Bytes` cursor: `let has_value = bytes.read_with(decode::read_bool)?;` then continue reading from the same `bytes` cursor (`if has_value { Some(bytes.read_string()?) } else { None }`), and `bytes.expect_eof()?`.

**Step 1-2:** Make the edits. This removes the only `&mut &[u8]` reader in the codebase, unifying on `Bytes`.

**Step 3: Verify**

Run: `cargo test -p atuin-kv`
Expected: PASS.
Run: `cargo clippy -p atuin-kv -- -D warnings`
Expected: PASS.

**Step 4: Commit**
```bash
git add crates/atuin-kv/Cargo.toml crates/atuin-kv/src/store/record.rs
git commit -m "refactor(kv): use atuin_common::rmp in record serde"
```

---

### Task 13: Full-workspace verification

**Step 1: Confirm the boilerplate is gone**

Run: `rg 'fn error_report' crates/`
Expected: no matches.
Run: `rg 'map_err\(\|err\| eyre!\("\{err:\?\}"\)\)' crates/`
Expected: no matches.

**Step 2: Build, test, lint the whole workspace**

Run: `cargo build --workspace`
Expected: PASS.
Run: `cargo test --workspace`
Expected: PASS.
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

**Step 3: Verify the feature-off path still compiles**

Run: `cargo build -p atuin-common` (no `rmp` feature)
Expected: PASS (module excluded).

**Step 4: Commit any final touch-ups**
```bash
git add -A
git commit -m "chore: verify workspace after rmp commonization"
```

---

## Appendix A — Complete target `crates/atuin-common/src/rmp.rs` (module code, no tests)

```rust
//! MessagePack encode/decode helpers built on [`rmp`].
//!
//! `rmp`'s own error types are awkward: some don't implement [`Display`] for
//! every `E`, none say which variant they are, and the decode cursor offers no
//! owned-string read, no nil-aware optional read, and no structural checks
//! (array length, end-of-input). This module fills those gaps with two
//! extension traits — [`RmpDecodeExt`] for reading a [`Bytes`] cursor and
//! [`RmpEncodeExt`] for writing — plus [`DecodeError`] / [`EncodeError`], which
//! carry legible messages and convert cleanly into [`eyre::Report`].
//!
//! Because every decode read returns [`DecodeError`], a decoder can use `?`
//! throughout and convert once at the boundary, rather than hand-rolling a
//! `map_err` at every read.
//!
//! [`Display`]: std::fmt::Display

use rmp::Marker;
use rmp::decode::bytes::{Bytes, BytesReadError};
use rmp::decode::{self, DecodeStringError, NumValueReadError, RmpRead, RmpReadErr, ValueReadError};
use rmp::encode::{self, RmpWrite, RmpWriteErr, ValueWriteError};

/// An error encountered while encoding a MessagePack value.
///
/// Wraps [`ValueWriteError`] with a message that names the failing write and its
/// inner I/O error — neither of which `rmp`'s own [`Display`] reports. Implements
/// [`std::error::Error`], so it converts into [`eyre::Report`] with `?`.
///
/// [`Display`]: std::fmt::Display
#[derive(Debug, derive_more::Display, derive_more::From, thiserror::Error)]
#[display("could not write MessagePack value: {_0:?}")]
pub struct EncodeError<E: RmpWriteErr = std::io::Error>(ValueWriteError<E>);

/// An error encountered while decoding a MessagePack value.
///
/// Wraps the three error types `rmp`'s decode functions return, and adds two
/// structural variants for checks `rmp` does not perform itself:
/// [`UnexpectedArrayLen`](Self::UnexpectedArrayLen) and
/// [`TrailingBytes`](Self::TrailingBytes).
///
/// Converts into [`eyre::Report`] via a manual `From` rather than a
/// [`std::error::Error`] impl, because a [`DecodeError`] is not, in general,
/// `'static`.
///
/// [`Display`]: std::fmt::Display
#[derive(Debug, derive_more::Display)]
pub enum DecodeError<'a, E: RmpReadErr = BytesReadError> {
    /// The next value was not a valid UTF-8 string.
    #[display("could not decode MessagePack string: {_0:?}")]
    DecodeString(DecodeStringError<'a, E>),
    /// The next value was not the expected number.
    #[display("could not decode MessagePack number: {_0:?}")]
    NumValueRead(NumValueReadError<E>),
    /// The next value could not be decoded.
    #[display("could not decode MessagePack value: {_0:?}")]
    ValueRead(ValueReadError<E>),
    /// An array marker did not have the expected length.
    #[display("expected a MessagePack array of length {expected}, found {actual}")]
    UnexpectedArrayLen { expected: u32, actual: u32 },
    /// Input remained after a value was fully decoded.
    #[display("{remaining} trailing byte(s) after decoding MessagePack value")]
    TrailingBytes { remaining: usize },
}

impl<'a, E: RmpReadErr> From<DecodeStringError<'a, E>> for DecodeError<'a, E> {
    fn from(e: DecodeStringError<'a, E>) -> Self {
        Self::DecodeString(e)
    }
}

impl<E: RmpReadErr> From<NumValueReadError<E>> for DecodeError<'_, E> {
    fn from(e: NumValueReadError<E>) -> Self {
        Self::NumValueRead(e)
    }
}

impl<E: RmpReadErr> From<ValueReadError<E>> for DecodeError<'_, E> {
    fn from(e: ValueReadError<E>) -> Self {
        Self::ValueRead(e)
    }
}

impl<E: RmpReadErr> DecodeError<'_, E> {
    /// If this is a type mismatch, the [`Marker`] found instead of the expected
    /// type. [`RmpDecodeExt::read_optional`] uses this to recognise nil.
    pub fn type_mismatch(&self) -> Option<Marker> {
        match self {
            Self::DecodeString(DecodeStringError::TypeMismatch(m))
            | Self::NumValueRead(NumValueReadError::TypeMismatch(m))
            | Self::ValueRead(ValueReadError::TypeMismatch(m)) => Some(*m),
            _ => None,
        }
    }
}

impl<E: RmpReadErr> From<DecodeError<'_, E>> for eyre::Report {
    fn from(e: DecodeError<'_, E>) -> Self {
        eyre::eyre!("{e}")
    }
}

/// Reading helpers for a MessagePack [`Bytes`] cursor.
///
/// Every method reports failures as [`DecodeError`], so decoders use `?`
/// throughout and convert once at the boundary.
pub trait RmpDecodeExt<'a> {
    /// Run an `rmp` decode function, converting its error into [`DecodeError`].
    ///
    /// Lets raw `rmp::decode` functions compose with `?`:
    /// `bytes.read_with(rmp::decode::read_u64)?`.
    fn read_with<T, E, F>(&mut self, read: F) -> Result<T, DecodeError<'a>>
    where
        F: FnOnce(&mut Self) -> Result<T, E>,
        E: Into<DecodeError<'a>>;

    /// Read an owned [`String`].
    fn read_string(&mut self) -> Result<String, DecodeError<'a>>;

    /// Read a value that may be encoded as nil, returning [`None`] for nil.
    ///
    /// `read` decodes a `T`; if it fails specifically because it found
    /// [`Marker::Null`], this yields [`None`] with the cursor left just past the
    /// nil. Any other error is forwarded unchanged.
    fn read_optional<T, E, F>(&mut self, read: F) -> Result<Option<T>, DecodeError<'a>>
    where
        F: FnOnce(&mut Self) -> Result<T, E>,
        E: Into<DecodeError<'a>>;

    /// Read an array-length marker.
    fn read_array_len(&mut self) -> Result<u32, DecodeError<'a>>;

    /// Read an array-length marker and require it to equal `expected`.
    ///
    /// Returns [`DecodeError::UnexpectedArrayLen`] otherwise. For a
    /// forward-compatible field count, use [`read_array_len`](Self::read_array_len)
    /// and range-check the value yourself.
    fn expect_array_len(&mut self, expected: u32) -> Result<u32, DecodeError<'a>>;

    /// Succeed only if the cursor is at end-of-input, else
    /// [`DecodeError::TrailingBytes`] — the standard malformed-record guard after
    /// a fixed set of fields.
    fn expect_eof(&self) -> Result<(), DecodeError<'a>>;
}

impl<'a> RmpDecodeExt<'a> for Bytes<'a> {
    fn read_with<T, E, F>(&mut self, read: F) -> Result<T, DecodeError<'a>>
    where
        F: FnOnce(&mut Self) -> Result<T, E>,
        E: Into<DecodeError<'a>>,
    {
        read(self).map_err(Into::into)
    }

    fn read_string(&mut self) -> Result<String, DecodeError<'a>> {
        let slice = self.remaining_slice();
        let (string, rest) = match decode::read_str_from_slice(slice) {
            Ok(pair) => pair,
            Err(e) => {
                if let DecodeStringError::TypeMismatch(_) = e {
                    // rmp's decode functions consume the marker byte on a type
                    // mismatch; do the same so `read_optional` can detect nil.
                    self.read_u8()
                        .expect("TypeMismatch implies the stream contains a marker byte");
                }
                return Err(e.into());
            }
        };
        *self = Bytes::new(rest);
        Ok(string.into())
    }

    fn read_optional<T, E, F>(&mut self, read: F) -> Result<Option<T>, DecodeError<'a>>
    where
        F: FnOnce(&mut Self) -> Result<T, E>,
        E: Into<DecodeError<'a>>,
    {
        match read(self) {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                let e = e.into();
                if let Some(Marker::Null) = e.type_mismatch() {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn read_array_len(&mut self) -> Result<u32, DecodeError<'a>> {
        decode::read_array_len(self).map_err(Into::into)
    }

    fn expect_array_len(&mut self, expected: u32) -> Result<u32, DecodeError<'a>> {
        let actual = self.read_array_len()?;
        if actual == expected {
            Ok(actual)
        } else {
            Err(DecodeError::UnexpectedArrayLen { expected, actual })
        }
    }

    fn expect_eof(&self) -> Result<(), DecodeError<'a>> {
        let remaining = self.remaining_slice().len();
        if remaining == 0 {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes { remaining })
        }
    }
}

/// Writing helpers for a MessagePack output buffer.
pub trait RmpEncodeExt: RmpWrite {
    /// Write an optional value, encoding [`None`] as [`Marker::Null`].
    ///
    /// The mirror of [`RmpDecodeExt::read_optional`]: `write` encodes the inner
    /// value when `value` is [`Some`].
    fn write_optional<T, F>(
        &mut self,
        value: Option<T>,
        write: F,
    ) -> Result<(), EncodeError<Self::Error>>
    where
        F: FnOnce(&mut Self, T) -> Result<(), ValueWriteError<Self::Error>>;
}

impl<W: RmpWrite> RmpEncodeExt for W {
    fn write_optional<T, F>(
        &mut self,
        value: Option<T>,
        write: F,
    ) -> Result<(), EncodeError<Self::Error>>
    where
        F: FnOnce(&mut Self, T) -> Result<(), ValueWriteError<Self::Error>>,
    {
        match value {
            Some(v) => write(self, v).map_err(EncodeError::from),
            None => encode::write_nil(self)
                .map_err(|e| EncodeError::from(ValueWriteError::InvalidMarkerWrite(e))),
        }
    }
}
```

> **If the build disagrees with Appendix A**, trust the compiler and the *old* module's proven signatures (`crates/atuin-client/src/utils/rmp.rs` in git history) over this appendix — particularly the exact `RmpWrite::Error` / `ValueWriteError` bounds. The old `read_string`/`read_optional`/`write_optional` bodies are known-good; the only substantive additions here are the two structural `DecodeError` variants and the `read_with`/`read_array_len`/`expect_array_len`/`expect_eof` methods.
