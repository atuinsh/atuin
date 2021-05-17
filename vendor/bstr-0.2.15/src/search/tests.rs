use search::twoway::TwoWay;

/// Each test is a (needle, haystack, expected_fwd, expected_rev) tuple.
type SearchTest = (&'static str, &'static str, Option<usize>, Option<usize>);

const SEARCH_TESTS: &'static [SearchTest] = &[
    ("", "", Some(0), Some(0)),
    ("", "a", Some(0), Some(1)),
    ("", "ab", Some(0), Some(2)),
    ("", "abc", Some(0), Some(3)),
    ("a", "", None, None),
    ("a", "a", Some(0), Some(0)),
    ("a", "aa", Some(0), Some(1)),
    ("a", "ba", Some(1), Some(1)),
    ("a", "bba", Some(2), Some(2)),
    ("a", "bbba", Some(3), Some(3)),
    ("a", "bbbab", Some(3), Some(3)),
    ("a", "bbbabb", Some(3), Some(3)),
    ("a", "bbbabbb", Some(3), Some(3)),
    ("a", "bbbbbb", None, None),
    ("ab", "", None, None),
    ("ab", "a", None, None),
    ("ab", "b", None, None),
    ("ab", "ab", Some(0), Some(0)),
    ("ab", "aab", Some(1), Some(1)),
    ("ab", "aaab", Some(2), Some(2)),
    ("ab", "abaab", Some(0), Some(3)),
    ("ab", "baaab", Some(3), Some(3)),
    ("ab", "acb", None, None),
    ("ab", "abba", Some(0), Some(0)),
    ("abc", "ab", None, None),
    ("abc", "abc", Some(0), Some(0)),
    ("abc", "abcz", Some(0), Some(0)),
    ("abc", "abczz", Some(0), Some(0)),
    ("abc", "zabc", Some(1), Some(1)),
    ("abc", "zzabc", Some(2), Some(2)),
    ("abc", "azbc", None, None),
    ("abc", "abzc", None, None),
    ("abczdef", "abczdefzzzzzzzzzzzzzzzzzzzz", Some(0), Some(0)),
    ("abczdef", "zzzzzzzzzzzzzzzzzzzzabczdef", Some(20), Some(20)),
    // Failures caught by quickcheck.
    ("\u{0}\u{15}", "\u{0}\u{15}\u{15}\u{0}", Some(0), Some(0)),
    ("\u{0}\u{1e}", "\u{1e}\u{0}", None, None),
];

#[test]
fn unit_twoway_fwd() {
    run_search_tests_fwd("TwoWay", |n, h| TwoWay::forward(n).find(h));
}

#[test]
fn unit_twoway_rev() {
    run_search_tests_rev("TwoWay", |n, h| TwoWay::reverse(n).rfind(h));
}

/// Run the substring search tests. `name` should be the type of searcher used,
/// for diagnostics. `search` should be a closure that accepts a needle and a
/// haystack and returns the starting position of the first occurrence of
/// needle in the haystack, or `None` if one doesn't exist.
fn run_search_tests_fwd(
    name: &str,
    mut search: impl FnMut(&[u8], &[u8]) -> Option<usize>,
) {
    for &(needle, haystack, expected_fwd, _) in SEARCH_TESTS {
        let (n, h) = (needle.as_bytes(), haystack.as_bytes());
        assert_eq!(
            expected_fwd,
            search(n, h),
            "{}: needle: {:?}, haystack: {:?}, expected: {:?}",
            name,
            n,
            h,
            expected_fwd
        );
    }
}

/// Run the substring search tests. `name` should be the type of searcher used,
/// for diagnostics. `search` should be a closure that accepts a needle and a
/// haystack and returns the starting position of the last occurrence of
/// needle in the haystack, or `None` if one doesn't exist.
fn run_search_tests_rev(
    name: &str,
    mut search: impl FnMut(&[u8], &[u8]) -> Option<usize>,
) {
    for &(needle, haystack, _, expected_rev) in SEARCH_TESTS {
        let (n, h) = (needle.as_bytes(), haystack.as_bytes());
        assert_eq!(
            expected_rev,
            search(n, h),
            "{}: needle: {:?}, haystack: {:?}, expected: {:?}",
            name,
            n,
            h,
            expected_rev
        );
    }
}

