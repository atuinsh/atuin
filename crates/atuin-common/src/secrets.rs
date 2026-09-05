//! Recognising credentials in text, so Atuin can avoid storing them.
//!
//! This file will probably trigger a lot of scanners. Sorry.
//!
//! Every pattern names, with a `secret` capture group, the span it wants taken out. For a bare
//! credential that group is the whole match; for one that appears as a value -- an assignment like
//! `AWS_SECRET_ACCESS_KEY=...`, a flag like `atuin login -p ...` -- it is only the value, so the
//! name or flag around it survives. Redacting the name and leaving the value beside it would be
//! worse than doing nothing, because it looks like it worked.
//!
//! The group is optional wherever the surrounding text is recognisable on its own. That is what
//! keeps the two entry points different:
//!
//! - [`contains_secret`] is true as soon as a pattern matches, whether or not a value was there to
//!   capture. `echo $AWS_SECRET_ACCESS_KEY` is recognised. Use it to discard a string entirely.
//! - [`redact`] only replaces captured groups, so that same string comes back untouched -- there
//!   is nothing in it to take out.
//!
//! Both are best-effort. [`redact`] in particular is applied to rendered terminal output, where a
//! credential can be broken up by SGR escape sequences or by a line wrap at the right margin.
//! Neither will match.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::LazyLock;

use regex::{Regex, RegexSet};

#[cfg(test)]
use self::tests::Test;

/// The string every credential [`redact`] locates is replaced with.
pub const REDACTED: &str = "****";

/// The capture group each pattern uses to name the span [`redact`] should replace.
const SECRET_GROUP: &str = "secret";

struct Pattern {
    name: &'static str,
    /// Must name a [`SECRET_GROUP`] capture group; see the module docs.
    regex: &'static str,
    /// See [`Test`].
    #[cfg(test)]
    tests: &'static [Test],
}

