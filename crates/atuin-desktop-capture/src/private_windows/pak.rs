//! Reads the strings of Chromium's resource packs, the `.pak` files that hold a browser's
//! translations.
//!
//! A pack starts with a header and an index of its resources, sorted by id. Version 4 and
//! version 5 differ in their headers, and version 5 adds a table of aliases, which point an id
//! at the data of another resource. Edge writes a variant of version 5 whose counts and ids
//! take 32 bits. Offsets take 32 bits in every layout. The two layouts of version 5 are told
//! apart by where the data starts: the first resource begins right after the alias table.

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::Path;

/// A text that [`Pak::find_each`] found, and the resource that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoundText<'t> {
    pub text: &'t str,
    pub id: ResourceId,
}

/// One entry of a pack's index.
struct Entry {
    id: u32,
    /// Where the resource's data starts. The data ends where the next entry's starts.
    offset: usize,
}

/// The id of a resource in a pack. The same message has the same id in every pack of one
/// build, and a different id in another build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceId(u32);

/// Where the parts of a pack lie, and how wide its fields are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    /// How many resources the index lists. The index holds one more entry, which marks the
    /// end of the last resource.
    resources: u32,
    aliases: u32,
    /// Where the index starts.
    index: u64,
    /// The width of an id, and of an alias's index into the resources.
    id_width: u8,
    encoding: Encoding,
}

impl Layout {
    /// The width of an offset, which is 32 bits in every layout.
    const OFFSET_WIDTH: u64 = 4;

    fn entry_size(self) -> u64 {
        u64::from(self.id_width) + Self::OFFSET_WIDTH
    }

    fn alias_size(self) -> u64 {
        2 * u64::from(self.id_width)
    }

    fn aliases_start(self) -> u64 {
        self.index + (u64::from(self.resources) + 1) * self.entry_size()
    }

    fn data_start(self) -> u64 {
        self.aliases_start() + u64::from(self.aliases) * self.alias_size()
    }

    /// Reads the layout from the first bytes of a pack.
    fn parse(header: &[u8; HEADER_SIZE]) -> Option<Self> {
        let u16_at = |at: usize| u16::from_le_bytes([header[at], header[at + 1]]);
        let u32_at = |at: usize| {
            u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
        };
        match u32_at(0) {
            4 => Some(Self {
                resources: u32_at(4),
                aliases: 0,
                index: 9,
                id_width: 2,
                encoding: Encoding::from_byte(header[8])?,
            }),
            5 => {
                let encoding = Encoding::from_byte(header[4])?;
                let narrow = Self {
                    resources: u32::from(u16_at(8)),
                    aliases: u32::from(u16_at(10)),
                    index: 12,
                    id_width: 2,
                    encoding,
                };
                let wide = Self {
                    resources: u32_at(8),
                    aliases: u32_at(12),
                    index: 16,
                    id_width: 4,
                    encoding,
                };
                // The first entry's offset follows its id.
                let first_offset = |layout: Self| {
                    let at = usize::try_from(layout.index).ok()? + usize::from(layout.id_width);
                    header.get(at..at + 4).map(|_| u64::from(u32_at(at)))
                };
                [narrow, wide]
                    .into_iter()
                    .find(|&layout| first_offset(layout) == Some(layout.data_start()))
            }
            _ => None,
        }
    }
}

/// The header of either layout of version 5 fits in this many bytes, together with the first
/// entry of the index.
const HEADER_SIZE: usize = 24;

/// How the resources of a pack encode text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Binary,
    Utf8,
    Utf16,
}

impl Encoding {
    const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Binary),
            1 => Some(Self::Utf8),
            2 => Some(Self::Utf16),
            _ => None,
        }
    }

    fn decode(self, bytes: Vec<u8>) -> Option<String> {
        match self {
            Self::Binary => None,
            Self::Utf8 => String::from_utf8(bytes).ok(),
            Self::Utf16 => {
                let units = bytes.as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair));
                char::decode_utf16(units).collect::<Result<String, _>>().ok()
            }
        }
    }
}

