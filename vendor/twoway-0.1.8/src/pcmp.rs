//! SSE4.2 (pcmpestri) accelerated substring search
//!
//! Using the two way substring search algorithm.
// wssm word size string matching<br>
// wslm word size lexicographical maximum suffix
//

#![allow(dead_code)]

extern crate unchecked_index;
extern crate memchr;

use std::cmp;
use std::iter::Zip;
use std::ptr;

use self::unchecked_index::get_unchecked;

use TwoWaySearcher;

fn zip<I, J>(i: I, j: J) -> Zip<I::IntoIter, J::IntoIter>
    where I: IntoIterator,
          J: IntoIterator
{
    i.into_iter().zip(j)
}

/// `pcmpestri` flags
const EQUAL_ANY: u8 = 0b0000;
const EQUAL_EACH: u8 = 0b1000;
const EQUAL_ORDERED: u8 = 0b1100;

/// `pcmpestri`
///
/// “Packed compare explicit length strings (return index)”
///
/// PCMPESTRI xmm1, xmm2/m128, imm8
///
/// Return value: least index for start of (partial) match, (16 if no match).
#[inline(always)]
unsafe fn pcmpestri_16(text: *const u8, offset: usize, text_len: usize,
                       needle_1: u64, needle_2: u64, needle_len: usize) -> u32 {
    //debug_assert!(text_len + offset <= text.len()); // saturates at 16
    //debug_assert!(needle_len <= 16); // saturates at 16
    let res: u32;
    // 0xC = 12, Equal Ordered comparison
    //
    // movlhps xmm0, xmm1  Move low word of xmm1 to high word of xmm0
    asm!("movlhps $1, $2
          pcmpestri $1, [$3 + $4], $5"
         : // output operands
         "={ecx}"(res)
         : // input operands
         "x"(needle_1),        // operand 1 = needle  `x` = sse register
         "x"(needle_2),        // operand 1 = needle
         "r"(text), // operand 2 pointer = haystack
         "r"(offset),        // operand 2 offset
         "i"(EQUAL_ORDERED),
         "{rax}"(needle_len),// length of operand 1 = needle
         "{rdx}"(text_len)   // length of operand 2 = haystack
         : // clobbers
         "cc"
         : "intel" // options
    );
    res
}

/// `pcmpestrm`
///
/// “Packed compare explicit length strings (return mask)”
///
/// PCMPESTRM xmm1, xmm2/m128, imm8
///
/// Return value: bitmask in the 16 lsb of the return value.
#[inline(always)]
unsafe fn pcmpestrm_eq_each(text: *const u8, offset: usize, text_len: usize,
                            needle: *const u8, noffset: usize, needle_len: usize) -> u64 {
    // NOTE: text *must* be readable for 16 bytes
    // NOTE: needle *must* be readable for 16 bytes
    //debug_assert!(text_len + offset <= text.len()); // saturates at 16
    //debug_assert!(needle_len <= 16); // saturates at 16
    let res: u64;
    // 0xC = 12, Equal Ordered comparison
    //
    // movlhps xmm0, xmm1  Move low word of xmm1 to high word of xmm0
    asm!("movdqu xmm0, [$1 + $2]
          pcmpestrm xmm0, [$3 + $4], $5"
         : // output operands
         "={xmm0}"(res)
         : // input operands
         "r"(needle),         // operand 1 = needle
         "r"(noffset),        // operand 1 = needle offset
         "r"(text), // operand 2 pointer = haystack
         "r"(offset),        // operand 2 offset
         "i"(EQUAL_EACH),
         "{rax}"(needle_len),// length of operand 1 = needle
         "{rdx}"(text_len)   // length of operand 2 = haystack
         : // clobbers
         "cc"
         : "intel" // options
    );
    res
}


