//! Compresses encoded trees with zstd, alone or as a delta from an earlier tree.
//!
//! A delta compresses the new tree with the earlier tree as zstd's reference prefix, so the
//! parts that the two trees share cost almost nothing.

use std::cell::RefCell;

use zstd::zstd_safe::{CCtx, CParameter, DCtx, DParameter, ResetDirective, get_frame_content_size};

use super::model::StoreError;
use super::schema::StoredContent;

const LEVEL: i32 = 3;

/// The smallest window that zstd accepts, as a power of two.
const MIN_WINDOW_LOG: u32 = 10;

/// The largest window that a decoder accepts by default, as a power of two.
///
/// A delta's window spans the base and the new tree, so that every part of the base stays in
/// reach. `MAX_WINDOW_LOG` bounds a delta's window at 128 MiB.
const MAX_WINDOW_LOG: u32 = 27;

/// The largest tree that [`expand`] decompresses. A larger size in a frame means damage.
const MAX_TREE_BYTES: u64 = 1 << 30;

thread_local! {
    /// The context that compresses whole trees on this thread.
    ///
    /// A context that refers to a prefix borrows the prefix for the context's whole life, so
    /// only the contexts of whole trees outlive one call.
    static WHOLE_COMPRESSOR: RefCell<CCtx<'static>> = RefCell::new(CCtx::create());

    /// The context that expands whole trees on this thread.
    static WHOLE_EXPANDER: RefCell<DCtx<'static>> = RefCell::new(DCtx::create());
}

/// Compresses a whole tree.
pub fn whole(tree: &[u8]) -> Vec<u8> {
    WHOLE_COMPRESSOR.with_borrow_mut(|cctx| {
        cctx.reset(ResetDirective::SessionAndParameters)
            .expect("no compression is in progress between calls");
        compress(cctx, tree)
    })
}

/// Compresses `tree` as a delta from `base`.
pub fn delta(tree: &[u8], base: &[u8]) -> Vec<u8> {
    let mut cctx = CCtx::create();
    cctx.set_parameter(CParameter::WindowLog(window_log(base.len() + tree.len())))
        .expect("the window log is within zstd's bounds");
    cctx.ref_prefix(base).expect("zstd accepts any prefix");
    compress(&mut cctx, tree)
}

fn compress(cctx: &mut CCtx<'_>, tree: &[u8]) -> Vec<u8> {
    cctx.set_parameter(CParameter::CompressionLevel(LEVEL))
        .expect("zstd accepts compression level 3");
    let mut out = Vec::with_capacity(zstd::zstd_safe::compress_bound(tree.len()));
    cctx.compress2(&mut out, tree).expect("the output has room for the compression bound");
    out
}

/// The window, as a power of two, that covers `len` bytes.
fn window_log(len: usize) -> u32 {
    let log = usize::BITS - len.saturating_sub(1).leading_zeros();
    log.clamp(MIN_WINDOW_LOG, MAX_WINDOW_LOG)
}

/// Decompresses a stored content into `out`, given the expanded tree of its base when it has
/// one.
///
/// `out` loses what it held before, and keeps its allocation when the allocation is large
/// enough.
pub fn expand(
    stored: &StoredContent<'_>,
    base: Option<&[u8]>,
    out: &mut Vec<u8>,
) -> Result<(), StoreError> {
    let size = get_frame_content_size(stored.bytes)
        .ok()
        .flatten()
        .filter(|&size| size <= MAX_TREE_BYTES)
        .and_then(|size| usize::try_from(size).ok())
        .ok_or(StoreError::Corrupt("a content has no valid size"))?;
    match (stored.base, base) {
        (Some(_), Some(base)) => {
            let mut dctx = DCtx::create();
            dctx.ref_prefix(base).expect("zstd accepts any prefix");
            decompress(&mut dctx, stored.bytes, size, out)
        }
        (None, None) => WHOLE_EXPANDER.with_borrow_mut(|dctx| {
            dctx.reset(ResetDirective::SessionAndParameters)
                .expect("no decompression is in progress between calls");
            decompress(dctx, stored.bytes, size, out)
        }),
        (Some(_), None) => Err(StoreError::Corrupt("a delta is missing its base")),
        (None, Some(_)) => Err(StoreError::Corrupt("a whole tree was given a base")),
    }
}