/// An open pack, which reads one resource at a time.
pub struct Pak {
    file: File,
    layout: Layout,
}

impl Pak {
    /// Opens the pack at `path`. Returns `None` when the file is not a pack of a known
    /// layout.
    pub fn open(path: &Path) -> io::Result<Option<Self>> {
        let mut file = File::open(path)?;
        // A pack with few resources can be shorter than the header buffer, so the rest of the
        // buffer stays zero.
        let mut header = [0; HEADER_SIZE];
        let mut filled = 0;
        while filled < HEADER_SIZE {
            match file.read(&mut header[filled..])? {
                0 => break,
                read => filled += read,
            }
        }
        Ok(Layout::parse(&header).map(|layout| Self { file, layout }))
    }

    /// The text of the resource `id`. Returns `None` when the pack lacks the resource, or
    /// when the resource is not text.
    pub fn get(&mut self, id: ResourceId) -> io::Result<Option<String>> {
        let Some(entry) = self.entry_of(id)? else {
            return Ok(None);
        };
        let (start, end) = (self.offset_of(entry)?, self.offset_of(entry + 1)?);
        let Some(length) = end.checked_sub(start) else {
            return Ok(None);
        };
        let mut bytes = vec![0; usize::try_from(length).unwrap_or(usize::MAX).min(MAX_TEXT)];
        self.file.seek(SeekFrom::Start(start))?;
        self.file.read_exact(&mut bytes)?;
        Ok(self.layout.encoding.decode(bytes))
    }

    /// Finds the resource that holds each of `texts`, in one read of the pack. The first
    /// resource whose text equals a text wins. The result leaves out a text that isn't in the
    /// pack.
    pub fn find_each<'t>(&mut self, texts: &[&'t str]) -> io::Result<Vec<FoundText<'t>>> {
        let wanted: Vec<Cow<'_, [u8]>> = match self.layout.encoding {
            Encoding::Binary => return Ok(Vec::new()),
            Encoding::Utf8 => texts.iter().map(|text| Cow::Borrowed(text.as_bytes())).collect(),
            Encoding::Utf16 => texts
                .iter()
                .map(|text| Cow::Owned(text.encode_utf16().flat_map(u16::to_le_bytes).collect()))
                .collect(),
        };
        let mut data = Vec::new();
        self.file.seek(SeekFrom::Start(0))?;
        self.file.read_to_end(&mut data)?;
        let layout = self.layout;
        let entry_at = |entry: u32| -> Option<Entry> {
            let at = usize::try_from(layout.index + u64::from(entry) * layout.entry_size()).ok()?;
            let id = read_uint(data.get(at..)?, layout.id_width)?;
            let offset = read_uint(data.get(at + usize::from(layout.id_width)..)?, 4)?;
            Some(Entry {
                id,
                offset: usize::try_from(offset).ok()?,
            })
        };
        let mut found: Vec<FoundText<'t>> = Vec::new();
        for entry in 0..layout.resources {
            let (Some(this), Some(next)) = (entry_at(entry), entry_at(entry + 1)) else {
                break;
            };
            let Some(bytes) = data.get(this.offset..next.offset) else {
                continue;
            };
            for (text, wanted) in texts.iter().zip(&wanted) {
                if bytes == wanted.as_ref() && !found.iter().any(|found| found.text == *text) {
                    found.push(FoundText {
                        text,
                        id: ResourceId(this.id),
                    });
                }
            }
        }
        Ok(found)
    }

    /// The position of `id` in the index, looked up through the aliases when the index lacks
    /// it.
    fn entry_of(&mut self, id: ResourceId) -> io::Result<Option<u32>> {
        let layout = self.layout;
        if let Some(entry) = self.search(layout.index, layout.resources, layout.entry_size(), id)? {
            return Ok(Some(entry));
        }
        let start = layout.aliases_start();
        let Some(alias) = self.search(start, layout.aliases, layout.alias_size(), id)? else {
            return Ok(None);
        };
        let at = start + u64::from(alias) * layout.alias_size() + u64::from(layout.id_width);
        let entry = self.uint_at(at, layout.id_width)?;
        Ok((entry < layout.resources).then_some(entry))
    }

    /// Searches `count` records of `size` bytes from `start`, each starting with an id, for
    /// `id`. The records are sorted by id.
    fn search(
        &mut self,
        start: u64,
        count: u32,
        size: u64,
        ResourceId(id): ResourceId,
    ) -> io::Result<Option<u32>> {
        let (mut low, mut high) = (0, count);
        while low < high {
            let middle = low + (high - low) / 2;
            let found = self.uint_at(start + u64::from(middle) * size, self.layout.id_width)?;
            match found.cmp(&id) {
                std::cmp::Ordering::Equal => return Ok(Some(middle)),
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
            }
        }
        Ok(None)
    }

    fn offset_of(&mut self, entry: u32) -> io::Result<u64> {
        let layout = self.layout;
        let at = layout.index + u64::from(entry) * layout.entry_size() + u64::from(layout.id_width);
        self.uint_at(at, 4).map(u64::from)
    }

    fn uint_at(&mut self, at: u64, width: u8) -> io::Result<u32> {
        let mut bytes = [0; 4];
        let bytes = &mut bytes[..usize::from(width)];
        self.file.seek(SeekFrom::Start(at))?;
        self.file.read_exact(bytes)?;
        read_uint(bytes, width).ok_or_else(|| io::ErrorKind::InvalidData.into())
    }
}

