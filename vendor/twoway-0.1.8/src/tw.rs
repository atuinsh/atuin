//! 
//!
//! Two-way string matching
//!
//! http://monge.univ-mlv.fr/~mac/Articles-PDF/CP-1991-jacm.pdf
#![allow(dead_code)]
#![cfg(test)]

use std::str;
use std::cmp::max;
use std::ops::Index;

#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Str<'a>(pub &'a [u8]);

impl<'a> Str<'a> {
    fn len(&self) -> usize { self.0.len() }
    fn as_str(&self) -> &str { str::from_utf8(self.0).unwrap() }
}

impl<'a> Index<usize> for Str<'a> {
    type Output = u8;
    fn index(&self, ix: usize) -> &u8 {
        &self.0[ix - 1]
    }
}

// A note on a simple computation of the maximal suffix of a string
//  simpler, slower version of duval's algorithm
fn compute_max_suf_pos(w: &[u8]) -> usize {
    // using 0-based indexing throughout
    let n = w.len();
    let mut i = 0;
    let mut j = 1;
    while j < n {
        let mut k = 0;
        while j + k < n - 1 && w[i + k] == w[j + k] {
            k += 1;
        }
        if w[i + k] < w[j + k] {
            i += k + 1;
        } else {
            j += k + 1;
        }
        if i == j {
            j += 1;
        }
    }
    i
}

/// Jewels of Stringology: Text Algorithms
/// Maxsuf-and-Period(x)
fn maxsuf_and_period(x: &[u8]) -> (usize, usize) {
    let n = x.len();

    let mut s = 1;
    let mut i = 2;
    let mut p = 1;

    while i <= n {
        let r = (i - s) % p;
        if x[i - 1] == x[s + r - 1] {
            i += 1;
        } else if x[i - 1] < x[s + r - 1] {
            i += 1;
            p = i - s;
        } else {
            s = i - r;
            i = s;
            p = 1;
        }
    }
    (s - 1, p)
}

/// From paper on Two-way string matching
/// Return index, period
/// The returned index is zero-based!
fn maximal_suffix(x: Str, rev: bool) -> (usize, usize) {
    let n = x.len();

    let mut i = 0;
    let mut j = 1;
    let mut k = 1;
    let mut p = 1;

    while j + k <= n {
        let ap = x[i + k];
        let a = x[j + k];
        /*
        println!("trace: x={}, i={}, j={}, k={}, ap={}/{}, a={}/{}",
                 x.as_str(), i, j, k, ap as char, ap, a as char, a);
        */
        if (a < ap && !rev) || (a > ap && rev) {
            j += k;
            k = 1;
            p = j - i;
        } else if a == ap {
            if k == p {
                j += p;
                k = 1;
            } else {
                k += 1;
            }
        } else {
            i = j;
            j = i + 1;
            k = 1;
            p = 1;
        }
    }
    //println!("trace: x={}, i={}, p={}", x.as_str(), i, p);

    (i, p)
}

/// Return critical position, period.
/// critical position is zero-based
pub fn crit_period(x: Str) -> (usize, usize) {
    let (i, p) = maximal_suffix(x, false);
    let (j, q) = maximal_suffix(x, true);
    if i >= j {
        (i, p)
    } else {
        (j, q)
    }
}

/// From paper on Two-way string matching
///
/// **x**: pattern,
/// **t**: text,
pub fn find(x: Str, t: Str) {
    let n = x.len();
    // `l` critical position, zero-based
    // `p` period
    let (l, p) = crit_period(x);
    println!("crit, period = {:?}", (l, p));
    // x[1..l] is a suffix of x[l + 1..l + p]
    if l < n / 2 && x.0[..l] == x.0[p..p + l] {
        println!("short period case");
        let mut pos = 0;
        let mut s = 0; // `s` is the memory
        while pos + n <= t.len() {
            let mut i = max(l, s) + 1;
            while i <= n && x[i] == t[pos + i] {
                i += 1;
            }
            if i <= n {
                println!("vars i={}, l={}, s={}, p={}", i, l, s, p);
                pos += max(i - l, 1 + max(s, p) - p);
                s = 0;
            } else {
                let mut j = l;
                while j > s && x[j] == t[pos + j] {
                    j -= 1;
                }
                if j <= s {
                    println!("pos={} is a match!", pos);
                }
                pos += p;
                s = n - p;
            }
        }
    } else {
        println!("long period case");
        let q = max(l, n - l) + 1;
        let mut pos = 0;
        while pos + n <= t.len() {
            let mut i = l + 1;
            println!("vars i={}, l={}, p={}, pos={}", i, l, p, pos);
            while i <= n && x[i] == t[pos + i] {
                i += 1;
            }
            if i <= n {
                pos += i - l;
            } else {
                let mut j = l;
                while j > 0 && x[j] == t[pos + j] {
                    j -= 1;
                }
                if j == 0 {
                    // pos is a zero-based index..
                    println!("pos={} is a match", pos);
                }
                pos += q;
            }
        }
    }
}

