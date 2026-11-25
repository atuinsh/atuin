//! A compact binary form of an element tree.
//!
//! The same tree always encodes to the same bytes, so a store can compare two snapshots, or
//! keep one as a delta from another, byte by byte. The form starts with a version, and a
//! decoder rejects a version or a code that it does not know.
//!
//! After the version byte, each element is written before its children:
//!
//! 1. the role, as one byte;
//! 2. one byte of flags: hidden, checked, unchecked, has text, has a URL, has a heading level;
//! 3. the heading level, as one byte, when the flag says so;
//! 4. the native role and the name, each as a length and UTF-8 bytes;
//! 5. the text and the URL, in the same form, when the flags say so;
//! 6. the number of children.
//!
//! Lengths and counts are unsigned LEB128 numbers.

use crate::backend::{Mark, Node, Role, Visibility};

/// The version that [`encode`] writes.
pub const VERSION: u8 = 1;

/// How many bytes the encoded form takes before its first element: the version.
pub const HEADER_LEN: usize = 1;

/// How deep a tree may nest before [`decode`] gives up, which bounds the decoder's stack.
///
/// Readers keep their trees within this depth, so that every tree that they produce decodes.
pub const MAX_DEPTH: usize = 1024;

const HIDDEN: u8 = 1 << 0;
const CHECKED: u8 = 1 << 1;
const UNCHECKED: u8 = 1 << 2;
const HAS_TEXT: u8 = 1 << 3;
const HAS_URL: u8 = 1 << 4;
const HAS_HEADING_LEVEL: u8 = 1 << 5;
const KNOWN_FLAGS: u8 = HIDDEN | CHECKED | UNCHECKED | HAS_TEXT | HAS_URL | HAS_HEADING_LEVEL;

/// The codes are part of the stored form, so a role keeps its code forever.
const ROLES: [(Role, u8); 16] = [
    (Role::Container, 0),
    (Role::Window, 1),
    (Role::Document, 2),
    (Role::Heading, 3),
    (Role::Paragraph, 4),
    (Role::ListItem, 5),
    (Role::Cell, 6),
    (Role::Label, 7),
    (Role::InlineText, 8),
    (Role::Link, 9),
    (Role::Editable, 10),
    (Role::Secret, 11),
    (Role::Controls, 12),
    (Role::Row, 13),
    (Role::ColumnHeader, 14),
    (Role::Terminal, 15),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("the data ends in the middle of an element")]
    Truncated,
    #[error("the data has version {0}, and this decoder reads version {VERSION}")]
    UnknownVersion(u8),
    #[error("the data uses role code {0}, which this decoder does not know")]
    UnknownRole(u8),
    #[error("the data sets flags that this decoder does not know: {0:#010b}")]
    UnknownFlags(u8),
    #[error("a string in the data is not UTF-8")]
    InvalidText,
    #[error("a number in the data does not fit")]
    Overflow,
    #[error("the tree nests deeper than {MAX_DEPTH} elements")]
    TooDeep,
    #[error("the data goes on after the tree is complete")]
    TrailingBytes,
}

/// Something that takes encoded bytes: a buffer, a hasher that only needs to see them, or a
/// count of their length.
trait Sink {
    fn put(&mut self, bytes: &[u8]);
}