quickcheck! {
    fn qc_twoway_fwd_prefix_is_substring(bs: Vec<u8>) -> bool {
        prop_prefix_is_substring(false, &bs, |n, h| TwoWay::forward(n).find(h))
    }

    fn qc_twoway_fwd_suffix_is_substring(bs: Vec<u8>) -> bool {
        prop_suffix_is_substring(false, &bs, |n, h| TwoWay::forward(n).find(h))
    }

    fn qc_twoway_rev_prefix_is_substring(bs: Vec<u8>) -> bool {
        prop_prefix_is_substring(true, &bs, |n, h| TwoWay::reverse(n).rfind(h))
    }

    fn qc_twoway_rev_suffix_is_substring(bs: Vec<u8>) -> bool {
        prop_suffix_is_substring(true, &bs, |n, h| TwoWay::reverse(n).rfind(h))
    }

    fn qc_twoway_fwd_matches_naive(
        needle: Vec<u8>,
        haystack: Vec<u8>
    ) -> bool {
        prop_matches_naive(
            false,
            &needle,
            &haystack,
            |n, h| TwoWay::forward(n).find(h),
        )
    }

    fn qc_twoway_rev_matches_naive(
        needle: Vec<u8>,
        haystack: Vec<u8>
    ) -> bool {
        prop_matches_naive(
            true,
            &needle,
            &haystack,
            |n, h| TwoWay::reverse(n).rfind(h),
        )
    }
}

/// Check that every prefix of the given byte string is a substring.
fn prop_prefix_is_substring(
    reverse: bool,
    bs: &[u8],
    mut search: impl FnMut(&[u8], &[u8]) -> Option<usize>,
) -> bool {
    if bs.is_empty() {
        return true;
    }
    for i in 0..(bs.len() - 1) {
        let prefix = &bs[..i];
        if reverse {
            assert_eq!(naive_rfind(prefix, bs), search(prefix, bs));
        } else {
            assert_eq!(naive_find(prefix, bs), search(prefix, bs));
        }
    }
    true
}

/// Check that every suffix of the given byte string is a substring.
fn prop_suffix_is_substring(
    reverse: bool,
    bs: &[u8],
    mut search: impl FnMut(&[u8], &[u8]) -> Option<usize>,
) -> bool {
    if bs.is_empty() {
        return true;
    }
    for i in 0..(bs.len() - 1) {
        let suffix = &bs[i..];
        if reverse {
            assert_eq!(naive_rfind(suffix, bs), search(suffix, bs));
        } else {
            assert_eq!(naive_find(suffix, bs), search(suffix, bs));
        }
    }
    true
}

/// Check that naive substring search matches the result of the given search
/// algorithm.
fn prop_matches_naive(
    reverse: bool,
    needle: &[u8],
    haystack: &[u8],
    mut search: impl FnMut(&[u8], &[u8]) -> Option<usize>,
) -> bool {
    if reverse {
        naive_rfind(needle, haystack) == search(needle, haystack)
    } else {
        naive_find(needle, haystack) == search(needle, haystack)
    }
}

/// Naively search forwards for the given needle in the given haystack.
fn naive_find(needle: &[u8], haystack: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    } else if haystack.len() < needle.len() {
        return None;
    }
    for i in 0..(haystack.len() - needle.len() + 1) {
        if needle == &haystack[i..i + needle.len()] {
            return Some(i);
        }
    }
    None
}

/// Naively search in reverse for the given needle in the given haystack.
fn naive_rfind(needle: &[u8], haystack: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(haystack.len());
    } else if haystack.len() < needle.len() {
        return None;
    }
    for i in (0..(haystack.len() - needle.len() + 1)).rev() {
        if needle == &haystack[i..i + needle.len()] {
            return Some(i);
        }
    }
    None
}
