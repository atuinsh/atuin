//! Private utility functions

const TAG_CONT: u8    = 0b1000_0000;
const TAG_TWO_B: u8   = 0b1100_0000;
const TAG_THREE_B: u8 = 0b1110_0000;
const TAG_FOUR_B: u8  = 0b1111_0000;
const MAX_ONE_B: u32   =     0x80;
const MAX_TWO_B: u32   =    0x800;
const MAX_THREE_B: u32 =  0x10000;

#[inline]
pub fn encode_utf8(c: char) -> EncodeUtf8 {
    let code = c as u32;
    let mut buf = [0; 4];
    let pos = if code < MAX_ONE_B {
        buf[3] = code as u8;
        3
    } else if code < MAX_TWO_B {
        buf[2] = (code >> 6 & 0x1F) as u8 | TAG_TWO_B;
        buf[3] = (code & 0x3F) as u8 | TAG_CONT;
        2
    } else if code < MAX_THREE_B {
        buf[1] = (code >> 12 & 0x0F) as u8 | TAG_THREE_B;
        buf[2] = (code >>  6 & 0x3F) as u8 | TAG_CONT;
        buf[3] = (code & 0x3F) as u8 | TAG_CONT;
        1
    } else {
        buf[0] = (code >> 18 & 0x07) as u8 | TAG_FOUR_B;
        buf[1] = (code >> 12 & 0x3F) as u8 | TAG_CONT;
        buf[2] = (code >>  6 & 0x3F) as u8 | TAG_CONT;
        buf[3] = (code & 0x3F) as u8 | TAG_CONT;
        0
    };
    EncodeUtf8 { buf: buf, pos: pos }
}

pub struct EncodeUtf8 {
    buf: [u8; 4],
    pos: usize,
}

impl EncodeUtf8 {
    // FIXME: use this from_utf8_unchecked, since we know it can never fail
    pub fn as_str(&self) -> &str {
        ::core::str::from_utf8(&self.buf[self.pos..]).unwrap()
    }
}

#[allow(non_upper_case_globals)]
const Pattern_White_Space_table: &'static [(char, char)] = &[
    ('\u{9}', '\u{d}'), ('\u{20}', '\u{20}'), ('\u{85}', '\u{85}'), ('\u{200e}', '\u{200f}'),
    ('\u{2028}', '\u{2029}')
];

fn bsearch_range_table(c: char, r: &'static [(char, char)]) -> bool {
    use core::cmp::Ordering::{Equal, Less, Greater};
    r.binary_search_by(|&(lo, hi)| {
        if c < lo {
            Greater
        } else if hi < c {
            Less
        } else {
            Equal
        }
    })
    .is_ok()
}

#[allow(non_snake_case)]
pub fn Pattern_White_Space(c: char) -> bool {
    bsearch_range_table(c, Pattern_White_Space_table)
}