/// Return critical position, period.
/// critical position is zero-based
///
/// Note: If the period is long, the correct period is not returned.
/// The approximation to a long period must be computed separately.
#[inline(never)]
fn crit_period(pat: &[u8]) -> (usize, usize) {
    let (i, p) = TwoWaySearcher::maximal_suffix(pat, false);
    let (j, q) = TwoWaySearcher::maximal_suffix(pat, true);
    if i >= j {
        (i, p)
    } else {
        (j, q)
    }
}

/// Search for first possible match of `pat` -- might be just a byte
/// Return `(pos, length)` length of match
#[cfg(test)]
fn first_start_of_match(text: &[u8], pat: &[u8]) -> Option<(usize, usize)> {
    // not safe for text that is non aligned and ends at page boundary
    let patl = pat.len();
    assert!(patl <= 16);
    // load pat as a little endian word
    let (patw1, patw2) = pat128(pat);
    first_start_of_match_inner(text, pat, patw1, patw2)
}

/// Safe wrapper around pcmpestri to find first match of `pat` in `text`.
/// `p1`, `p2` are the first two words of `pat` and *must* match.
/// Length given by length of `pat`, only first 16 bytes considered.
fn first_start_of_match_inner(text: &[u8], pat: &[u8], p1: u64, p2: u64) -> Option<(usize, usize)> {
    // align the text pointer
    let tp = text.as_ptr();
    let tp_align_offset = tp as usize & 0xF;
    let init_len;
    let tp_aligned;
    unsafe {
        if tp_align_offset != 0 {
            init_len = 16 - tp_align_offset;
            tp_aligned = tp.offset(-(tp_align_offset as isize));
        } else {
            init_len = 0;
            tp_aligned = tp;
        };
    }

    let patl = pat.len();
    debug_assert!(patl <= 16);

    let mut offset = 0;

    // search the unaligned prefix first
    if init_len > 0 {
        for start in 0..cmp::min(init_len, text.len()) {
            if text[start] != pat[0] {
                continue;
            }
            let mut mlen = 1;
            for (&a, &b) in zip(&text[start + 1..], &pat[1..]) {
                if a != b {
                    mlen = 0;
                    break;
                }
                mlen += 1;
            }
            return Some((start, mlen))
        }
        offset += 16;
    }
    while text.len() >= offset - tp_align_offset + patl {
        unsafe {
            let tlen = text.len() - (offset - tp_align_offset);
            let ret = pcmpestri_16(tp_aligned, offset, tlen, p1, p2, patl) as usize;
            if ret == 16 {
                offset += 16;
            } else {
                let match_len = cmp::min(patl, 16 - ret);
                return Some((offset - tp_align_offset + ret, match_len));
            }
        }
    }

    None
}

/// safe to search unaligned for first start of match
///
/// unsafe because the end of text must not be close (within 16 bytes) of a page boundary
unsafe fn first_start_of_match_unaligned(text: &[u8], pat_len: usize, p1: u64, p2: u64) -> Option<(usize, usize)> {
    let tp = text.as_ptr();
    debug_assert!(pat_len <= 16);
    debug_assert!(pat_len <= text.len());

    let mut offset = 0;

    while text.len() - pat_len >= offset {
        let tlen = text.len() - offset;
        let ret = pcmpestri_16(tp, offset, tlen, p1, p2, pat_len) as usize;
        if ret == 16 {
            offset += 16;
        } else {
            let match_len = cmp::min(pat_len, 16 - ret);
            return Some((offset + ret, match_len));
        }
    }

    None
}

#[test]
fn test_first_start_of_match() {
    let text = b"abc";
    let longer = "longer text and so on";
    assert_eq!(first_start_of_match(text, b"d"), None);
    assert_eq!(first_start_of_match(text, b"c"), Some((2, 1)));
    assert_eq!(first_start_of_match(text, b"abc"), Some((0, 3)));
    assert_eq!(first_start_of_match(text, b"T"), None);
    assert_eq!(first_start_of_match(text, b"\0text"), None);
    assert_eq!(first_start_of_match(text, b"\0"), None);

    // test all windows
    for wsz in 1..17 {
        for window in longer.as_bytes().windows(wsz) {
            let str_find = longer.find(::std::str::from_utf8(window).unwrap());
            assert!(str_find.is_some());
            let first_start = first_start_of_match(longer.as_bytes(), window);
            assert!(first_start.is_some());
            let (pos, len) = first_start.unwrap();
            assert!(len <= wsz);
            assert!(len == wsz && Some(pos) == str_find
                    || pos <= str_find.unwrap());
        }
    }
}

