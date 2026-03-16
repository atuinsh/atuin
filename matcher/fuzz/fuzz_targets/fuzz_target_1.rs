#![no_main]

use fzf_oxide::{chars, Matcher, MatcherConfig, Utf32Str};
use libfuzzer_sys::arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
pub struct Input<'a> {
    haystack: &'a str,
    needle: &'a str,
    ignore_case: bool,
    normalize: bool,
}

fuzz_target!(|data: Input<'_>| {
    let mut data = data;
    let mut config = MatcherConfig::DEFAULT;
    config.ignore_case = data.ignore_case;
    config.normalize = data.normalize;
    let mut matcher = Matcher::new(config);
    let mut indices_optimal = Vec::new();
    let mut indices_greedy = Vec::new();
    let mut needle_buf = Vec::new();
    let mut haystack_buf = Vec::new();
    let normalize = |mut c: char| {
        if config.normalize {
            c = chars::normalize(c);
        }
        if config.ignore_case {
            c = chars::to_lower_case(c);
        }
        c
    };
    let needle: String = data.needle.chars().map(normalize).collect();
    let needle_chars: Vec<_> = needle.chars().collect();
    let needle = Utf32Str::new(&needle, &mut needle_buf);
    let haystack = Utf32Str::new(data.haystack, &mut haystack_buf);

    let greedy_score = matcher.fuzzy_indices_greedy(haystack, needle, &mut indices_greedy);
    if greedy_score.is_some() {
        let match_chars: Vec<_> = indices_greedy
            .iter()
            .map(|&i| normalize(haystack.get(i)))
            .collect();
        assert_eq!(
            match_chars, needle_chars,
            "failed match, found {indices_greedy:?} {match_chars:?} (greedy)"
        );
    }
    let optimal_score = matcher.fuzzy_indices(haystack, needle, &mut indices_optimal);
    if optimal_score.is_some() {
        let match_chars: Vec<_> = indices_optimal
            .iter()
            .map(|&i| normalize(haystack.get(i)))
            .collect();
        assert_eq!(
            match_chars, needle_chars,
            "failed match, found {indices_optimal:?} {match_chars:?}"
        );
    }
    match (greedy_score, optimal_score) {
        (None, Some(score)) => unreachable!("optimal matched {score} but greedy did not match"),
        (Some(score), None) => unreachable!("greedy matched {score} but optimal did not match"),
        (Some(greedy), Some(optimal)) => {
            assert!(
                greedy <= optimal,
                "optimal score must be atleast the same as greedy score {greedy} {optimal}"
            );
            if indices_greedy == indices_optimal {
                assert_eq!(
                    greedy, optimal,
                    "if matching same char greedy and optimal score should be identical"
                )
            }
        }
        (None, None) => (),
    }
});
