#![cfg(feature = "pattern")]
#![feature(pattern)]
#![allow(dead_code)]

extern crate twoway;

extern crate quickcheck;
extern crate itertools as it;
extern crate odds;
#[macro_use] extern crate macro_attr;
#[macro_use] extern crate newtype_derive;

mod quickchecks {

use twoway::{Str, StrSearcher};
use twoway::{
    find_str,
    rfind_str,
};
use it::{Itertools, unfold};

use std::str::pattern::{Pattern, Searcher, ReverseSearcher, SearchStep};
use std::str::pattern::SearchStep::{Match, Reject, Done};

use std::ops::Deref;

use odds::string::StrExt;

use quickcheck as qc;
use quickcheck::TestResult;
use quickcheck::Arbitrary;
use quickcheck::quickcheck;

#[derive(Copy, Clone, Debug)]
/// quickcheck Arbitrary adaptor - half the size of `T` on average
struct Short<T>(T);

impl<T> Deref for Short<T> {
    type Target = T;
    fn deref(&self) -> &T { &self.0 }
}

impl<T> Arbitrary for Short<T>
    where T: Arbitrary
{
    fn arbitrary<G: qc::Gen>(g: &mut G) -> Self {
        let sz = g.size() / 2;
        Short(T::arbitrary(&mut qc::StdGen::new(g, sz)))
    }

    fn shrink(&self) -> Box<Iterator<Item=Self>> {
        Box::new((**self).shrink().map(Short))
    }
}

macro_attr! {
    #[derive(Clone, Debug, NewtypeDeref!)]
    struct Text(String);
}

static ALPHABET: &'static str = "abñòαβ\u{3c72}";
static SIMPLEALPHABET: &'static str = "ab";

impl Arbitrary for Text {
    fn arbitrary<G: qc::Gen>(g: &mut G) -> Self {
        let len = u16::arbitrary(g);
        let mut s = String::with_capacity(len as usize);
        let alpha_len = ALPHABET.chars().count();
        for _ in 0..len {
            let i = usize::arbitrary(g);
            let i = i % alpha_len;
            s.push(ALPHABET.chars().nth(i).unwrap());
        }
        Text(s)
    }
    fn shrink(&self) -> Box<Iterator<Item=Self>> {
        Box::new(self.0.shrink().map(Text))
    }
}

/// Text from an alphabet of only two letters
macro_attr! {
    #[derive(Clone, Debug, NewtypeDeref!)]
    struct SimpleText(String);
}

impl Arbitrary for SimpleText {
    fn arbitrary<G: qc::Gen>(g: &mut G) -> Self {
        let len = u16::arbitrary(g);
        let mut s = String::with_capacity(len as usize);
        let alpha_len = SIMPLEALPHABET.chars().count();
        for _ in 0..len {
            let i = usize::arbitrary(g);
            let i = i % alpha_len;
            s.push(SIMPLEALPHABET.chars().nth(i).unwrap());
        }
        SimpleText(s)
    }
    fn shrink(&self) -> Box<Iterator<Item=Self>> {
        Box::new(self.0.shrink().map(SimpleText))
    }
}

#[derive(Clone, Debug)]
struct ShortText(String);
// Half the length of Text on average
impl Arbitrary for ShortText {
    fn arbitrary<G: qc::Gen>(g: &mut G) -> Self {
        let len = u16::arbitrary(g) / 2;
        let mut s = String::with_capacity(len as usize);
        let alpha_len = ALPHABET.chars().count();
        for _ in 0..len {
            let i = usize::arbitrary(g);
            let i = i % alpha_len;
            s.push(ALPHABET.chars().nth(i).unwrap());
        }
        ShortText(s)
    }
    fn shrink(&self) -> Box<Iterator<Item=Self>> {
        Box::new(self.0.shrink().map(ShortText))
    }
}

pub fn contains(hay: &str, n: &str) -> bool {
    Str(n).is_contained_in(hay)
}

pub fn find(hay: &str, n: &str) -> Option<usize> {
    Str(n).into_searcher(hay).next_match().map(|(a, _)| a)
}

pub fn contains_rev(hay: &str, n: &str) -> bool {
    let mut tws = StrSearcher::new(hay, n);
    loop {
        match tws.next_back() {
            SearchStep::Done => return false,
            SearchStep::Match(..) => return true,
            _ => { }
        }
    }
}

pub fn rfind(hay: &str, n: &str) -> Option<usize> {
    Str(n).into_searcher(hay).next_match_back().map(|(a, _)| a)
}

#[test]
fn test_contains() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let truth = a.contains(b);
        TestResult::from_bool(contains(&a, &b) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_contains_rev() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let truth = a.contains(b);
        TestResult::from_bool(contains_rev(&a, &b) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_find_str() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let truth = a.find(b);
        TestResult::from_bool(find_str(&a, &b) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_rfind_str() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let truth = a.rfind(b);
        TestResult::from_bool(rfind_str(&a, &b) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_contains_plus() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        //let b = &b.0;
        if b.len() == 0 { return TestResult::discard() }
        let truth = a.contains(b);
        TestResult::from_bool(contains(&a, &b) == truth &&
            (!truth || b.substrings().all(|sub| contains(&a, sub))))
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_contains_rev_plus() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        if b.len() == 0 { return TestResult::discard() }
        let truth = a.contains(b);
        TestResult::from_bool(contains_rev(&a, &b) == truth &&
            (!truth || b.substrings().all(|sub| contains_rev(&a, sub))))
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_starts_with() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let truth = a.starts_with(b);
        TestResult::from_bool(Str(b).is_prefix_of(a) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_ends_with() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let truth = a.ends_with(b);
        TestResult::from_bool(Str(b).is_suffix_of(a) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_next_reject() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let truth = b.into_searcher(a).next_reject().map(|(a, _)| a);
        TestResult::from_bool(Str(b).into_searcher(a).next_reject().map(|(a, _)| a) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_next_reject_back() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let truth = b.into_searcher(a).next_reject_back().map(|(_, b)| b);
        TestResult::from_bool(Str(b).into_searcher(a).next_reject_back().map(|(_, b)| b) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

fn coalesce_rejects(a: SearchStep, b: SearchStep)
    -> Result<SearchStep, (SearchStep, SearchStep)>
{
    match (a, b) {
        (SearchStep::Reject(a, b), SearchStep::Reject(c, d)) => {
            assert_eq!(b, c);
            Ok(SearchStep::Reject(a, d))
        }
        otherwise => Err(otherwise),
    }
}

fn coalesce_intervals(a: Option<(usize, usize)>, b: Option<(usize, usize)> )
    -> Result<Option<(usize, usize)>, (Option<(usize, usize)>, Option<(usize, usize)>)>
{
    match (a, b) {
        (Some((x, y)), Some((w, z))) => {
            assert_eq!(y, w);
            Ok(Some((x, z)))
        }
        otherwise => Err(otherwise),
    }
}

// Test that all search steps are contiguous
#[test]
fn test_search_steps() {
    fn prop(a: Text, b: Text) -> bool {
        let hay = &a.0;
        let n = &b.0;
        let tws = StrSearcher::new(hay, n);
        // Make sure it covers the whole string
        let mut search_steps = unfold(tws, |mut tws| {
            match tws.next() {
                SearchStep::Done => None,
                otherwise => Some(otherwise),
            }
        }).map(|step| match step {
            SearchStep::Match(a, b) | SearchStep::Reject(a, b) => Some((a, b)),
            SearchStep::Done => None,
        }).coalesce(coalesce_intervals);
        match search_steps.next() {
            None => hay.len() == 0,
            Some(None) => true,
            Some(Some((a, b))) => {
                //println!("Next step would be: {:?}", search_steps.next());
                assert_eq!((a, b), (0, hay.len()));
                true
            }
        }
        //assert_eq!(search_steps.next(), Some(SearchStep::Match(0, a.len())));
        //true
        //search_steps.next() == Some(SearchStep::Match(0, a.len()))    }
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_search_steps_rev() {
    fn prop(a: Text, b: Text) -> bool {
        let hay = &a.0;
        let n = &b.0;
        let tws = StrSearcher::new(hay, n);
        // Make sure it covers the whole string
        let mut search_steps = unfold(tws, |mut tws| {
            match tws.next_back() {
                SearchStep::Done => None,
                otherwise => Some(otherwise),
            }
        }).map(|step| match step {
            SearchStep::Match(a, b) | SearchStep::Reject(a, b) => Some((a, b)),
            SearchStep::Done => None,
        }).coalesce(|a, b| match coalesce_intervals(b, a) {
            // adaption for reverse order
            Ok(c) => Ok(c),
            Err((d, e)) => Err((e, d)),
        });
        match search_steps.next() {
            None => hay.len() == 0,
            Some(None) => true,
            Some(Some((a, b))) => {
                //println!("Next step would be: {:?}", search_steps.next());
                assert_eq!((a, b), (0, hay.len()));
                true
            }
        }
        //assert_eq!(search_steps.next(), Some(SearchStep::Match(0, a.len())));
        //true
        //search_steps.next() == Some(SearchStep::Match(0, a.len()))    }
    }
    quickcheck(prop as fn(_, _) -> _);
}

// Test that all search steps are well formed
#[test]
fn test_search_steps_wf() {
    fn prop(a: Text, b: Text) -> bool {
        let hay = &a.0;
        let n = &b.0;
        let mut tws = StrSearcher::new(hay, n);
        let mut rejects_seen = 0; // n rejects seen since last match
        let test_single_rejects = false;
        let test_utf8_boundaries = true;
        loop {
            match tws.next() {
                Reject(a, b) => {
                    assert!(!test_single_rejects || rejects_seen == 0);
                    assert!(!test_utf8_boundaries || (hay.is_char_boundary(a) && hay.is_char_boundary(b)));
                    rejects_seen += 1;
                    assert!(a != b, "Reject({}, {}) is zero size", a, b);
                }
                Match(a, b) => {
                    assert_eq!(b - a, n.len());
                    assert!(!test_single_rejects || rejects_seen <= 1, "rejects_seen={}", rejects_seen);
                    rejects_seen = 0;
                }
                Done => {
                    assert!(!test_single_rejects || rejects_seen <= 1, "rejects_seen={}", rejects_seen);
                    break;
                }
            }
        }
        true
    }
    quickcheck(prop as fn(_, _) -> _);
}

// Test that all search steps are well formed
#[test]
fn test_search_steps_wf_rev() {
    fn prop(a: Text, b: Text) -> bool {
        let hay = &a.0;
        let n = &b.0;
        let mut tws = StrSearcher::new(hay, n);
        let mut rejects_seen = 0; // n rejects seen since last match
        let test_single_rejects = false;
        let test_utf8_boundaries = true;
        loop {
            match tws.next_back() {
                Reject(a, b) => {
                    assert!(!test_utf8_boundaries || (hay.is_char_boundary(a) && hay.is_char_boundary(b)));
                    assert!(!test_single_rejects || rejects_seen == 0);
                    rejects_seen += 1;
                    assert!(a != b, "Reject({}, {}) is zero size", a, b);
                }
                Match(a, b) => {
                    assert_eq!(b - a, n.len());
                    assert!(!test_single_rejects || rejects_seen <= 1, "rejects_seen={}", rejects_seen);
                    rejects_seen = 0;
                }
                Done => {
                    assert!(!test_single_rejects || rejects_seen <= 1, "rejects_seen={}", rejects_seen);
                    break;
                }
            }
        }
        true
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_contains_substrings() {
    fn prop(s: (char, char, char, char)) -> bool {
        let mut ss = String::new();
        ss.push(s.0);
        ss.push(s.1);
        ss.push(s.2);
        ss.push(s.3);
        let a = &ss;
        for sub in a.substrings() {
            assert!(a.contains(sub));
            if !contains(a, sub) {
                return false;
            }
        }
        true
    }
    quickcheck(prop as fn(_) -> _);
}

#[test]
fn test_contains_substrings_rev() {
    fn prop(s: (char, char, char, char)) -> bool {
        let mut ss = String::new();
        ss.push(s.0);
        ss.push(s.1);
        ss.push(s.2);
        ss.push(s.3);
        let a = &ss;
        for sub in a.substrings() {
            assert!(a.contains(sub));
            if !contains_rev(a, sub) {
                return false;
            }
        }
        true
    }
    quickcheck(prop as fn(_) -> _);
}

#[test]
fn test_find_period() {
    fn prop(a: SimpleText, b: Short<SimpleText>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let pat = [b, b].concat();
        let truth = a.find(&pat);
        TestResult::from_bool(find(a, &pat) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[test]
fn test_find_rev_period() {
    fn prop(a: SimpleText, b: Short<SimpleText>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let pat = [b, b].concat();
        let truth = a.rfind(&pat);
        TestResult::from_bool(rfind(a, &pat) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}


#[cfg(feature = "pcmp")]
// pcmpestr tests
#[test]
fn test_pcmp_contains() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let truth = a.contains(b);
        TestResult::from_bool(::twoway::pcmp::find(a.as_bytes(), b.as_bytes()).is_some() == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[cfg(feature = "pcmp")]
#[test]
fn test_pcmp_contains_plus() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let contains = |a:&str, b:&str| ::twoway::pcmp::find(a.as_bytes(), b.as_bytes()).is_some();
        let a = &a.0;
        let b = &b[..];
        //let b = &b.0;
        if b.len() == 0 { return TestResult::discard() }
        let truth = a.contains(b);
        TestResult::from_bool(contains(&a, &b) == truth &&
            (!truth || b.substrings().all(|sub| contains(&a, sub))))
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[cfg(feature = "pcmp")]
// pcmpestr tests
#[test]
fn test_pcmp_find() {
    fn prop(a: Text, b: Short<Text>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let truth = a.find(b);
        TestResult::from_bool(::twoway::pcmp::find(a.as_bytes(), b.as_bytes()) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[cfg(feature = "pcmp")]
// pcmpestr tests
#[test]
fn test_pcmp_find_simple() {
    fn prop(a: SimpleText, b: Short<SimpleText>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let truth = a.find(b);
        TestResult::from_bool(::twoway::pcmp::find(a.as_bytes(), b.as_bytes()) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[cfg(feature = "pcmp")]
// pcmpestr tests
#[test]
fn test_pcmp_find_period() {
    fn prop(a: SimpleText, b: Short<SimpleText>) -> TestResult {
        let a = &a.0;
        let b = &b[..];
        let pat = [b, b].concat();
        let truth = a.find(&pat);
        TestResult::from_bool(::twoway::pcmp::find(a.as_bytes(), pat.as_bytes()) == truth)
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[cfg(feature = "test-set")]
#[test]
fn test_find_byte() {
    fn prop(v: Vec<u8>, offset: u8) -> bool {
        use twoway::set::find_byte as memchr;

        // test all pointer alignments
        let uoffset = (offset & 0xF) as usize;
        let data = if uoffset <= v.len() {
            &v[uoffset..]
        } else {
            &v[..]
        };
        for byte in 0..256u32 {
            let byte = byte as u8;
            if memchr(byte, &data) != data.iter().position(|elt| *elt == byte) {
                return false;
            }
        }
        true
    }
    quickcheck(prop as fn(_, _) -> _);
}

#[cfg(feature = "test-set")]
#[test]
fn test_rfind_byte() {
    fn prop(v: Vec<u8>, offset: u8) -> bool {
        use twoway::set::rfind_byte as memrchr;

        // test all pointer alignments
        let uoffset = (offset & 0xF) as usize;
        let data = if uoffset <= v.len() {
            &v[uoffset..]
        } else {
            &v[..]
        };
        for byte in 0..256u32 {
            let byte = byte as u8;
            if memrchr(byte, &data) != data.iter().rposition(|elt| *elt == byte) {
                return false;
            }
        }
        true
    }
    quickcheck(prop as fn(_, _) -> _);
}
}