fn find_2byte_pat(text: &[u8], pat: &[u8]) -> Option<(usize, usize)> {
    debug_assert!(text.len() >= pat.len());
    debug_assert!(pat.len() == 2);
    // Search for the second byte of the pattern, not the first, better for
    // scripts where we have two-byte encoded codepoints (the first byte will
    // repeat much more often than the second).
    let mut off = 1;
    while let Some(i) = memchr::memchr(pat[1], &text[off..]) {
        match text.get(off + i - 1) {
            None => break,
            Some(&c) if c == pat[0] => return Some((off + i - 1, off + i + 1)),
            _ => off += i + 1,
        }

    }
    None
}

/// Simd text search optimized for short patterns (<= 8 bytes)
fn find_short_pat(text: &[u8], pat: &[u8]) -> Option<usize> {
    debug_assert!(pat.len() <= 8);
    /*
    if pat.len() == 2 {
        return find_2byte_pat(text, pat);
    }
    */
    let (r1, _) = pat128(pat);

    // safe part of text -- everything but the last 16 bytes
    let safetext = &text[..cmp::max(text.len(), 16) - 16];

    let mut pos = 0;
    'search: loop {
        if pos + pat.len() > safetext.len() {
            break;
        }
        // find the next occurence
        match unsafe { first_start_of_match_unaligned(&safetext[pos..], pat.len(), r1, 0) } {
            None => break, // no matches
            Some((mpos, mlen)) => {
                pos += mpos;
                if mlen < pat.len() {
                    if pos > text.len() - pat.len() {
                        return None;
                    }
                    for (&a, &b) in zip(&text[pos + mlen..], &pat[mlen..]) {
                        if a != b {
                            pos += 1;
                            continue 'search;
                        }
                    }
                }

                return Some(pos);
            }
        }
    }

    'tail: loop {
        if pos > text.len() - pat.len() {
            return None;
        }
        // find the next occurence
        match first_start_of_match_inner(&text[pos..], pat, r1, 0) {
            None => return None, // no matches
            Some((mpos, mlen)) => {
                pos += mpos;
                if mlen < pat.len() {
                    if pos > text.len() - pat.len() {
                        return None;
                    }
                    for (&a, &b) in zip(&text[pos + mlen..], &pat[mlen..]) {
                        if a != b {
                            pos += 1;
                            continue 'tail;
                        }
                    }
                }

                return Some(pos);
            }
        }
    }
}

