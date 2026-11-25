//! Expands the stored contents of a segment back into encoded trees.

use atuin_desktop_capture::ContentHash;

use super::delta;
use super::keyspaces::{ContentValue, Reader};
use super::model::{ContentKey, StoreError};

/// How often a segment stores a whole tree: its first distinct content, and every 32nd after.
pub const WHOLE_EVERY: u32 = 32;

/// A content with its expanded tree.
#[derive(Debug)]
pub struct Expanded {
    pub hash: ContentHash,
    /// The encoded tree.
    pub tree: Vec<u8>,
}

/// The encoded tree of a content, expanded from the nearest whole tree before it.
pub fn tree(reader: &Reader<'_>, key: ContentKey) -> Result<Vec<u8>, StoreError> {
    tree_from(reader, key, None)
}

/// The encoded tree of a content, expanded from `known` when `known` is an earlier content on
/// the content's chain, and otherwise from the nearest whole tree before the content.
pub fn tree_from(
    reader: &Reader<'_>,
    key: ContentKey,
    known: Option<&Expanded>,
) -> Result<Vec<u8>, StoreError> {
    let mut chain: Vec<ContentValue> = Vec::new();
    let mut hash = key.hash;
    let mut base = None;
    loop {
        let value = reader.content(ContentKey {
            segment: key.segment,
            hash,
        })?;
        let next = value.view()?.base;
        chain.push(value);
        match next {
            None => break,
            Some(next) if known.is_some_and(|known| known.hash == next) => {
                base = known.map(|known| known.tree.as_slice());
                break;
            }
            Some(_) if chain.len() >= WHOLE_EVERY as usize => {
                return Err(StoreError::Corrupt("a delta chain is too long"));
            }
            Some(next) => hash = next,
        }
    }
    let mut values = chain.iter().rev();
    let first = values.next().ok_or(StoreError::Corrupt("a content is missing"))?;
    let mut tree = Vec::new();
    expand(reader, first, base, &mut tree)?;
    // Each step expands into the buffer that the step before last used, so that the chain
    // takes two buffers.
    let mut spare = Vec::new();
    for value in values {
        expand(reader, value, Some(&tree), &mut spare)?;
        std::mem::swap(&mut tree, &mut spare);
    }
    Ok(tree)
}

fn expand(
    reader: &Reader<'_>,
    value: &ContentValue,
    base: Option<&[u8]>,
    out: &mut Vec<u8>,
) -> Result<(), StoreError> {
    reader.count_expansion();
    delta::expand(&value.view()?, base, out)
}
