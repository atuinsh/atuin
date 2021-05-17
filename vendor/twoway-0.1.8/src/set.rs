use std::cmp;

/*
/// Find one out of a set of bytes, max 4.
pub fn set(text: &[u8], set: &[u8]) -> Option<usize> {
    None
}

/// Find two consecutive bytes
pub fn find2(text: &[u8], a: u8, b: u8) -> Option<usize> {
    None
}
*/

const LO_U64: u64 = 0x0101010101010101;
const HI_U64: u64 = 0x8080808080808080;

// use truncation
const LO_USIZE: usize = LO_U64 as usize;
const HI_USIZE: usize = HI_U64 as usize;

#[cfg(target_pointer_width = "32")]
const USIZE_BYTES: usize = 4;
#[cfg(target_pointer_width = "64")]
const USIZE_BYTES: usize = 8;

/// Return `true` if `x` contains any zero byte.
///
/// From *Matters Computational*, J. Arndt
///
/// "The idea is to subtract one from each of the bytes and then look for
/// bytes where the borrow propagated all the way to the most significant
/// bit."
#[inline]
fn contains_zero_byte(x: usize) -> bool {
    x.wrapping_sub(LO_USIZE) & !x & HI_USIZE != 0
}

#[cfg(target_pointer_width = "32")]
#[inline]
fn repeat_byte(b: u8) -> usize {
    let mut rep = (b as usize) << 8 | b as usize;
    rep = rep << 16 | rep;
    rep
}

#[cfg(target_pointer_width = "64")]
#[inline]
fn repeat_byte(b: u8) -> usize {
    let mut rep = (b as usize) << 8 | b as usize;
    rep = rep << 16 | rep;
    rep = rep << 32 | rep;
    rep
}

pub fn find_byte(x: u8, text: &[u8]) -> Option<usize> {
    let len = text.len();
    let ptr = text.as_ptr();

    // search up to an aligned boundary
    let align = (ptr as usize) & (USIZE_BYTES - 1);
    let mut offset;
    if align > 0 {
        offset = cmp::min(USIZE_BYTES - align, len);
        if let Some(index) = text[..offset].iter().position(|elt| *elt == x) {
            return Some(index);
        }
    } else {
        offset = 0;
    }

    let repeated_x = repeat_byte(x);

    if len >= 2 * USIZE_BYTES {
        while offset <= len - 2 * USIZE_BYTES {
            unsafe {
                let u = *(ptr.offset(offset as isize) as *const usize);
                let v = *(ptr.offset((offset + USIZE_BYTES) as isize) as *const usize);

                // break if there is a matching byte
                let zu = contains_zero_byte(u ^ repeated_x);
                let zv = contains_zero_byte(v ^ repeated_x);
                if zu || zv {
                    break;
                }
            }
            offset += USIZE_BYTES * 2;
        }
    }

    // find the byte after the point the body loop stopped
    text[offset..].iter().position(|elt| *elt == x).map(|i| offset + i)
}

#[test]
fn test_find_byte() {
    let text = b"abcb";
    assert_eq!(find_byte(b'd', text), None);
    assert_eq!(find_byte(b'a', text), Some(0));
    assert_eq!(find_byte(b'c', text), Some(2));
    assert_eq!(find_byte(b'b', text), Some(1));

    let longer = "longer text and so on, a bit zebras and zw";
    assert_eq!(find_byte(b'z', longer.as_bytes()), longer.find('z'));
    assert_eq!(find_byte(b'w', longer.as_bytes()), longer.find('w'));
}

pub fn rfind_byte(x: u8, text: &[u8]) -> Option<usize> {
    // Scan for a single byte value by reading two `usize` words at a time.
    //
    // Split `text` in three parts
    // - unaligned tail, after the last word aligned address in text
    // - body, scan by 2 words at a time
    // - the first remaining bytes, < 2 word size
    let len = text.len();
    let ptr = text.as_ptr();

    // search up to a 16 byte aligned boundary
    let end_align = (ptr as usize + len) & (USIZE_BYTES - 1);
    let mut offset;
    if end_align > 0 {
        offset = len - cmp::min(USIZE_BYTES - end_align, len);
        if let Some(index) = text[offset..].iter().rposition(|elt| *elt == x) {
            return Some(offset + index);
        }
    } else {
        offset = len;
    }

    // search the body of the text
    let repeated_x = repeat_byte(x);

    while offset >= 2 * USIZE_BYTES {
        unsafe {
            let u = *(ptr.offset(offset as isize - 2 * USIZE_BYTES as isize) as *const usize);
            let v = *(ptr.offset(offset as isize - USIZE_BYTES as isize) as *const usize);

            // break if there is a matching byte
            let zu = contains_zero_byte(u ^ repeated_x);
            let zv = contains_zero_byte(v ^ repeated_x);
            if zu || zv {
                break;
            }
        }
        offset -= 2 * USIZE_BYTES;
    }

    // find the byte after the point the body loop stopped
    text[..offset].iter().rposition(|elt| *elt == x)
}

#[test]
fn test_rfind_byte() {
    assert_eq!(rfind_byte(b'x', b""), None);

    let text = b"abcb";
    assert_eq!(rfind_byte(b'd', text), None);
    assert_eq!(rfind_byte(b'a', text), Some(0));
    assert_eq!(rfind_byte(b'c', text), Some(2));
    assert_eq!(rfind_byte(b'b', text), Some(3));

    let longer = "loAAer text and yo on, a bit zebras and zw";
    assert_eq!(rfind_byte(b'z', longer.as_bytes()), longer.rfind('z'));
    assert_eq!(rfind_byte(b'w', longer.as_bytes()), longer.rfind('w'));
    assert_eq!(rfind_byte(b'y', longer.as_bytes()), longer.rfind('y'));
    assert_eq!(rfind_byte(b'A', longer.as_bytes()), longer.rfind('A'));
}