/// `find` finds the first ocurrence of `pattern` in the `text`.
///
/// This is the SSE42 accelerated version.
pub fn find(text: &[u8], pattern: &[u8]) -> Option<usize> {
    let pat = pattern;
    if pat.len() == 0 {
        return Some(0);
    }

    if text.len() < pat.len() {
        return None;
    }

    if pat.len() == 1 {
        return memchr::memchr(pat[0], text);
    } else if pat.len() <= 6 {
        return find_short_pat(text, pat);
    }

    // real two way algorithm
    //

    // `memory` is the number of bytes of the left half that we already know
    let (crit_pos, mut period) = crit_period(pat);
    let mut memory;

    if &pat[..crit_pos] == &pat[period.. period + crit_pos] {
        memory = 0; // use memory
    } else {
        memory = !0; // !0 means memory is unused
        // approximation to the true period
        period = cmp::max(crit_pos, pat.len() - crit_pos) + 1;
    }

    //println!("pat: {:?}, crit={}, period={}", pat, crit_pos, period);
    let (left, right) = pat.split_at(crit_pos);
    let (right16, _right17) = right.split_at(cmp::min(16, right.len()));
    assert!(right.len() != 0);

    let (r1, r2) = pat128(right);

    // safe part of text -- everything but the last 16 bytes
    let safetext = &text[..cmp::max(text.len(), 16) - 16];

    let mut pos = 0;
    if memory == !0 {
        // Long period case -- no memory, period is an approximation
        'search: loop {
            if pos + pat.len() > safetext.len() {
                break;
            }
            // find the next occurence of the right half
            let start = crit_pos;
            match unsafe { first_start_of_match_unaligned(&safetext[pos + start..], right16.len(), r1, r2) } {
                None => break, // no matches
                Some((mpos, mlen)) => {
                    pos += mpos;
                    let mut pfxlen = mlen;
                    if pfxlen < right.len() {
                        pfxlen += shared_prefix(&text[pos + start + mlen..], &right[mlen..]);
                    }
                    if pfxlen != right.len() {
                        // partial match
                        // skip by the number of bytes matched
                        pos += pfxlen + 1;
                        continue 'search;
                    } else {
                        // matches right part
                    }
                }
            }

            // See if the left part of the needle matches
            // XXX: Original algorithm compares from right to left here
            if left != &text[pos..pos + left.len()] {
                pos += period;
                continue 'search;
            }

            return Some(pos);
        }
    } else {
        // Short period case -- use memory, true period
        'search_memory: loop {
            if pos + pat.len() > safetext.len() {
                break;
            }
            // find the next occurence of the right half
            //println!("memory trace pos={}, memory={}", pos, memory);
            let mut pfxlen = if memory == 0 {
                let start = crit_pos;
                match unsafe { first_start_of_match_unaligned(&safetext[pos + start..], right16.len(), r1, r2) } {
                    None => break, // no matches
                    Some((mpos, mlen)) => {
                        pos += mpos;
                        mlen
                    }
                }
            } else {
                memory - crit_pos
            };
            if pfxlen < right.len() {
                pfxlen += shared_prefix(&text[pos + crit_pos + pfxlen..], &right[pfxlen..]);
            }
            if pfxlen != right.len() {
                // partial match
                // skip by the number of bytes matched
                pos += pfxlen + 1;
                memory = 0;
                continue 'search_memory;
            } else {
                // matches right part
            }

            // See if the left part of the needle matches
            // XXX: Original algorithm compares from right to left here
            if memory <= left.len() && &left[memory..] != &text[pos + memory..pos + left.len()] {
                pos += period;
                memory = pat.len() - period;
                continue 'search_memory;
            }

            return Some(pos);
        }
    }

    // no memory used for final part
    'tail: loop {
        if pos > text.len() - pat.len() {
            return None;
        }
        // find the next occurence of the right half
        let start = crit_pos;
        match first_start_of_match_inner(&text[pos + start..], right16, r1, r2) {
            None => return None, // no matches
            Some((mpos, mlen)) => {
                pos += mpos;
                let mut pfxlen = mlen;
                if pfxlen < right.len() {
                    pfxlen += shared_prefix(&text[pos + start + mlen..], &right[mlen..]);
                }
                if pfxlen != right.len() {
                    // partial match
                    // skip by the number of bytes matched
                    pos += pfxlen + 1;
                    continue 'tail;

                } else {
                    // matches right part
                }
            }
        }

        // See if the left part of the needle matches
        // XXX: Original algorithm compares from right to left here
        if left != &text[pos..pos + left.len()] {
            pos += period;
            continue 'tail;
        }

        return Some(pos);
    }
}

