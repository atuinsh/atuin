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
//! credential can be broken up by SGR escape sequences, which will not match. (A soft wrap at the
//! right margin is re-joined by the terminal emulator before capture, so a wrap alone does not
//! defeat matching.)

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
    prefilter: Option<&'static str>,
    /// See [`Test`].
    #[cfg(test)]
    tests: &'static [Test],
}

/// The value side of an assignment or flag: balanced quoted segments, unquoted runs and lone
/// quotes, repeated, so a shell concatenation like `'it'"'"'s'` or an unterminated `"oops` is one
/// value, while an unquoted run stops at structural punctuation so a compact JSON line is not
/// swallowed past the value. Balanced quotes are tried first, which is what makes `"v",` stop.
macro_rules! secret_value {
    () => {
        concat!(
            r#"(?<secret>(?:\x1b\[[0-9;]*m)*(?:"#,
            r#"\{(?:[^{}\n]|\{(?:[^{}\n]|\{[^{}\n]*\})*\})*\}"#,
            r#"|\[(?:[^\[\]\n]|\[(?:[^\[\]\n]|\[[^\[\]\n]*\])*\])*\]"#,
            r#"|[{\[][^\n]*"#,
            r#"|\|[^|\n]+\|"#,
            r#"|(?:"(?:[^"\\\n]|\\.)*"|'[^'\n]*'|["'])(?:"(?:[^"\\\n]|\\.)*"|'[^'\n]*'|["']|[^\s"',;)\]}]+)*"#,
            r#"|[^\s"',;)\]}=:][^\s"',;)\]}]*"#,
            "))"
        )
    };
}

/// A variable name followed, *optionally*, by a separator and its value. Optional so that a bare
/// mention still matches — and so still drops the command — while capturing nothing.
///
/// Between the name and the separator a closing quote or `]` may sit (JSON, TOML, Python
/// `os.environ[..]`). The separator is `=`, `:`, `:=`, `=>`, `==`, a type annotation ending in `=`,
/// a table column gap (a tab or two-plus spaces) or a `|`/`│` cell border. Nothing here crosses a
/// newline, so an empty assignment cannot reach into the next line.
///
/// A type annotation needs a blank before its `=`, so that a base64 value's `==` padding reads as
/// part of the value rather than as `type =`. The table-gap and `|` separators deliberately take the
/// next word even in prose or a pipeline (`NAME  is required`, `"$NAME" | pbcopy`): over-redaction
/// there is the price of catching `vault`/`doppler`-style tables.
macro_rules! type_token {
    () => {
        r#"&?(?:'\w+[ \t]+)?[\w.:]+(?:[<\[](?:[^<>\[\]\n=]|[<\[][^<>\[\]\n=]*[>\]])*[>\]])?"#
    };
}

macro_rules! type_annotation {
    () => {
        concat!(type_token!(), r"(?:[ \t]*\|[ \t]*", type_token!(), ")*")
    };
}

macro_rules! assigned {
    ($name:literal) => {
        concat!(
            $name,
            r#"(?:\w*["']?(?:\[\d+\])?\]?[ \t]*(?::[ \t]*"#,
            type_annotation!(),
            r#"[ \t]+=|[?+]=|[=:][=>]?|[ \t]*[|│][ \t]*|[ \t]{2,}|\t)[ \t]*"#,
            secret_value!(),
            ")?"
        )
    };
}

/// `atuin login`, followed optionally by one of `$flags` and its value later on the line (a flag
/// that opens the very next line still counts — capturing it is the safe direction). The search
/// window is bounded so that a line mentioning `atuin login` many times costs linear rather than
/// quadratic time; no real login line has 512 characters before its password.
macro_rules! login_flag {
    ($flags:literal) => {
        concat!(r"atuin\s+login(?:[^\n]{0,512}?\s+-(?:", $flags, r")[= \t]*", secret_value!(), ")?")
    };
}