/// Every credential shape Atuin recognises.
static SECRET_PATTERNS: &[Pattern] = &[
    Pattern {
        name: "AWS Access Key ID",
        regex: "(?<secret>A[KS]IA[0-9A-Z]{16})",
        #[cfg(test)]
        tests: &[Test {
            input: "AKIAIOSFODNN7EXAMPLE",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "AWS Secret Access Key env var",
        regex: r"AWS_SECRET_ACCESS_KEY(?:\s*[=:]\s*(?<secret>\S+))?",
        #[cfg(test)]
        tests: &[
            Test {
                input: "AWS_SECRET_ACCESS_KEY=KEYDATA",
                redacted: "AWS_SECRET_ACCESS_KEY=****",
            },
            // Named but not assigned: recognised, but there is no value to take out.
            Test {
                input: "echo $AWS_SECRET_ACCESS_KEY",
                redacted: "echo $AWS_SECRET_ACCESS_KEY",
            },
        ],
    },
    Pattern {
        name: "AWS Session Token env var",
        regex: r"AWS_SESSION_TOKEN(?:\s*[=:]\s*(?<secret>\S+))?",
        #[cfg(test)]
        tests: &[Test {
            input: "AWS_SESSION_TOKEN=KEYDATA",
            redacted: "AWS_SESSION_TOKEN=****",
        }],
    },
    Pattern {
        name: "Microsoft Azure secret access key env var",
        // Lazy, so that two assignments on one line stay two matches rather than one match
        // spanning both -- which would leave the first value exposed.
        regex: r"AZURE_.*?_KEY(?:\s*[=:]\s*(?<secret>\S+))?",
        #[cfg(test)]
        tests: &[Test {
            input: "export AZURE_STORAGE_ACCOUNT_KEY=KEYDATA",
            redacted: "export AZURE_STORAGE_ACCOUNT_KEY=****",
        }],
    },
    Pattern {
        name: "Google cloud platform key env var",
        regex: r"GOOGLE_SERVICE_ACCOUNT_KEY(?:\s*[=:]\s*(?<secret>\S+))?",
        #[cfg(test)]
        tests: &[Test {
            input: "export GOOGLE_SERVICE_ACCOUNT_KEY=KEYDATA",
            redacted: "export GOOGLE_SERVICE_ACCOUNT_KEY=****",
        }],
    },
    // The password and the key are separate patterns because one match can only capture one group,
    // and `atuin login` takes both at once.
    Pattern {
        name: "Atuin login password",
        regex: r#"atuin\s+login(?:[^\n]*?\s-(?:p|-password)[=\s]+(?<secret>"[^"]*"|'[^']*'|\S+))?"#,
        #[cfg(test)]
        tests: &[
            Test {
                input: "atuin login -u mycoolusername -p mycoolpassword",
                redacted: "atuin login -u mycoolusername -p ****",
            },
            // Recognised, but nothing to take out.
            Test {
                input: "atuin login",
                redacted: "atuin login",
            },
        ],
    },
    Pattern {
        name: "Atuin login key",
        regex: r#"atuin\s+login(?:[^\n]*?\s-(?:k|-key)[=\s]+(?<secret>"[^"]*"|'[^']*'|\S+))?"#,
        #[cfg(test)]
        tests: &[Test {
            input: "atuin login -k \"lots of random words\"",
            redacted: "atuin login -k ****",
        }],
    },
    Pattern {
        name: "GitHub PAT (old)",
        regex: "(?<secret>ghp_[a-zA-Z0-9]{36})",
        // legit, I expired it
        #[cfg(test)]
        tests: &[Test {
            input: "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",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "GitHub PAT (new)",
        regex: "(?<secret>gh1_[A-Za-z0-9]{21}_[A-Za-z0-9]{59}\
                |github_pat_[0-9][A-Za-z0-9]{21}_[A-Za-z0-9]{59})",
        #[cfg(test)]
        tests: &[
            Test {
                input: "gh1_1234567890abcdefghijk_1234567890abcdefghijklmnopqrstuvwxyz1234567890abcdefghijklm",
                redacted: REDACTED,
            },
            // also legit, also expired
            Test {
                input: "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",
                redacted: REDACTED,
            },
        ],
    },
    Pattern {
        name: "GitHub OAuth Access Token",
        regex: "(?<secret>gho_[A-Za-z0-9]{36})",
        // not a real token
        #[cfg(test)]
        tests: &[Test {
            input: "gho_1234567890abcdefghijklmnopqrstuvwx00",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "GitHub OAuth Access Token (user)",
        regex: "(?<secret>ghu_[A-Za-z0-9]{36})",
        // not a real token
        #[cfg(test)]
        tests: &[Test {
            input: "ghu_1234567890abcdefghijklmnopqrstuvwx00",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "GitHub App Installation Access Token",
        regex: "(?<secret>ghs_[A-Za-z0-9._-]{36,})",
        #[cfg(test)]
        tests: &[
            // not a real token
            Test {
                input: "ghs_1234567890abcdefghijklmnopqrstuvwx000",
                redacted: REDACTED,
            },
            // new token format, fake data
            Test {
                input: "ghs_abc-def.ghi_jklMNOP0123456789qrstuv-wxyzABCD",
                redacted: REDACTED,
            },
        ],
    },
    Pattern {
        name: "GitHub Refresh Token",
        regex: "(?<secret>ghr_[A-Za-z0-9]{76})",
        // not a real token
        #[cfg(test)]
        tests: &[Test {
            input: "ghr_1234567890abcdefghijklmnopqrstuvwx1234567890abcdefghijklmnopqrstuvwx12345678",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "GitHub App Installation Access Token v1",
        regex: r"(?<secret>v1\.[0-9A-Fa-f]{40})",
        // not a real token
        #[cfg(test)]
        tests: &[Test {
            input: "v1.1234567890abcdef1234567890abcdef12345678",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "GitLab PAT",
        regex: "(?<secret>glpat-[a-zA-Z0-9_]{20})",
        #[cfg(test)]
        tests: &[Test {
            input: "glpat-RkE_BG5p_bbjML21WSfy",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "Slack OAuth v2 bot",
        regex: "(?<secret>xoxb-[0-9]{11}-[0-9]{11}-[0-9a-zA-Z]{24})",
        #[cfg(test)]
        tests: &[Test {
            input: "xoxb-17653672481-19874698323-pdFZKVeTuE8sk7oOcBrzbqgy",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "Slack OAuth v2 user token",
        regex: "(?<secret>xoxp-[0-9]{11}-[0-9]{11}-[0-9a-zA-Z]{24})",
        #[cfg(test)]
        tests: &[Test {
            input: "xoxp-17653672481-19874698323-pdFZKVeTuE8sk7oOcBrzbqgy",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "Slack webhook",
        regex: "(?<secret>T[a-zA-Z0-9_]{8}/B[a-zA-Z0-9_]{8}/[a-zA-Z0-9_]{24})",
        #[cfg(test)]
        tests: &[Test {
            input: "https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX",
            redacted: "https://hooks.slack.com/services/****",
        }],
    },
    Pattern {
        name: "Stripe test key",
        regex: "(?<secret>sk_test_[0-9a-zA-Z]{24})",
        // Split so the literal is not a contiguous `sk_test_...` token in this file: at the
        // correct length for the pattern, GitHub push protection rejects it as a real key.
        #[cfg(test)]
        tests: &[Test {
            input: concat!("sk_", "test_1234567890abcdefghijklmn"),
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "Stripe live key",
        regex: "(?<secret>sk_live_[0-9a-zA-Z]{24})",
        // See the note on the test key above.
        #[cfg(test)]
        tests: &[Test {
            input: concat!("sk_", "live_1234567890abcdefghijklmn"),
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "Netlify authentication token",
        regex: "(?<secret>nf[pcoub]_[0-9a-zA-Z]{36})",
        #[cfg(test)]
        tests: &[Test {
            input: "nfp_nBh7BdJxUwyaBBwFzpyD29MMFT6pZ9wq5634",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "npm token",
        regex: "(?<secret>npm_[A-Za-z0-9]{36})",
        #[cfg(test)]
        tests: &[Test {
            input: "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",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "Pulumi personal access token",
        regex: "(?<secret>pul-[0-9a-f]{40})",
        #[cfg(test)]
        tests: &[Test {
            input: "pul-683c2770662c51d960d72ec27613be7653c5cb26",
            redacted: REDACTED,
        }],
    },
];

struct Patterns {
    /// A prefilter over every pattern at once: one pass says which, if any, are worth running.
    set: RegexSet,
    regexes: Vec<Regex>,
}

static PATTERNS: LazyLock<Patterns> = LazyLock::new(|| {
    let regexes: Vec<Regex> = SECRET_PATTERNS
        .iter()
        .map(|pattern| {
            Regex::new(pattern.regex)
                .unwrap_or_else(|e| panic!("failed to compile regex for {}: {e}", pattern.name))
        })
        .collect();

    Patterns {
        set: RegexSet::new(regexes.iter().map(Regex::as_str))
            .expect("failed to build secrets regex set"),
        regexes,
    }
});

/// Whether `s` contains anything that looks like it involves a credential.
#[must_use]
pub fn contains_secret(s: &str) -> bool {
    PATTERNS.set.is_match(s)
}

/// Replace every credential [`redact`] can locate in `s` with [`REDACTED`].
///
/// Returns [`Cow::Borrowed`] if and only if nothing was replaced, so
/// `matches!(redact(s), Cow::Owned(_))` is an exact test for "something was taken out". Note that
/// this is a weaker condition than [`contains_secret`]: a pattern can match text that holds no
/// value to remove.
///
/// Best-effort; in particular it cannot see through SGR escape sequences or terminal line wraps.
#[must_use]
pub fn redact(s: &str) -> Cow<'_, str> {
    let mut spans: Vec<Range<usize>> = PATTERNS
        .set
        .matches(s)
        .iter()
        .flat_map(|i| PATTERNS.regexes[i].captures_iter(s))
        .filter_map(|caps| Some(caps.name(SECRET_GROUP)?.range()))
        // Text that is already the marker is not a change. This is what makes `redact`
        // idempotent and keeps "Borrowed iff nothing changed" exact.
        .filter(|span| &s[span.clone()] != REDACTED)
        .collect();

    if spans.is_empty() {
        return Cow::Borrowed(s);
    }
    spans.sort_unstable_by_key(|span| span.start);

    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    for span in spans {
        if span.start >= cursor {
            out.push_str(&s[cursor..span.start]);
            out.push_str(REDACTED);
            cursor = span.end;
        } else if span.end > cursor {
            // Overlaps the redaction just written and runs past it. Swallow the tail into that
            // same redaction rather than emitting it raw or marking it twice.
            cursor = span.end;
        }
    }
    out.push_str(&s[cursor..]);

    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use proptest::prelude::*;
    use rstest::rstest;

    use super::{PATTERNS, REDACTED, SECRET_GROUP, SECRET_PATTERNS, contains_secret, redact};

    pub(super) struct Test {
        pub(super) input: &'static str,
        pub(super) redacted: &'static str,
    }

    /// An alphabet dense in the characters the patterns care about, so that generated strings
    /// actually trip them instead of drifting past.
    const FUZZY: &str = r#"[A-Za-z0-9_=:$"' .\-]{0,120}"#;

    /// The table cases whose *whole* input is the credential, so planting one inside a larger
    /// string must yield exactly one [`REDACTED`]. Derived from the table rather than repeated, so
    /// a new pattern joins the property tests for free.
    fn plantable() -> Vec<&'static str> {
        SECRET_PATTERNS
            .iter()
            .flat_map(|pattern| pattern.tests)
            .filter(|test| test.redacted == REDACTED)
            .map(|test| test.input)
            .collect()
    }

    /// The contract `redact` relies on: without this group it would not know which part of a match
    /// is the credential and which is the variable name holding it.
    #[test]
    fn every_pattern_names_its_secret_group() {
        for (pattern, regex) in SECRET_PATTERNS.iter().zip(&PATTERNS.regexes) {
            assert!(
                regex.capture_names().any(|name| name == Some(SECRET_GROUP)),
                "{} does not name a `{SECRET_GROUP}` capture group",
                pattern.name
            );
        }
    }

    #[test]
    fn every_pattern_is_recognised() {
        for pattern in SECRET_PATTERNS {
            for test in pattern.tests {
                assert!(
                    contains_secret(test.input),
                    "{} not recognised: {}",
                    pattern.name,
                    test.input
                );
            }
        }
    }

    /// Each pattern's test value must redact to exactly what the pattern promises. Run both bare
    /// and embedded in surrounding text, since output is rarely just the credential.
    #[rstest]
    fn every_pattern_redacts_to_its_declared_value(#[values(false, true)] embed: bool) {
        for pattern in SECRET_PATTERNS {
            for test in pattern.tests {
                let wrap = |s: &str| {
                    if embed {
                        format!("some random text {s} some more random text")
                    } else {
                        s.to_string()
                    }
                };
                let (input, expected) = (wrap(test.input), wrap(test.redacted));

                assert_eq!(
                    &*redact(&input),
                    expected.as_str(),
                    "{} redacted wrongly",
                    pattern.name
                );
            }
        }
    }

    /// `contains_secret` decides whether a command is stored at all, so these cases are the guard
    /// on that behaviour: a pattern that stopped matching a mere mention would start letting the
    /// command through, and its output would start being captured.
    #[rstest]
    #[case::assignment("AWS_SECRET_ACCESS_KEY=KEYDATA", true)]
    #[case::mere_mention_with_no_value("echo $AWS_SECRET_ACCESS_KEY", true)]
    #[case::login_without_arguments("atuin login", true)]
    #[case::bare_credential("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", true)]
    #[case::ordinary_command("ls -la /tmp", false)]
    #[case::empty("", false)]
    #[case::the_marker_itself("****", false)]
    fn recognises_text_involving_a_credential(#[case] input: &str, #[case] expected: bool) {
        assert_eq!(contains_secret(input), expected);
    }

    #[rstest]
    // Blanking the name and leaving the value beside it would be worse than doing nothing,
    // because it looks like it worked.
    #[case::assignment_keeps_its_name(
        "export AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI",
        "export AWS_SECRET_ACCESS_KEY=****"
    )]
    #[case::colon_separated_assignment("AWS_SESSION_TOKEN: abc123", "AWS_SESSION_TOKEN: ****")]
    // One match can only fill one group, which is why the login password and key are separate
    // patterns; both must fire on the same command.
    #[case::both_credentials_of_one_login(
        "atuin login -u mycoolusername -p mycoolpassword -k \"lots of random words\"",
        "atuin login -u mycoolusername -p **** -k ****"
    )]
    #[case::single_quoted_login_password("atuin login -p 'hunter two'", "atuin login -p ****")]
    #[case::long_login_flag("atuin login --password=hunter2", "atuin login --password=****")]
    // A greedy `AZURE_.*_KEY` would span from the first name to the last, putting the first value
    // outside the captured group and so leaving it exposed.
    #[case::two_assignments_stay_two_matches(
        "export AZURE_A_KEY=one AZURE_B_KEY=two",
        "export AZURE_A_KEY=**** AZURE_B_KEY=****"
    )]
    #[case::surrounding_text_survives(
        "export TOKEN=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 # for ci",
        "export TOKEN=**** # for ci"
    )]
    // Splicing is by byte offset, so a multi-byte neighbour would panic on a mid-character span.
    #[case::multibyte_neighbours_survive(
        "café ☕ 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 ☕ café",
        "café ☕ **** ☕ café"
    )]
    #[case::credential_at_the_very_start(
        "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 is the token",
        "**** is the token"
    )]
    #[case::two_credentials_on_separate_lines(
        "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\nglpat-RkE_BG5p_bbjML21WSfy",
        "****\n****"
    )]
    // Five stars is not the marker; it is a value, and comes out as the marker.
    #[case::five_stars_are_a_value("AWS_SECRET_ACCESS_KEY=*****", "AWS_SECRET_ACCESS_KEY=****")]
    fn redacts(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(&*redact(input), expected);
    }

    /// Borrowed means "nothing was taken out". Note this is weaker than [`contains_secret`] being
    /// false: a pattern can match text that holds no value to remove.
    #[rstest]
    #[case::ordinary_command("ls -la /tmp")]
    #[case::empty("")]
    #[case::the_marker_itself("****")]
    #[case::named_but_never_assigned("echo $AWS_SECRET_ACCESS_KEY")]
    #[case::login_without_arguments("atuin login")]
    #[case::almost_a_token("ghp_tooshort")]
    #[case::already_redacted_assignment("AWS_SECRET_ACCESS_KEY=****")]
    #[case::already_redacted_login("atuin login -p ****")]
    #[case::two_markers_as_value("AWS_SESSION_TOKEN=**** ****")]
    fn passes_text_through_borrowed(#[case] input: &str) {
        assert!(matches!(redact(input), Cow::Borrowed(_)), "{input:?} should not be copied");
    }

    #[rstest]
    // The `ghs_` character class swallows the `ghp_` token that follows it, so the two patterns
    // match overlapping spans and must collapse into a single marker.
    #[case::overlapping(&format!("ghs_{}ghp_{}", "a".repeat(32), "b".repeat(36)), "****")]
    #[case::adjacent(&format!("ghp_{} npm_{}", "a".repeat(36), "b".repeat(36)), "**** ****")]
    // Distinct credentials with nothing between them are adjacent, not overlapping: two
    // credentials were found, so two markers is the honest result.
    #[case::touching(&format!("ghp_{}npm_{}", "a".repeat(36), "b".repeat(36)), "********")]
    fn merges_spans(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(&*redact(input), expected);
    }

    proptest! {
        /// Plant credentials between stretches of ordinary text. Every credential must become
        /// exactly one marker, and every stretch around them must survive verbatim -- which pins
        /// the splicing, the sort, and the span boundaries all at once.
        ///
        /// Lowercase filler cannot form a credential of its own (every pattern needs an uppercase
        /// letter, a digit or punctuation), and the spaces around each planted value stop a
        /// greedy class from reaching into its neighbours.
        #[test]
        fn planted_credentials_go_and_their_surroundings_stay(
            parts in prop::collection::vec(("[a-z]{0,16}", prop::sample::select(plantable())), 1..6),
            tail in "[a-z]{0,16}",
        ) {
            let mut input = String::new();
            let mut expected = String::new();
            for (filler, credential) in &parts {
                prop_assume!(!contains_secret(filler));
                input.push_str(filler);
                input.push(' ');
                input.push_str(credential);
                input.push(' ');

                expected.push_str(filler);
                expected.push(' ');
                expected.push_str(REDACTED);
                expected.push(' ');
            }
            prop_assume!(!contains_secret(&tail));
            input.push_str(&tail);
            expected.push_str(&tail);

            prop_assert_eq!(&*redact(&input), expected.as_str());
        }

        /// The contract callers rely on to avoid copying clean output.
        #[test]
        fn borrowed_exactly_when_nothing_changed(s in FUZZY) {
            match redact(&s) {
                Cow::Borrowed(out) => prop_assert_eq!(out, s.as_str()),
                Cow::Owned(out) => prop_assert_ne!(out.as_str(), s.as_str()),
            }
        }

        /// Redacting again must be a no-op, or the marker itself would be feeding the patterns.
        #[test]
        fn redaction_is_idempotent(s in FUZZY) {
            let once = redact(&s).into_owned();
            prop_assert_eq!(&*redact(&once), once.as_str());
        }

        #[test]
        fn text_with_nothing_recognisable_is_returned_as_is(s in FUZZY) {
            prop_assume!(!contains_secret(&s));
            prop_assert!(matches!(redact(&s), Cow::Borrowed(_)));
        }

        /// Arbitrary unicode either side of a planted credential: the credential still goes, and
        /// the byte-offset splicing must not land mid-character.
        #[test]
        fn a_planted_credential_goes_whatever_surrounds_it(
            prefix in "\\PC{0,20}",
            suffix in "\\PC{0,20}",
            credential in prop::sample::select(plantable()),
        ) {
            let redacted = redact(&format!("{prefix} {credential} {suffix}")).into_owned();
            prop_assert!(redacted.contains(REDACTED));
            prop_assert!(!redacted.contains(credential));
        }
    }
}