fn decompress(
    dctx: &mut DCtx<'_>,
    bytes: &[u8],
    size: usize,
    out: &mut Vec<u8>,
) -> Result<(), StoreError> {
    dctx.set_parameter(DParameter::WindowLogMax(MAX_WINDOW_LOG))
        .expect("zstd accepts its default window limit");
    out.clear();
    out.reserve(size);
    dctx.decompress(out, bytes)
        .map_err(|_| StoreError::Corrupt("a content does not decompress"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use atuin_desktop_capture::ContentHash;
    use atuin_desktop_capture::encoding::encode;
    use proptest::prelude::*;
    use proptest::test_runner::TestRunner;
    use rstest::rstest;

    use super::*;
    use crate::desktop_capture::testing::records::page;

    /// The encoded tree of a page of `lines`.
    fn tree<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<u8> {
        encode(&page(lines.into_iter().map(str::to_owned)))
    }

    fn stored(bytes: &[u8], base: Option<ContentHash>) -> StoredContent<'_> {
        StoredContent { base, bytes }
    }

    /// The tree that `stored` expands to, in a buffer that held another tree before.
    fn expanded(stored: &StoredContent<'_>, base: Option<&[u8]>) -> Result<Vec<u8>, StoreError> {
        let mut out = b"an earlier tree".to_vec();
        expand(stored, base, &mut out).map(|()| out)
    }

    fn lines() -> impl Strategy<Value = Vec<String>> {
        proptest::collection::vec(".{0,40}", 0..60)
    }

    #[rstest]
    fn round_trip() {
        let trees = (lines(), lines(), lines()).prop_map(|(common, a_tail, b_tail)| {
            let a = tree(common.iter().chain(&a_tail).map(String::as_str));
            let b = tree(common.iter().chain(&b_tail).map(String::as_str));
            [a, b]
        });
        TestRunner::default()
            .run(&trees, |[a, b]| {
                let base = Some(ContentHash([0; 32]));
                prop_assert_eq!(expanded(&stored(&delta(&b, &a), base), Some(&a)).unwrap(), b);
                // The thread's contexts of whole trees serve one call after another.
                for _ in 0..2 {
                    prop_assert_eq!(expanded(&stored(&whole(&a), None), None).unwrap(), a.clone());
                }
                Ok(())
            })
            .unwrap();
    }

    /// A delta of a tree of several hundred kilobytes stays small and expands back.
    #[rstest]
    #[case::grows_by_one_line(8_000, 8_001)]
    #[case::drops_its_first_lines(8_000, 7_900)]
    fn round_trip_of_large_trees(#[case] base_lines: usize, #[case] tree_lines: usize) {
        let line = |i: usize| format!("{i} Compiling crate-{i} v0.1.{i} (/build/crates/c{i})");
        let base_text: Vec<String> = (0..base_lines).map(line).collect();
        let first = base_lines.saturating_sub(tree_lines);
        let tree_text: Vec<String> = (first..first + tree_lines).map(line).collect();
        let base = tree(base_text.iter().map(String::as_str));
        let new = tree(tree_text.iter().map(String::as_str));
        assert!(base.len() > 400_000, "the base takes {} bytes", base.len());

        let compressed = delta(&new, &base);
        assert!(compressed.len() < 2_000, "the delta takes {} bytes", compressed.len());
        let content = stored(&compressed, Some(ContentHash([0; 32])));
        assert_eq!(expanded(&content, Some(&base)).unwrap(), new);
    }

    #[rstest]
    fn a_delta_needs_its_base() {
        let a = tree(["a"]);
        let compressed = delta(&tree(["b"]), &a);
        let content = stored(&compressed, Some(ContentHash([0; 32])));
        assert!(matches!(expanded(&content, None), Err(StoreError::Corrupt(_))));
    }

    /// A whole tree that fails to expand leaves the thread's context usable.
    #[rstest]
    fn a_damaged_whole_tree_leaves_the_context_usable() {
        let a = tree(["a"]);
        let mut damaged = whole(&a);
        let last = damaged.len() - 1;
        damaged[last] ^= 0xff;
        damaged.truncate(last - 2);
        let _ = expanded(&stored(&damaged, None), None);

        assert_eq!(expanded(&stored(&whole(&a), None), None).unwrap(), a);
    }
}