/// Every credential shape Atuin recognises.
static SECRET_PATTERNS: &[Pattern] = &[
    Pattern {
        name: "AWS Access Key ID",
        regex: "(?<secret>A[KS]IA[0-9A-Z]{16})",
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: "AKIAIOSFODNN7EXAMPLE",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "AWS Secret Access Key env var",
        regex: assigned!("AWS_SECRET_ACCESS_KEY"),
        prefilter: Some("AWS_SECRET_ACCESS_KEY"),
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
        regex: assigned!("AWS_SESSION_TOKEN"),
        prefilter: Some("AWS_SESSION_TOKEN"),
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
        regex: assigned!(r"AZURE_.*?_KEY"),
        prefilter: Some(r"AZURE_.*?_KEY"),
        #[cfg(test)]
        tests: &[Test {
            input: "export AZURE_STORAGE_ACCOUNT_KEY=KEYDATA",
            redacted: "export AZURE_STORAGE_ACCOUNT_KEY=****",
        }],
    },
    Pattern {
        name: "Google cloud platform key env var",
        regex: assigned!("GOOGLE_SERVICE_ACCOUNT_KEY"),
        prefilter: Some("GOOGLE_SERVICE_ACCOUNT_KEY"),
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
        regex: login_flag!("p|-password"),
        prefilter: Some(r"atuin\s+login"),
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
        regex: login_flag!("k|-key"),
        prefilter: Some(r"atuin\s+login"),
        #[cfg(test)]
        tests: &[Test {
            input: "atuin login -k \"lots of random words\"",
            redacted: "atuin login -k ****",
        }],
    },
    Pattern {
        name: "GitHub PAT (old)",
        regex: "(?<secret>ghp_[a-zA-Z0-9]{36})",
        prefilter: None,
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
        prefilter: None,
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
        prefilter: None,
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
        prefilter: None,
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
        prefilter: None,
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
        prefilter: None,
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
        prefilter: None,
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
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: "glpat-RkE_BG5p_bbjML21WSfy",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "Slack OAuth v2 bot",
        regex: "(?<secret>xoxb-[0-9]{11}-[0-9]{11}-[0-9a-zA-Z]{24})",
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: "xoxb-17653672481-19874698323-pdFZKVeTuE8sk7oOcBrzbqgy",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "Slack OAuth v2 user token",
        regex: "(?<secret>xoxp-[0-9]{11}-[0-9]{11}-[0-9a-zA-Z]{24})",
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: "xoxp-17653672481-19874698323-pdFZKVeTuE8sk7oOcBrzbqgy",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "Slack webhook",
        regex: "(?<secret>T[a-zA-Z0-9_]{8}/B[a-zA-Z0-9_]{8}/[a-zA-Z0-9_]{24})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: "https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX",
                redacted: "https://hooks.slack.com/services/****",
            },
            Test { input: "T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX", redacted: REDACTED },
        ],
    },
    Pattern {
        name: "Stripe test key",
        regex: "(?<secret>sk_test_[0-9a-zA-Z]{24,})",
        prefilter: None,
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
        regex: "(?<secret>sk_live_[0-9a-zA-Z]{24,})",
        prefilter: None,
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
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: "nfp_nBh7BdJxUwyaBBwFzpyD29MMFT6pZ9wq5634",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "npm token",
        regex: "(?<secret>npm_[A-Za-z0-9]{36})",
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: "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",
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "Pulumi personal access token",
        regex: "(?<secret>pul-[0-9a-f]{40})",
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: "pul-683c2770662c51d960d72ec27613be7653c5cb26",
            redacted: REDACTED,
        }],
    },
];

static PREFILTER: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new(SECRET_PATTERNS.iter().map(|pattern| pattern.prefilter.unwrap_or(pattern.regex)))
        .expect("failed to build secrets regex set")
});

static REGEXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    SECRET_PATTERNS
        .iter()
        .map(|pattern| {
            Regex::new(pattern.regex)
                .unwrap_or_else(|e| panic!("failed to compile regex for {}: {e}", pattern.name))
        })
        .collect()
});

static SGR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").expect("sgr regex"));

static OUTPUT_UNSAFE: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new([
        r"atuin\s+key\b",
        r"atuin\s+(?:account\s+)?login\b",
        r"atuin\s+(?:account\s+)?register\b",
        r"atuin\s+account\s+change-password\b",
    ])
    .expect("failed to build output-unsafe set")
});

#[must_use]
pub fn output_unsafe(command: &str) -> bool {
    OUTPUT_UNSAFE.is_match(&SGR.replace_all(command, ""))
}

/// Whether `s` contains anything that looks like it involves a credential.
#[must_use]
pub fn contains_secret(s: &str) -> bool {
    PREFILTER.is_match(s)
}

fn is_multiline_opener(value: &str) -> bool {
    matches!(value, "|" | "|-" | "|+" | ">" | ">-" | ">+" | "\\" | "{" | "[")
        || value.starts_with("<<")
        || value.starts_with("!!")
        || (value.len() > 1
            && value.starts_with('&')
            && value[1..].chars().all(|c| c.is_alphanumeric() || c == '_'))
}

/// A single pass over `s`, replacing every credential the patterns can locate with [`REDACTED`].
fn redact_once(s: &str) -> Cow<'_, str> {
    let mut spans: Vec<Range<usize>> = PREFILTER
        .matches(s)
        .iter()
        .flat_map(|i| REGEXES[i].captures_iter(s))
        .filter_map(|caps| Some(caps.name(SECRET_GROUP)?.range()))
        // Text that is already the marker is not a change. This is what makes `redact`
        // idempotent and keeps "Borrowed iff nothing changed" exact.
        .filter(|span| &s[span.clone()] != REDACTED)
        .filter(|span| !is_multiline_opener(&s[span.clone()]))
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

