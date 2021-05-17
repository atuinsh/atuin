//! Boyer-Moore-Horspool
//!

extern crate memchr;
use std::cmp;

fn bmh_skip(pat: &[u8], skip: &mut [u16; 256]) {
    let pat_skip = cmp::min(pat.len(), u16::max_value() as usize) as u16;
    for entry in skip.iter_mut() {
        *entry = pat_skip;
    }

    for (index, &byte) in pat[..pat.len() - 1].iter().enumerate() {
        skip[byte as usize] = cmp::min(pat.len() - index - 1, u16::max_value() as usize) as u16;
    }
}

/// Boyer-Moore-Horspool substring search
pub fn find(text: &[u8], pat: &[u8]) -> Option<usize> {
    let mut skip = [0; 256];
    bmh_skip(pat, &mut skip);

    let pat_len = pat.len();

    if pat_len == 0 {
        return Some(0);
    }

    let pat_len_m1 = pat_len - 1;
    let pat_last = pat[pat_len - 1];

    // initial search by memchr
    let mut j = match memchr::memchr(pat[0], text) {
        Some(x) => x,
        None => return None,
    };

    while let Some(&c) = text.get(j + pat_len_m1) {
        // check the back character of the pattern
        if c == pat_last && &text[j..j + pat_len] == pat {
            return Some(j);
        }
        j += skip[c as usize] as usize;
    }
    None
}

#[test]
fn bmh_preprocess() {
    let mut skip = [0; 256];
    let needle = b"gcagagag";
    bmh_skip(needle, &mut skip);
    assert_eq!(skip[b'g' as usize], 2);
    assert_eq!(skip[b'c' as usize], 6);
    assert_eq!(skip[b'a' as usize], 1);
    assert_eq!(skip[b't' as usize], 8);
}

#[test]
fn bmh_find() {
    let text = b"abc";
    assert_eq!(find(text, b"d"), None);
    assert_eq!(find(text, b"c"), Some(2));

    let longer = "longer text and so on";

    // test all windows
    for wsz in 1..17 {
        for window in longer.as_bytes().windows(wsz) {
            let str_find = longer.find(::std::str::from_utf8(window).unwrap());
            assert!(str_find.is_some());
            assert_eq!(find(longer.as_bytes(), window), str_find);
        }
    }

    let pat = b"ger text and so on";
    assert!(pat.len() > 16);
    assert_eq!(Some(3), find(longer.as_bytes(), pat));
}