impl Sink for Vec<u8> {
    fn put(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

/// How many bytes [`BufferedHasher`] collects before it hashes them.
const HASH_BUFFER_BYTES: usize = 16 * 1024;

/// Hashes encoded bytes in large updates.
///
/// The encoder puts a few bytes at a time, and BLAKE3 hashes several chunks of one update at
/// once, so a few bytes per update hash about 8 times slower.
struct BufferedHasher {
    hasher: blake3::Hasher,
    buffer: [u8; HASH_BUFFER_BYTES],
    len: usize,
}

impl BufferedHasher {
    fn new() -> Self {
        Self {
            hasher: blake3::Hasher::new(),
            buffer: [0; HASH_BUFFER_BYTES],
            len: 0,
        }
    }

    fn flush(&mut self) {
        self.hasher.update(&self.buffer[..self.len]);
        self.len = 0;
    }

    fn finalize(mut self) -> blake3::Hash {
        self.flush();
        self.hasher.finalize()
    }
}

impl Sink for BufferedHasher {
    fn put(&mut self, bytes: &[u8]) {
        if bytes.len() > HASH_BUFFER_BYTES - self.len {
            self.flush();
        }
        if bytes.len() >= HASH_BUFFER_BYTES {
            self.hasher.update(bytes);
        } else {
            self.buffer[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
        }
    }
}

/// Counts encoded bytes, so that a buffer can be allocated once at the size of a tree.
#[derive(Default)]
struct Length(usize);

impl Sink for Length {
    fn put(&mut self, bytes: &[u8]) {
        self.0 += bytes.len();
    }
}

/// Encodes the tree under `root`.
#[must_use]
pub fn encode(root: &Node) -> Vec<u8> {
    let mut length = Length::default();
    encode_into(root, &mut length);
    encode_with_capacity(root, length.0)
}

/// Encodes the tree under `root` into a buffer that starts with room for `capacity` bytes. A
/// caller that knows the length of the encoded tree avoids a walk of the tree that counts the
/// length.
#[must_use]
pub fn encode_with_capacity(root: &Node, capacity: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(capacity);
    encode_into(root, &mut out);
    out
}

/// The BLAKE3 hash of the encoded tree under `root`, without the encoded bytes in memory.
pub(crate) fn hash(root: &Node) -> blake3::Hash {
    let mut hasher = BufferedHasher::new();
    encode_into(root, &mut hasher);
    hasher.finalize()
}

fn encode_into(root: &Node, out: &mut impl Sink) {
    out.put(&[VERSION]);
    encode_node(root, out);
}

/// How many bytes `node` takes in the encoded form, without the elements under it.
pub(crate) fn element_len(node: &Node) -> usize {
    let mut length = Length::default();
    encode_element(node, &mut length);
    length.0
}

fn encode_node(node: &Node, out: &mut impl Sink) {
    encode_element(node, out);
    for child in &node.children {
        encode_node(child, out);
    }
}

/// Encodes `node` up to its number of children.
fn encode_element(node: &Node, out: &mut impl Sink) {
    let code = ROLES.iter().find(|(role, _)| *role == node.role).map(|&(_, code)| code);
    // A role that was added without a code is written as a container, which loses the role
    // and keeps the tree. The test below fails when a role has no code.
    let mut flags = match node.mark {
        Mark::None => 0,
        Mark::Checked => CHECKED,
        Mark::Unchecked => UNCHECKED,
    };
    if node.visibility == Visibility::Hidden {
        flags |= HIDDEN;
    }
    if node.text.is_some() {
        flags |= HAS_TEXT;
    }
    if node.url.is_some() {
        flags |= HAS_URL;
    }
    if node.heading_level.is_some() {
        flags |= HAS_HEADING_LEVEL;
    }
    out.put(&[code.unwrap_or(0), flags]);
    if let Some(level) = node.heading_level {
        out.put(&[level]);
    }
    let strings = [Some(&*node.native_role), Some(node.name.as_str())];
    for string in strings.into_iter().chain([node.text.as_deref(), node.url.as_deref()]).flatten() {
        put_number(string.len(), out);
        out.put(string.as_bytes());
    }
    put_number(node.children.len(), out);
}

fn put_number(number: usize, out: &mut impl Sink) {
    let mut rest = number;
    loop {
        let low = u8::try_from(rest & 0x7f).expect("seven bits fit in a byte");
        rest >>= 7;
        if rest == 0 {
            out.put(&[low]);
            return;
        }
        out.put(&[low | 0x80]);
    }
}

/// Decodes a tree that [`encode`] wrote.
///
/// # Errors
///
/// Fails when the data is cut short, goes on after the tree, or uses a version, a role, or a
/// flag that this decoder does not know.
pub fn decode(bytes: &[u8]) -> Result<Node, DecodeError> {
    let mut reader = Reader { rest: bytes };
    let version = reader.byte()?;
    if version != VERSION {
        return Err(DecodeError::UnknownVersion(version));
    }
    let root = reader.node(0)?;
    if reader.rest.is_empty() {
        Ok(root)
    } else {
        Err(DecodeError::TrailingBytes)
    }
}

struct Reader<'a> {
    rest: &'a [u8],
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, DecodeError> {
        let (&first, rest) = self.rest.split_first().ok_or(DecodeError::Truncated)?;
        self.rest = rest;
        Ok(first)
    }

    fn number(&mut self) -> Result<usize, DecodeError> {
        let mut number = 0usize;
        for shift in (0..usize::BITS).step_by(7) {
            let byte = self.byte()?;
            let low = usize::from(byte & 0x7f);
            if (low << shift) >> shift != low {
                return Err(DecodeError::Overflow);
            }
            number |= low << shift;
            if byte & 0x80 == 0 {
                return Ok(number);
            }
        }
        Err(DecodeError::Overflow)
    }

    fn string(&mut self) -> Result<String, DecodeError> {
        let length = self.number()?;
        let (bytes, rest) = self.rest.split_at_checked(length).ok_or(DecodeError::Truncated)?;
        self.rest = rest;
        std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| DecodeError::InvalidText)
    }

    fn node(&mut self, depth: usize) -> Result<Node, DecodeError> {
        if depth > MAX_DEPTH {
            return Err(DecodeError::TooDeep);
        }
        let code = self.byte()?;
        let role = ROLES
            .iter()
            .find(|(_, known)| *known == code)
            .map(|&(role, _)| role)
            .ok_or(DecodeError::UnknownRole(code))?;
        let flags = self.byte()?;
        if flags & !KNOWN_FLAGS != 0 {
            return Err(DecodeError::UnknownFlags(flags));
        }
        let heading_level = if flags & HAS_HEADING_LEVEL == 0 {
            None
        } else {
            Some(self.byte()?)
        };
        let native_role = self.string()?;
        let name = self.string()?;
        let text = if flags & HAS_TEXT == 0 {
            None
        } else {
            Some(self.string()?)
        };
        let url = if flags & HAS_URL == 0 {
            None
        } else {
            Some(self.string()?)
        };
        let mut node = Node {
            role,
            native_role: native_role.into(),
            url,
            visibility: if flags & HIDDEN == 0 {
                Visibility::Showing
            } else {
                Visibility::Hidden
            },
            name,
            text,
            heading_level,
            mark: if flags & CHECKED != 0 {
                Mark::Checked
            } else if flags & UNCHECKED != 0 {
                Mark::Unchecked
            } else {
                Mark::None
            },
            children: Vec::new(),
        };
        let children = self.number()?;
        // Every child takes at least four bytes, so a count beyond the data is a lie that
        // must not reserve memory.
        if children > self.rest.len() {
            return Err(DecodeError::Truncated);
        }
        node.children.reserve_exact(children);
        for _ in 0..children {
            node.children.push(self.node(depth + 1)?);
        }
        Ok(node)
    }
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    fn tree() -> Node {
        let leaf = |role, native: &str, text: Option<&str>| Node {
            role,
            native_role: native.to_owned().into(),
            text: text.map(str::to_owned),
            ..Node::default()
        };
        Node {
            role: Role::Window,
            native_role: "frame".into(),
            name: "Rich page — Mozilla Firefox".to_owned(),
            children: vec![Node {
                role: Role::Document,
                native_role: "document web".into(),
                url: Some("https://docs.example/page".to_owned()),
                children: vec![
                    Node {
                        heading_level: Some(2),
                        ..leaf(Role::Heading, "heading", Some("Título"))
                    },
                    Node {
                        mark: Mark::Checked,
                        name: "Remember me".to_owned(),
                        ..leaf(Role::Label, "check box", None)
                    },
                    Node {
                        visibility: Visibility::Hidden,
                        ..leaf(Role::Controls, "push button", Some(""))
                    },
                ],
                ..Node::default()
            }],
            ..Node::default()
        }
    }

    #[rstest]
    fn a_tree_survives_encoding(tree: Node) {
        assert_eq!(decode(&encode(&tree)), Ok(tree));
    }

    #[rstest]
    fn equal_trees_encode_to_equal_bytes(tree: Node) {
        assert_eq!(encode(&tree), encode(&tree.clone()));
    }

    /// A window with `count` paragraphs that each hold `text_bytes` bytes of text.
    fn paragraphs(count: usize, text_bytes: usize) -> Node {
        let paragraph = Node {
            role: Role::Paragraph,
            text: Some("x".repeat(text_bytes)),
            ..Node::default()
        };
        Node {
            role: Role::Window,
            children: vec![paragraph; count],
            ..Node::default()
        }
    }

    #[rstest]
    #[case::small(tree())]
    #[case::many_elements(paragraphs(5_000, 40))]
    #[case::text_longer_than_the_buffer(paragraphs(3, HASH_BUFFER_BYTES * 2 + 1))]
    #[case::text_as_long_as_the_buffer(paragraphs(3, HASH_BUFFER_BYTES))]
    fn the_hash_covers_the_encoded_bytes(#[case] tree: Node) {
        let bytes = encode(&tree);
        assert_eq!(hash(&tree), blake3::hash(&bytes));
        assert_eq!(bytes.capacity(), bytes.len());
    }

    /// Stores keep snapshots by these bytes and point back to them by this hash, so neither may
    /// change for a tree that is already stored.
    #[rstest]
    fn the_stored_form_stays_the_same(tree: Node) {
        let mut expected = vec![VERSION];
        // The window: role, flags, native role, name, and one child.
        expected.extend([1, 0, 5]);
        expected.extend(b"frame");
        expected.push(29);
        expected.extend("Rich page — Mozilla Firefox".as_bytes());
        expected.push(1);
        // The document: role, flags with a URL, native role, empty name, URL, three children.
        expected.extend([2, HAS_URL, 12]);
        expected.extend(b"document web");
        expected.extend([0, 25]);
        expected.extend(b"https://docs.example/page");
        expected.push(3);
        // The heading: role, flags with a text and a level, level, native role, empty name,
        // text, no children.
        expected.extend([3, HAS_TEXT | HAS_HEADING_LEVEL, 2, 7]);
        expected.extend(b"heading");
        expected.extend([0, 7]);
        expected.extend("Título".as_bytes());
        expected.push(0);
        // The check box: role, checked, native role, name, no children.
        expected.extend([7, CHECKED, 9]);
        expected.extend(b"check box");
        expected.push(11);
        expected.extend(b"Remember me");
        expected.push(0);
        // The button: role, hidden with an empty text, native role, empty name, empty text, no
        // children.
        expected.extend([12, HIDDEN | HAS_TEXT, 11]);
        expected.extend(b"push button");
        expected.extend([0, 0, 0]);

        assert_eq!(encode(&tree), expected);
        assert_eq!(
            hash(&tree).to_hex().as_str(),
            "4f70568d2ca98aa71bdbb940b9ab507bb81a577dbb8658e310c62553e434cb7e"
        );
    }

    #[rstest]
    fn a_small_element_takes_five_bytes() {
        // The version, the role, the flags, two empty strings, and no children.
        assert_eq!(encode(&Node::default()), [VERSION, 0, 0, 0, 0, 0]);
    }

    #[rstest]
    fn every_role_has_a_code_of_its_own() {
        // The match fails to compile when a role is added, which is the reminder to give the
        // new role a code.
        let all = ROLES.map(|(role, _)| match role {
            Role::Container
            | Role::Window
            | Role::Document
            | Role::Heading
            | Role::Paragraph
            | Role::ListItem
            | Role::Cell
            | Role::Label
            | Role::InlineText
            | Role::Link
            | Role::Editable
            | Role::Secret
            | Role::Controls
            | Role::Row
            | Role::ColumnHeader
            | Role::Terminal => role,
        });
        for (index, (role, code)) in ROLES.into_iter().enumerate() {
            assert_eq!(usize::from(code), index, "{role:?}");
            assert_eq!(all.iter().filter(|other| **other == role).count(), 1, "{role:?}");
        }
    }

    #[rstest]
    #[case::empty(&[], DecodeError::Truncated)]
    #[case::newer_version(&[2, 0, 0, 0, 0, 0], DecodeError::UnknownVersion(2))]
    #[case::unknown_role(&[VERSION, 200, 0, 0, 0, 0], DecodeError::UnknownRole(200))]
    #[case::unknown_flag(&[VERSION, 0, 0x80, 0, 0, 0], DecodeError::UnknownFlags(0x80))]
    #[case::cut_short(&[VERSION, 0, 0, 0], DecodeError::Truncated)]
    #[case::string_past_the_end(&[VERSION, 0, 0, 9, b'a'], DecodeError::Truncated)]
    #[case::not_utf8(&[VERSION, 0, 0, 1, 0xff, 0, 0], DecodeError::InvalidText)]
    #[case::children_past_the_end(&[VERSION, 0, 0, 0, 0, 7], DecodeError::Truncated)]
    #[case::left_over(&[VERSION, 0, 0, 0, 0, 0, 0], DecodeError::TrailingBytes)]
    #[case::endless_number(&[VERSION, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], DecodeError::Overflow)]
    fn bad_data_is_rejected(#[case] bytes: &[u8], #[case] error: DecodeError) {
        assert_eq!(decode(bytes), Err(error));
    }

    #[rstest]
    fn deep_nesting_is_rejected() {
        let mut bytes = vec![VERSION];
        for _ in 0..=MAX_DEPTH + 1 {
            bytes.extend([0, 0, 0, 0, 1]);
        }
        assert_eq!(decode(&bytes), Err(DecodeError::TooDeep));
    }
}