/// Replace every credential [`redact`] can locate in `s` with [`REDACTED`].
///
/// Returns [`Cow::Borrowed`] if and only if nothing was replaced, so
/// `matches!(redact(s), Cow::Owned(_))` is an exact test for "something was taken out". Note that
/// this is a stronger condition than [`contains_secret`]: everything replaced was recognised, but
/// a pattern can also match text that holds no value to remove.
///
/// Best-effort; in particular it cannot see through SGR escape sequences.
#[must_use]
pub fn redact(s: &str) -> Cow<'_, str> {
    let mut out = match redact_once(s) {
        Cow::Borrowed(_) => return Cow::Borrowed(s),
        Cow::Owned(out) => out,
    };

    // Two patterns can carve one stretch into spans whose leftovers a second pass captures
    // differently (see `redaction_reaches_a_fixed_point_in_one_call`), so iterate until a pass
    // changes nothing. Each pass replaces at least one span that is not already the marker, so
    // this converges in a couple of iterations; the bound is a backstop, not a budget — a test
    // panics on it, a running sink only warns.
    for _ in 0..8 {
        let next = match redact_once(&out) {
            Cow::Borrowed(_) => None,
            Cow::Owned(next) => Some(next),
        };
        let Some(next) = next else {
            return Cow::Owned(out);
        };
        out = next;
    }
    #[allow(clippy::manual_assert)]
    if cfg!(test) {
        panic!("redact did not reach a fixed point in 8 passes on {s:?}");
    }
    tracing::warn!("redact did not reach a fixed point in 8 passes; storing the last pass");
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::LazyLock;

    use proptest::prelude::*;
    use regex::RegexSet;
    use rstest::rstest;

    use super::{
        REDACTED, REGEXES, SECRET_GROUP, SECRET_PATTERNS, contains_secret, output_unsafe, redact,
    };

    pub(super) struct Test {
        pub(super) input: &'static str,
        pub(super) redacted: &'static str,
    }

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

    /// A generator that never produces a credential makes every property test vacuous: each one
    /// collapses to "clean input comes back Borrowed". This pins that the generator below does
    /// reach the Owned path, so the properties driven by it mean what they say.
    #[test]
    fn the_credential_dense_generator_reaches_the_owned_path() {
        use proptest::strategy::ValueTree;
        use proptest::test_runner::TestRunner;

        let mut runner = TestRunner::deterministic();
        let strategy = credential_dense();
        let owned = (0..500)
            .filter(|_| {
                let sample = strategy.new_tree(&mut runner).expect("strategy").current();
                matches!(redact(&sample), Cow::Owned(_))
            })
            .count();

        assert!(
            owned >= 100,
            "only {owned}/500 generated strings changed under redact; the generator is not \
             exercising the Owned path"
        );
    }

    /// Names, flags and separator glue the patterns care about, so that generated text actually
    /// forms assignments and login lines rather than drifting past every pattern.
    const NAMES_AND_FLAGS: &[&str] = &[
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AZURE_STORAGE_ACCOUNT_KEY",
        "GOOGLE_SERVICE_ACCOUNT_KEY",
        "atuin login",
        "-p",
        "-k",
        "--password=",
        "--key",
    ];
    const GLUE: &[&str] = &[
        "=", ": ", " := ", " => ", " == ", "\"", "'", "]", "[", " ", "  ", "\t", "\n", "****", "*",
        ",", "}", "{", "|",
    ];

    /// Text in which credentials, assignment shapes, quotes, the marker and whitespace occur
    /// often, so property tests reach the Owned path. Credentials come from the pattern table's
    /// own plantable fixtures, so a new pattern is generated automatically.
    fn credential_dense() -> impl Strategy<Value = String> {
        let token = prop_oneof![
            4 => prop::sample::select(plantable()).prop_map(str::to_owned),
            3 => prop::sample::select(NAMES_AND_FLAGS.to_vec()).prop_map(str::to_owned),
            3 => prop::sample::select(GLUE.to_vec()).prop_map(str::to_owned),
            2 => "[a-z0-9]{1,8}",
        ];
        prop::collection::vec(token, 0..12).prop_map(|parts| parts.concat())
    }

    /// The expressions `should_save` matched before this module existed, verbatim from
    /// `atuin-client/src/secrets.rs` at 7baae5c23. FROZEN. A pattern edit that makes
    /// `contains_secret` disagree with this set changes which commands atuin drops from history,
    /// and needs its own review — it is not a redaction tweak.
    const OLD_PATTERNS: &[&str] = &[
        "A[KS]IA[0-9A-Z]{16}",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AZURE_.*_KEY",
        "GOOGLE_SERVICE_ACCOUNT_KEY",
        r"atuin\s+login",
        "ghp_[a-zA-Z0-9]{36}",
        "gh1_[A-Za-z0-9]{21}_[A-Za-z0-9]{59}|github_pat_[0-9][A-Za-z0-9]{21}_[A-Za-z0-9]{59}",
        "gho_[A-Za-z0-9]{36}",
        "ghu_[A-Za-z0-9]{36}",
        "ghs_[A-Za-z0-9._-]{36,}",
        "ghr_[A-Za-z0-9]{76}",
        r"v1\.[0-9A-Fa-f]{40}",
        "glpat-[a-zA-Z0-9_]{20}",
        "xoxb-[0-9]{11}-[0-9]{11}-[0-9a-zA-Z]{24}",
        "xoxp-[0-9]{11}-[0-9]{11}-[0-9a-zA-Z]{24}",
        "T[a-zA-Z0-9_]{8}/B[a-zA-Z0-9_]{8}/[a-zA-Z0-9_]{24}",
        "sk_test_[0-9a-zA-Z]{24}",
        "sk_live_[0-9a-zA-Z]{24}",
        "nf[pcoub]_[0-9a-zA-Z]{36}",
        "npm_[A-Za-z0-9]{36}",
        "pul-[0-9a-f]{40}",
    ];
    static OLD: LazyLock<RegexSet> =
        LazyLock::new(|| RegexSet::new(OLD_PATTERNS).expect("frozen old patterns compile"));

    /// Boundary strings where a rewrite is most likely to drift from the old set: bare mentions,
    /// odd whitespace inside the Azure wildcard, newlines inside `atuin\s+login`, near-misses.
    #[rstest]
    #[case::bare_name("AWS_SECRET_ACCESS_KEY")]
    #[case::mention("echo $AWS_SESSION_TOKEN")]
    #[case::azure_with_space("AZURE_ X_KEY")]
    #[case::azure_with_tab("AZURE_\tX_KEY")]
    #[case::azure_tokens_apart("AZURE_TENANT_ID=abc AZURE_CLIENT_SECRET=s OTHER_KEY=v")]
    #[case::azure_no_key("AZURE_TENANT_ID=abc")]
    #[case::login_newline("atuin\nlogin")]
    #[case::login_no_space("atuinlogin")]
    #[case::login_bare("atuin login")]
    #[case::lowercase_aws("aws_secret_access_key = x")]
    #[case::marker("****")]
    #[case::empty("")]
    #[case::azure_across_lines("AZURE_X\nY_KEY")]
    #[case::login_crlf("atuin\r\nlogin -p hunter2")]
    #[case::login_nbsp("atuin\u{a0}login -p hunter2")]
    #[case::stripe_test_one_short(&format!("sk_test_{}", "a".repeat(23)))]
    fn contains_secret_agrees_with_the_old_set_on_boundary_strings(#[case] s: &str) {
        assert_eq!(contains_secret(s), OLD.is_match(s), "{s:?}");
    }

    /// The contract `redact` relies on: without this group it would not know which part of a match
    /// is the credential and which is the variable name holding it.
    #[test]
    fn every_pattern_names_its_secret_group() {
        for (pattern, regex) in SECRET_PATTERNS.iter().zip(REGEXES.iter()) {
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
    #[case::yaml_literal_block("stringData:\n  AWS_SECRET_ACCESS_KEY: |-\n    wJalr\n")]
    #[case::yaml_folded_block("AWS_SECRET_ACCESS_KEY: >-\n  v")]
    #[case::heredoc("AWS_SESSION_TOKEN = <<EOT\nVALUE\nEOT")]
    #[case::shell_continuation("export AWS_SECRET_ACCESS_KEY=\\\nVALUE")]
    #[case::yaml_tag("AWS_SECRET_ACCESS_KEY: !!binary |\n  d0phbHI=")]
    #[case::yaml_anchor("AWS_SECRET_ACCESS_KEY: &key\n  v")]
    #[case::multiline_json_opener("GOOGLE_SERVICE_ACCOUNT_KEY={\n  \"type\": \"x\"\n}")]
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
    // Starts inside the previous span and runs past it. This is the branch of the splice loop
    // that advances the cursor without writing a second marker; nothing else reaches it.
    #[case::runs_past(&format!("ghp_{}ghs_{}", "a".repeat(33), "b".repeat(36)), "****")]
    fn merges_spans(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(&*redact(input), expected);
    }

    /// Two patterns can carve one stretch of text into spans whose leftovers a second pass would
    /// capture differently: here the type-annotation branch of `assigned!` takes `aAKIA…` for a
    /// type name, the AWS key pattern redacts the key inside it, and the leftover `a****` reads as
    /// a plain value next time round. `redact` runs to a fixed point so that stored output never
    /// changes on re-processing.
    #[rstest]
    #[case::credential_in_a_type_position(
        "AWS_SECRET_ACCESS_KEY: aAKIAIOSFODNN7EXAMPLE => ",
        "AWS_SECRET_ACCESS_KEY: **** => "
    )]
    #[case::credential_in_a_type_position_with_a_value(
        "AWS_SECRET_ACCESS_KEY: xAKIAIOSFODNN7EXAMPLE = y",
        "AWS_SECRET_ACCESS_KEY: **** = ****"
    )]
    fn redaction_reaches_a_fixed_point_in_one_call(#[case] input: &str, #[case] expected: &str) {
        let once = redact(input);
        assert_eq!(&*once, expected);
        assert!(matches!(redact(&once), Cow::Borrowed(_)), "a second pass changed {once:?}");
    }

    proptest! {
        /// Plant credentials between stretches of ordinary text. Every credential must become
        /// exactly one marker, and every stretch around them must survive verbatim -- which pins
        /// the splicing, the sort, and the span boundaries all at once.
        ///
        /// Lowercase filler cannot form a credential of its own (every pattern needs an uppercase
        /// letter, a digit, punctuation or whitespace), and the spaces around each planted value stop a
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
        fn borrowed_exactly_when_nothing_changed(s in credential_dense()) {
            match redact(&s) {
                Cow::Borrowed(out) => prop_assert_eq!(out, s.as_str()),
                Cow::Owned(out) => prop_assert_ne!(out.as_str(), s.as_str()),
            }
        }

        /// Redacting again must be a no-op, or the marker itself would be feeding the patterns.
        #[test]
        fn redaction_is_idempotent(s in credential_dense()) {
            let once = redact(&s).into_owned();
            prop_assert_eq!(&*redact(&once), once.as_str());
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

        /// `contains_secret` must recognise exactly what the old set did. A string that stops
        /// matching would start being stored, and its output captured; one that starts matching
        /// would silently vanish from history.
        #[test]
        fn contains_secret_matches_the_old_pattern_set_exactly(s in credential_dense()) {
            prop_assert_eq!(contains_secret(&s), OLD.is_match(&s), "{:?}", s);
        }

        #[test]
        fn each_prefilter_matches_exactly_where_its_pattern_does(s in credential_dense()) {
            for pattern in SECRET_PATTERNS {
                let Some(prefilter) = pattern.prefilter else { continue };
                prop_assert_eq!(
                    regex::Regex::new(prefilter).unwrap().is_match(&s),
                    regex::Regex::new(pattern.regex).unwrap().is_match(&s),
                    "{}: {:?}", pattern.name, s
                );
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            max_local_rejects: 1_000_000,
            ..ProptestConfig::default()
        })]

        // A filter on the strategy, not `prop_assume!`: with credential-dense input most samples
        // are rejected, and proptest's global-reject cap (1024) does not scale with the case
        // count, so `prop_assume!` would fail the test under PROPTEST_CASES=2000. Local rejects
        // from a filter are capped far higher.
        #[test]
        fn text_with_nothing_recognisable_is_returned_as_is(
            s in credential_dense().prop_filter("contains a secret", |s| !contains_secret(s)),
        ) {
            prop_assert!(matches!(redact(&s), Cow::Borrowed(_)));
        }
    }

    /// Every assignment shape, against every env-var pattern. The four patterns share one suffix
    /// builder precisely so this cannot drift per pattern again; the `contains_secret` assertion
    /// on every shape — including the bare mention — is the should_save guard for all four.
    #[rstest]
    fn every_env_var_pattern_redacts_each_assignment_shape(
        #[values(
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "AZURE_STORAGE_ACCOUNT_KEY",
            "GOOGLE_SERVICE_ACCOUNT_KEY"
        )]
        name: &str,
        #[values(
            ("NAME=wJalrXUtnFEMI", "NAME=****"),
            ("NAME = wJalrXUtnFEMI", "NAME = ****"),
            ("NAME: wJalrXUtnFEMI", "NAME: ****"),
            ("NAME: AQoDYXdzEJr==", "NAME: ****"),
            ("NAME: abc=def", "NAME: ****"),
            ("NAME:abc==", "NAME:****"),
            ("NAME := wJalrXUtnFEMI", "NAME := ****"),
            ("NAME => wJalrXUtnFEMI", "NAME => ****"),
            ("NAME == wJalrXUtnFEMI", "NAME == ****"),
            ("export NAME=\"a b c\"", "export NAME=****"),
            ("NAME='{\"type\": \"service_account\", \"k\": \"v\"}'", "NAME=****"),
            ("{\"NAME\": \"wJalr\", \"X\": 1}", "{\"NAME\": ****, \"X\": 1}"),
            ("{\"NAME\":\"wJalr\",\"X\":1}", "{\"NAME\":****,\"X\":1}"),
            ("'NAME': 'wJalr', 'X': '1'", "'NAME': ****, 'X': '1'"),
            ("os.environ[\"NAME\"] = \"wJalr\"", "os.environ[\"NAME\"] = ****"),
            ("+ \"NAME\" = \"wJalr\"", "+ \"NAME\" = ****"),
            ("NAME    wJalrXUtnFEMI", "NAME    ****"),
            ("NAME\twJalrXUtnFEMI", "NAME\t****"),
            ("│ NAME │ wJalr │", "│ NAME │ **** │"),
            ("NAME: str = \"wJalr\"", "NAME: str = ****"),
            ("const NAME: &str = \"wJalr\";", "const NAME: &str = ****;"),
            ("NAME: Optional[str] = \"wJalr\"", "NAME: Optional[str] = ****"),
            ("NAME=wJalr/K7+MD=", "NAME=****"),
            ("NAME=abc;", "NAME=****;"),
            ("(NAME=abc)", "(NAME=****)"),
            ("NAME=", "NAME="),
            ("NAME= ", "NAME= "),
            ("NAME=\nOTHER=value", "NAME=\nOTHER=value"),
            ("NAME:\n  password: hunter2", "NAME:\n  password: hunter2"),
            ("echo $NAME", "echo $NAME"),
            ("NAME is required", "NAME is required"),
            ("set NAME, and OTHER", "set NAME, and OTHER"),
            ("NAME={\"type\":\"service_account\",\"private_key\":\"-----BEGIN\"}", "NAME=****"),
            ("NAME={\"type\": \"service_account\", \"private_key\": \"-----BEGIN\"}", "NAME=****"),
            ("NAME={\"a\": {\"b\": {\"c\": 1}}} tail", "NAME=**** tail"),
            ("{\"NAME\": {\"a\":1}, \"X\": 1}", "{\"NAME\": ****, \"X\": 1}"),
            ("NAME=[\"a\", \"b\"] rest", "NAME=**** rest"),
            ("│ NAME │ {\"type\": \"service_account\"} │", "│ NAME │ **** │"),
            ("NAME=\x1b[31;1mwJalr\x1b[m", "NAME=****"),
            ("NAME=\x1b[38;2;166;226;46m\"wJalr\"\x1b[m", "NAME=****"),
            ("[\"NAME=V\",\"PATH=/usr/bin\",\"HOME=/root\"]", "[\"NAME=****\",\"PATH=/usr/bin\",\"HOME=/root\"]"),
            ("Environment=\"NAME=V\" \"FOO=bar\"", "Environment=\"NAME=****\" \"FOO=bar\""),
            ("{\"NAME\": \"ab\\\"cd\", \"Expiration\": \"2026\"}", "{\"NAME\": ****, \"Expiration\": \"2026\"}"),
            ("NAME: |wJalr/K7|", "NAME: ****"),
            ("NAME   = ", "NAME   = "),
            ("NAME  =\nOTHER=x", "NAME  =\nOTHER=x"),
            ("pub const NAME: &'static str = \"wJalr\";", "pub const NAME: &'static str = ****;"),
            ("NAME: str | None = \"wJalr\"", "NAME: str | None = ****"),
            ("NAME: dict[str, str] = {\"k\": \"wJalr\"}", "NAME: dict[str, str] = ****"),
            ("NAME: typing.Optional[str] = \"wJalr\"", "NAME: typing.Optional[str] = ****"),
            ("const NAME: string | undefined = 'wJalr';", "const NAME: string | undefined = ****;"),
            ("const NAME: Cow<'static, str> = \"wJalr\";", "const NAME: Cow<'static, str> = ****;"),
            ("NAME ?= wJalr", "NAME ?= ****"),
            ("NAME += wJalr", "NAME += ****"),
            ("$NAME[1]: |wJalr/K7|", "$NAME[1]: ****"),
            ("NAME_JSON={\"type\": \"service_account\"}", "NAME_JSON=****"),
            ("NAME_OLD=wJalr", "NAME_OLD=****"),
            ("{'NAME': 'wJalr', 'X': 'y = z'}", "{'NAME': ****, 'X': 'y = z'}"),
            ("NAME: abc123 AWS_REGION = us-east-1", "NAME: **** AWS_REGION = us-east-1"),
            ("NAME: wJalr, OTHER_TOKEN = tok", "NAME: ****, OTHER_TOKEN = tok"),
            ("NAME: \"wJalr\", \"Expiration\": \"2026 = x\"", "NAME: ****, \"Expiration\": \"2026 = x\""),
            ("NAME: Dict[str, Dict[str, str]] = {\"a\": {\"b\": \"c\"}}", "NAME: Dict[str, Dict[str, str]] = ****"),
            ("NAME={\"abc", "NAME=****"),
            ("NAME=[\"abc", "NAME=****"),
            ("NAME={\"a\":{\"b\":{\"c\":{\"d\":1}}}}", "NAME=****"),
            ("NAME=abc\"def\"", "NAME=****\"def\""),
            ("NAME_FILE=/run/secrets/aws", "NAME_FILE=****"),
        )]
        shape: (&str, &str),
    ) {
        let (input, expected) = (shape.0.replace("NAME", name), shape.1.replace("NAME", name));

        assert_eq!(&*redact(&input), expected, "input {input:?}");
        assert!(contains_secret(&input), "{input:?} must still be recognised");
        assert_eq!(
            matches!(redact(&input), Cow::Borrowed(_)),
            input == expected,
            "Borrowed must mean unchanged for {input:?}"
        );
    }

    /// Every flag spelling against every shape; the password and key patterns share one builder so
    /// that coverage is symmetric between them, which it was not before.
    #[rstest]
    fn every_login_flag_spelling_is_redacted(
        #[values("-p", "--password", "-k", "--key")] flag: &str,
        #[values(
            ("atuin login FLAG hunter2", "atuin login FLAG ****"),
            ("atuin login FLAG=hunter2", "atuin login FLAG=****"),
            ("atuin login FLAG 'hunter two'", "atuin login FLAG ****"),
            ("atuin login FLAG \"hunter two\"", "atuin login FLAG ****"),
            ("atuin login -u me FLAG hunter2 --other x", "atuin login -u me FLAG **** --other x"),
            ("atuin login FLAG \"it's \\\"quoted\\\" here\"", "atuin login FLAG ****"),
            ("atuin login FLAG 'it'\"'\"'s'", "atuin login FLAG ****"),
            ("atuin login FLAG \"hun\"ter2", "atuin login FLAG ****"),
            ("atuin login FLAG \"oops\nls -la\ncat f", "atuin login FLAG ****\nls -la\ncat f"),
            ("atuin login FLAG", "atuin login FLAG"),
            ("atuin login FLAG ****", "atuin login FLAG ****"),
            ("atuin login FLAG \x1b[31;1mhunter2\x1b[m", "atuin login FLAG ****"),
            ("atuin login FLAG=", "atuin login FLAG="),
            ("atuin login -u me \\\n  FLAG hunter2\n", "atuin login -u me \\\n  FLAG ****\n"),
            ("atuin login \\\n\tFLAG hunter2", "atuin login \\\n\tFLAG ****"),
            ("atuin login\nFLAG hunter2", "atuin login\nFLAG ****"),
        )]
        shape: (&str, &str),
    ) {
        let (input, expected) = (shape.0.replace("FLAG", flag), shape.1.replace("FLAG", flag));

        assert_eq!(&*redact(&input), expected, "input {input:?}");
        assert!(contains_secret(&input), "{input:?} must still be recognised");
        assert_eq!(
            matches!(redact(&input), Cow::Borrowed(_)),
            input == expected,
            "Borrowed must mean unchanged for {input:?}"
        );
    }

    /// clap accepts a short flag's value attached: `-phunter2`.
    #[rstest]
    #[case::attached_password("atuin login -u me -phunter2", "atuin login -u me -p****")]
    #[case::attached_key("atuin login -kabc", "atuin login -k****")]
    #[case::both_attached(
        "atuin login --password=hunter2 -kabc",
        "atuin login --password=**** -k****"
    )]
    // A following flag is taken as the value. This was already so and is not worth a lookahead
    // the regex crate does not have; the leak direction is safe (over-redaction).
    #[case::flag_as_value_is_over_redacted("atuin login -p -k y", "atuin login -p **** ****")]
    fn login_attached_and_adjacent_flags(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(&*redact(input), expected);
    }

    #[rstest]
    #[case::long_username_inside_the_window(
        &format!("atuin login --username {} -p hunter2", "a".repeat(400)),
        &format!("atuin login --username {} -p ****", "a".repeat(400))
    )]
    #[case::beyond_the_window_is_left_alone(
        &format!("atuin login {} -p secret", "x".repeat(520)),
        &format!("atuin login {} -p secret", "x".repeat(520))
    )]
    #[case::two_space_table_gap("AWS_SECRET_ACCESS_KEY  wJalr", "AWS_SECRET_ACCESS_KEY  ****")]
    #[case::double_quote_stops_at_the_line_end(
        "AWS_SECRET_ACCESS_KEY=\"abc\nexport OTHER=\"y\"\nls",
        "AWS_SECRET_ACCESS_KEY=****\nexport OTHER=\"y\"\nls"
    )]
    #[case::single_quote_stops_at_the_line_end(
        "AWS_SECRET_ACCESS_KEY='abc\nexport OTHER='y'\nls",
        "AWS_SECRET_ACCESS_KEY=****\nexport OTHER='y'\nls"
    )]
    fn window_gap_and_quote_bounds(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(&*redact(input), expected);
    }

    /// The login patterns' search window is bounded, so a line mentioning `atuin login` many times
    /// costs linear time. Unbounded, this input took about 17 minutes in a debug build (23.6 s release) and
    /// stalled the capture sink long enough to drop later commands' output; bounded it is under
    /// 2 s debug. The budget leaves room for a slow CI box while still catching a return to
    /// quadratic.
    #[test]
    fn many_login_mentions_on_one_line_stay_linear() {
        use std::time::{Duration, Instant};

        let line = "atuin login ".repeat(87_381);
        let started = Instant::now();
        assert!(matches!(redact(&line), Cow::Borrowed(_)));
        let took = started.elapsed();

        assert!(
            took < Duration::from_secs(10),
            "took {took:?}; the login patterns have gone quadratic again"
        );
    }

    fn irregular_login_mentions() -> String {
        let mut line = String::new();
        let mut n = 7u32;
        while line.len() < 1 << 20 {
            line.push_str("atuin login");
            line.push_str(if n.is_multiple_of(3) {
                "  "
            } else {
                " "
            });
            n = n.wrapping_mul(1_103_515_245).wrapping_add(12_345) >> 3;
        }
        line
    }

    #[test]
    fn irregularly_spaced_login_mentions_stay_fast() {
        use std::time::{Duration, Instant};

        let line = irregular_login_mentions();
        let started = Instant::now();
        assert!(matches!(redact(&line), Cow::Borrowed(_)));
        let took = started.elapsed();

        assert!(took < Duration::from_secs(10), "took {took:?}");
    }

    #[test]
    fn a_hostile_capture_does_not_slow_later_scans_on_the_same_thread() {
        use std::time::{Duration, Instant};

        let quiet = "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\n".repeat(25_000);
        let time = |s: &str| {
            let started = Instant::now();
            let _ = redact(s);
            started.elapsed()
        };

        let before = (0..3).map(|_| time(&quiet)).min().unwrap();
        let _ = redact(&irregular_login_mentions());
        let after = time(&quiet);

        assert!(
            after < before * 5 + Duration::from_millis(50),
            "before {before:?}, after {after:?}"
        );
    }

    /// Modern Stripe secret keys are `sk_live_` plus 99 characters; the pattern's floor of 24 is
    /// the old format. A fixed width would redact 24 and leave 75 beside the marker.
    #[rstest]
    #[case::modern_live(&format!("sk_live_{}", "a".repeat(99)), "****")]
    #[case::modern_test(&format!("sk_test_{}", "b".repeat(99)), "****")]
    #[case::old_format_still_exact(concat!("sk_", "live_1234567890abcdefghijklmn"), "****")]
    #[case::followed_by_punctuation(&format!("key={}.", concat!("sk_", "live_1234567890abcdefghijklmn")), "key=****.")]
    fn stripe_keys_of_any_modern_length_are_fully_redacted(
        #[case] input: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(&*redact(input), expected);
    }

    /// One character short, one character wrong, one case wrong. Without these every *broadening*
    /// of a pattern passes the suite, and broadening contains_secret silently drops more commands
    /// from history. The lowercase AWS case also pins that case-sensitivity is deliberate.
    #[rstest]
    #[case::ghp_one_short(&format!("ghp_{}", "a".repeat(35)))]
    #[case::ghs_one_short(&format!("ghs_{}", "a".repeat(35)))]
    #[case::akia_one_short(&format!("AKIA{}", "B".repeat(15)))]
    #[case::lowercase_aws_name("aws_secret_access_key = wJalrXUtnFEMI")]
    #[case::pulumi_uppercase_hex(&format!("pul-{}", "ABCDEF0123".repeat(4)))]
    #[case::v1_unescaped_dot(&format!("v1x{}", "0".repeat(40)))]
    #[case::github_pat_no_leading_digit(&format!("github_pat_A{}_{}", "a".repeat(21), "b".repeat(59)))]
    #[case::login_without_whitespace("atuinlogin -p x")]
    #[case::stripe_one_short(&format!("sk_live_{}", "a".repeat(23)))]
    #[case::slack_webhook_short_team(&format!("T1234567/B12345678/{}", "x".repeat(24)))]
    #[case::slack_webhook_wrong_team_prefix(&format!("U00000000/B00000000/{}", "x".repeat(24)))]
    #[case::slack_bot_short_first_group(&format!("xoxb-1234567890-12345678901-{}", "x".repeat(24)))]
    #[case::npm_one_short(&format!("npm_{}", "a".repeat(35)))]
    #[case::netlify_unknown_kind(&format!("nfx_{}", "a".repeat(36)))]
    #[case::stripe_test_one_short(&format!("sk_test_{}", "a".repeat(23)))]
    fn near_misses_are_not_credentials(#[case] input: &str) {
        assert!(!contains_secret(input), "{input:?} should not be recognised");
        assert!(matches!(redact(input), Cow::Borrowed(_)), "{input:?} should be left alone");
    }

    /// The table-driven tests and the planted-credential properties only cover a pattern if it
    /// brings its own cases. Without this floor, `tests: &[]` silently removes a pattern from all
    /// of them and the suite stays green.
    #[test]
    fn every_pattern_has_a_case_that_actually_redacts() {
        for pattern in SECRET_PATTERNS {
            assert!(!pattern.tests.is_empty(), "{} has no test cases", pattern.name);
            assert!(
                pattern.tests.iter().any(|test| test.input != test.redacted),
                "{} has no case in which anything is redacted",
                pattern.name
            );
        }
    }

    #[rstest]
    #[case::key("atuin key", true)]
    #[case::key_base64("atuin key --base64", true)]
    #[case::highlighted_account_login("\x1b[32matuin\x1b[0m account login -u me", true)]
    #[case::account_register("atuin account register", true)]
    #[case::register("atuin register -u x", true)]
    #[case::change_password("atuin account change-password", true)]
    #[case::login("atuin login -u me", true)]
    #[case::keys_subcommand("atuin keys list", false)]
    #[case::history("atuin history list", false)]
    #[case::unrelated_key_word("cat keyfile", false)]
    #[case::plain("ls", false)]
    fn commands_whose_output_holds_the_encryption_key(
        #[case] command: &str,
        #[case] unsafe_: bool,
    ) {
        assert_eq!(output_unsafe(command), unsafe_);
    }
}