/// The longest text that a lookup reads. Titles are short, and a corrupt offset must not make
/// a lookup allocate the size of the file.
const MAX_TEXT: usize = 4096;

/// Reads a little-endian number of `width` bytes, 2 or 4, from the start of `bytes`.
fn read_uint(bytes: &[u8], width: u8) -> Option<u32> {
    match width {
        2 => bytes.first_chunk().map(|&pair| u32::from(u16::from_le_bytes(pair))),
        4 => bytes.first_chunk().map(|&quad| u32::from_le_bytes(quad)),
        _ => None,
    }
}

#[cfg(test)]
pub mod tests {
    use std::io::Write as _;
    use std::path::PathBuf;

    use proptest::prelude::*;
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    /// The layouts that [`build`] can write.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Built {
        Version4,
        Version5,
        /// Edge's variant of version 5, with 32-bit fields.
        Wide,
    }

    /// A resource that [`build`] writes.
    #[derive(Debug, Clone, Copy)]
    pub struct Resource<'a> {
        pub id: u32,
        pub text: &'a str,
    }

    /// An alias that [`build`] writes: `id` reads as the resource at position `entry`.
    #[derive(Debug, Clone, Copy)]
    pub struct Alias {
        pub id: u32,
        pub entry: u32,
    }

    /// Writes a UTF-8 pack with `resources` and `aliases`, both sorted by id.
    pub fn build(layout: Built, resources: &[Resource<'_>], aliases: &[Alias]) -> Vec<u8> {
        let count = u32::try_from(resources.len()).unwrap();
        let alias_count = u32::try_from(aliases.len()).unwrap();
        let mut out = Vec::new();
        let (id_width, header_len) = match layout {
            Built::Version4 => {
                out.extend(4u32.to_le_bytes());
                out.extend(count.to_le_bytes());
                out.push(1);
                (2, 9)
            }
            Built::Version5 => {
                out.extend(5u32.to_le_bytes());
                out.extend([1, 0, 0, 0]);
                out.extend(u16::try_from(count).unwrap().to_le_bytes());
                out.extend(u16::try_from(alias_count).unwrap().to_le_bytes());
                (2, 12)
            }
            Built::Wide => {
                out.extend(5u32.to_le_bytes());
                out.extend([1, 0, 0, 0]);
                out.extend(count.to_le_bytes());
                out.extend(alias_count.to_le_bytes());
                (4, 16)
            }
        };
        let put = |out: &mut Vec<u8>, value: u32, width: usize| {
            out.extend(&value.to_le_bytes()[..width]);
        };
        let entry_size = id_width + 4;
        let alias_size = if layout == Built::Version4 {
            0
        } else {
            2 * id_width
        };
        let mut offset = u32::try_from(
            header_len + (resources.len() + 1) * entry_size + aliases.len() * alias_size,
        )
        .unwrap();
        let end = Resource {
            id: resources.last().map_or(0, |last| last.id + 1),
            text: "",
        };
        for resource in resources.iter().chain([&end]) {
            put(&mut out, resource.id, id_width);
            put(&mut out, offset, 4);
            offset += u32::try_from(resource.text.len()).unwrap();
        }
        for alias in aliases {
            put(&mut out, alias.id, id_width);
            put(&mut out, alias.entry, id_width);
        }
        for resource in resources {
            out.extend(resource.text.as_bytes());
        }
        out
    }

    /// Writes `bytes` to `name` in `dir`, and returns its path.
    pub fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        File::create(&path).unwrap().write_all(bytes).unwrap();
        path
    }

    #[fixture]
    pub fn dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    fn open(dir: &TempDir, bytes: &[u8]) -> Pak {
        Pak::open(&write(dir, "test.pak", bytes)).unwrap().expect("a pack of a known layout")
    }

    #[rstest]
    fn reads_resources_and_aliases(
        dir: TempDir,
        #[values(Built::Version4, Built::Version5, Built::Wide)] layout: Built,
    ) {
        let aliases: &[Alias] = if layout == Built::Version4 {
            &[]
        } else {
            &[Alias { id: 15, entry: 1 }]
        };
        let texts = [
            Resource { id: 3, text: "one" },
            Resource {
                id: 11,
                text: "$1 (Incognito)",
            },
            Resource {
                id: 40,
                text: "last",
            },
        ];
        let bytes = build(layout, &texts, aliases);
        let mut pak = open(&dir, &bytes);
        assert_eq!(pak.get(ResourceId(3)).unwrap().as_deref(), Some("one"));
        assert_eq!(pak.get(ResourceId(40)).unwrap().as_deref(), Some("last"));
        assert_eq!(pak.get(ResourceId(4)).unwrap(), None);
        assert_eq!(pak.find_each(&["(Incognito)", "last", "$1 (Incognito)"]).unwrap(), [
            FoundText {
                text: "$1 (Incognito)",
                id: ResourceId(11),
            },
            FoundText {
                text: "last",
                id: ResourceId(40),
            },
        ]);
        if layout != Built::Version4 {
            assert_eq!(pak.get(ResourceId(15)).unwrap().as_deref(), Some("$1 (Incognito)"));
        }
    }

    #[rstest]
    #[case::empty(&[])]
    #[case::short(&[5, 0, 0, 0])]
    #[case::version_6(&[6, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])]
    fn rejects_unknown_files(dir: TempDir, #[case] bytes: &[u8]) {
        let path = write(&dir, "bad.pak", bytes);
        assert!(!matches!(Pak::open(&path), Ok(Some(_))));
    }

    proptest! {
        #[test]
        fn every_resource_reads_back(
            texts in prop::collection::btree_map(0u32..60_000, "[a-zé()$1 ]{0,20}", 1..40),
            wide in any::<bool>(),
        ) {
            let dir = dir();
            let resources: Vec<Resource<'_>> =
                texts.iter().map(|(&id, text)| Resource { id, text }).collect();
            let layout = if wide { Built::Wide } else { Built::Version5 };
            let mut pak = open(&dir, &build(layout, &resources, &[]));
            for resource in &resources {
                let read = pak.get(ResourceId(resource.id)).unwrap();
                prop_assert_eq!(read.as_deref(), Some(resource.text));
            }
        }
    }
}