pub fn find_(x: &str, y: &str) {
    find(Str(x.as_bytes()),
         Str(y.as_bytes()))
}

/// **x**: pattern,
/// **t**: text,
///
/// return the index of the first match (0-based index)
pub fn find_first(x: Str, t: Str) -> Option<usize> {
    let n = x.len();
    let (l, p) = crit_period(x);
    // x[1..l] is a suffix of x[l + 1..l + p]
    if l < n / 2 && x.0[..l] == x.0[p..p + l] {
        // short period case
        let mut pos = 0;
        let mut s = 0; // `s` is the memory
        while pos + n <= t.len() {
            let mut i = max(l, s) + 1;
            while i <= n && x[i] == t[pos + i] {
                i += 1;
            }
            if i <= n {
                pos += max(i - l, 1 + max(s, p) - p);
                s = 0;
            } else {
                let mut j = l;
                while j > s && x[j] == t[pos + j] {
                    j -= 1;
                }
                if j <= s {
                    return Some(pos);
                }
                pos += p;
                s = n - p;
            }
        }
    } else {
        // long period case
        let q = max(l, n - l) + 1;
        let mut pos = 0;
        while pos + n <= t.len() {
            let mut i = l + 1;
            while i <= n && x[i] == t[pos + i] {
                i += 1;
            }
            if i <= n {
                pos += i - l;
            } else {
                let mut j = l;
                while j > 0 && x[j] == t[pos + j] {
                    j -= 1;
                }
                if j == 0 {
                    return Some(pos);
                }
                pos += q;
            }
        }
    }
    None
}

#[test]
fn test_max() {
    assert_eq!((2, 1), maximal_suffix(Str(b"aab"), false));
    assert_eq!((0, 3), maximal_suffix(Str(b"aab"), true));

    assert_eq!((0, 3), maximal_suffix(Str(b"aabaa"), true));
    assert_eq!((2, 3), maximal_suffix(Str(b"aabaa"), false));

    assert_eq!((0, 7), maximal_suffix(Str(b"gcagagag"), false));
    assert_eq!((2, 2), maximal_suffix(Str(b"gcagagag"), true));
    assert_eq!((2, 2), crit_period(Str(b"gcagagag")));

    assert_eq!((2, 1), crit_period(Str(b"aab")));
    assert_eq!((2, 3), crit_period(Str(b"aabaa")));

    assert_eq!((2, 4), crit_period(Str(b"abaaaba")));

    // both of these factorizations are critial factorizations
    assert_eq!((2, 2), crit_period(Str(b"banana")));
    assert_eq!((1, 2), crit_period(Str(b"zanana")));

    assert_eq!((10, 1), crit_period(Str(b"caaaaaaaaacc")));

    assert_eq!((2, 3), crit_period(Str(b"aabaab")));
    assert_eq!((1, 3), crit_period(Str(b"baabaa")));

    assert_eq!((2, 3), crit_period(Str(b"babbab")));

    // NOTE: returns "long period" case per = 2, which is an approximation
    assert_eq!((2, 2), crit_period(Str(b"abca")));
    assert_eq!((1, 3), crit_period(Str(b"acba")));
}
#[test]
fn test_find() {
    find_("aab", "aaable");
    find_("aab", "aaabaabe");
    find_("aaaa", "boyaaarateaaaade");
    assert_eq!(Some(10), find_first(Str(b"aaaa"), Str(b"boyaaarateaaaade")));

    find_("aab", "bbbbbbbbb");
    find_("aabaa", "aabaaabaaabaa");

    assert_eq!(find_first(Str(b"ababab"), Str(b"cbcbabababab")), Some(4));
    // this was a bug (typo) in the short period case
    assert_eq!(find_first(Str(b"aabaab"), Str(b"abbaab")), None);
}

#[test]
fn test_max_suf_pos() {
    assert_eq!(2, compute_max_suf_pos((b"aab")));

    assert_eq!(2, compute_max_suf_pos((b"aabaa")));

    assert_eq!(0, compute_max_suf_pos((b"gcagagag")));
    assert_eq!(2, compute_max_suf_pos((b"banana")));
}

#[test]
fn test_maxsuf_and_period() {
    assert_eq!((2, 1), maxsuf_and_period((b"aab")));
    assert_eq!((2, 3), maxsuf_and_period((b"aabaa")));
    assert_eq!((0, 7), maxsuf_and_period((b"gcagagag")));
}

/*
#[test]
fn slow() {
    let needle   = (0..100).map(|_| "b").collect::<String>();
    let heystack = (0..100_000).map(|_| "a").collect::<String>();

    println!("Data ready.");

    for _ in 0..100 {
        if find_first(Str(heystack.as_bytes()), Str(needle.as_bytes())).is_none() {
            // Stuff...
        }
    }
}
*/