#[test]
fn test_find() {
    let text = b"abc";
    assert_eq!(find(text, b"d"), None);
    assert_eq!(find(text, b"c"), Some(2));

    let longer = "longer text and so on, a bit more";

    // test all windows
    for wsz in 1..longer.len() {
        for window in longer.as_bytes().windows(wsz) {
            let str_find = longer.find(::std::str::from_utf8(window).unwrap());
            assert!(str_find.is_some());
            assert_eq!(find(longer.as_bytes(), window), str_find);
        }
    }

    let pat = b"ger text and so on";
    assert!(pat.len() > 16);
    assert_eq!(Some(3), find(longer.as_bytes(), pat));

    // test short period case

    let text = "cbabababcbabababab";
    let n = "abababab";
    assert_eq!(text.find(n), find(text.as_bytes(), n.as_bytes()));

    // memoized case -- this is tricky
    let text = "cbababababababababababababababab";
    let n = "abababab";
    assert_eq!(text.find(n), find(text.as_bytes(), n.as_bytes()));

}

/// Load the first 16 bytes of `pat` into two words, little endian
fn pat128(pat: &[u8]) -> (u64, u64) {
    // load pat as a little endian word
    let (mut p1, mut p2) = (0, 0);
    unsafe {
        let patl = pat.len();
        ptr::copy_nonoverlapping(&pat[0],
                                 &mut p1 as *mut _ as *mut _,
                                 cmp::min(8, patl));

        if patl > 8 {
            ptr::copy_nonoverlapping(&pat[8],
                                     &mut p2 as *mut _ as *mut _,
                                     cmp::min(16, patl) - 8);

        }
    }
    (p1, p2)
}

/// Find longest shared prefix, return its length
/// 
/// Alignment safe: works for any text, pat.
pub fn shared_prefix(text: &[u8], pat: &[u8]) -> usize {
    let tp = text.as_ptr();
    let tlen = text.len();
    let pp = pat.as_ptr();
    let plen = pat.len();
    let len = cmp::min(tlen, plen);

    unsafe {
        // TODO: do non-aligned prefix manually too(?) aligned text or pat..
        // all but the end we can process with pcmpestrm
        let initial_part = len.saturating_sub(16);
        let mut prefix_len = 0;
        let mut offset = 0;
        while offset < initial_part {
            let initial_tail = initial_part - offset;
            let mask = pcmpestrm_eq_each(tp, offset, initial_tail, pp, offset, initial_tail);
            // find zero in the first 16 bits
            if mask != 0xffff {
                let first_bit_set = (mask ^ 0xffff).trailing_zeros() as usize;
                prefix_len += first_bit_set;
                return prefix_len;
            } else {
                prefix_len += cmp::min(initial_tail, 16);
            }
            offset += 16;
        }
        // so one block left, the last (up to) 16 bytes
        // unchecked slicing .. we don't want panics in this function
        let text_suffix = get_unchecked(text, prefix_len..len);
        let pat_suffix = get_unchecked(pat, prefix_len..len);
        for (&a, &b) in zip(text_suffix, pat_suffix) {
            if a != b {
                break;
            }
            prefix_len += 1;
        }

        prefix_len
    }
}

#[test]
fn test_prefixlen() {
    let text_long  = b"0123456789abcdefeffect";
    let text_long2 = b"9123456789abcdefeffect";
    let text_long3 = b"0123456789abcdefgffect";
    let plen = shared_prefix(text_long, text_long);
    assert_eq!(plen, text_long.len());
    let plen = shared_prefix(b"abcd", b"abc");
    assert_eq!(plen, 3);
    let plen = shared_prefix(b"abcd", b"abcf");
    assert_eq!(plen, 3);
    assert_eq!(0, shared_prefix(text_long, text_long2));
    assert_eq!(0, shared_prefix(text_long, &text_long[1..]));
    assert_eq!(16, shared_prefix(text_long, text_long3));

    for i in 0..text_long.len() + 1 {
        assert_eq!(text_long.len() - i, shared_prefix(&text_long[i..], &text_long[i..]));
    }

    let l1 = [7u8; 1024];
    let mut l2 = [7u8; 1024];
    let off = 1000;
    l2[off] = 0;
    for i in 0..off {
        let plen = shared_prefix(&l1[i..], &l2[i..]);
        assert_eq!(plen, off - i);
    }
}
