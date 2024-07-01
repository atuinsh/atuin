// This file will probably trigger a lot of scanners. Sorry.

pub enum TestValue<'a> {
    Single(&'a str),
    Multiple(&'a [&'a str]),
}

// A list of (name, regex, test), where test should match against regex
pub static SECRET_PATTERNS: &[(&str, &str, TestValue)] = &[
    (
        "AWS Access Key ID",
        "AKIA[0-9A-Z]{16}",
        TestValue::Single("AKIAIOSFODNN7EXAMPLE"),
    ),
    (
        "AWS secret access key env var",
        "AWS_ACCESS_KEY_ID",
        TestValue::Single("export AWS_ACCESS_KEY_ID=KEYDATA"),
    ),
    (
        "AWS secret access key env var",
        "AWS_ACCESS_KEY_ID",
        TestValue::Single("export AWS_ACCESS_KEY_ID=KEYDATA"),
    ),
    (
        "Microsoft Azure secret access key env var",
        "AZURE_.*_KEY",
        TestValue::Single("export AZURE_STORAGE_ACCOUNT_KEY=KEYDATA"),
    ),
    (
        "Google cloud platform key env var",
        "GOOGLE_SERVICE_ACCOUNT_KEY",
        TestValue::Single("export GOOGLE_SERVICE_ACCOUNT_KEY=KEYDATA"),
    ),
    (
        "Atuin login",
        r"atuin\s+login",
        TestValue::Single("atuin login -u mycoolusername -p mycoolpassword -k \"lots of random words\""),
    ),
    (
        "GitHub PAT (old)",
        "ghp_[a-zA-Z0-9]{36}",
        TestValue::Single("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"), // legit, I expired it
    ),
    (
        "GitHub PAT (new)",
        "gh1_[A-Za-z0-9]{21}_[A-Za-z0-9]{59}|github_pat_[0-9][A-Za-z0-9]{21}_[A-Za-z0-9]{59}",
        TestValue::Multiple(&[
            "gh1_1234567890abcdefghijk_1234567890abcdefghijklmnopqrstuvwxyz1234567890abcdefghijklm",
            "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", // also legit, also expired
        ])
    ),
    (
        "GitHub OAuth Access Token",
        "gho_[A-Za-z0-9]{36}",
        TestValue::Single("gho_1234567890abcdefghijklmnopqrstuvwx000"),  // not a real token
    ),
    (
        "GitHub OAuth Access Token (user)",
        "ghu_[A-Za-z0-9]{36}",
        TestValue::Single("ghu_1234567890abcdefghijklmnopqrstuvwx000"),  // not a real token
    ),
    (
        "GitHub App Installation Access Token",
        "ghs_[A-Za-z0-9]{36}",
        TestValue::Single("ghs_1234567890abcdefghijklmnopqrstuvwx000"),  // not a real token
    ),
    (
        "GitHub Refresh Token",
        "ghr_[A-Za-z0-9]{76}",
        TestValue::Single("ghr_1234567890abcdefghijklmnopqrstuvwx1234567890abcdefghijklmnopqrstuvwx1234567890abcdefghijklmnopqrstuvwx"),  // not a real token
    ),
    (
        "GitHub App Installation Access Token v1",
        "v1\\.[0-9A-Fa-f]{40}",
        TestValue::Single("v1.1234567890abcdef1234567890abcdef12345678"),  // not a real token
    ),
    (
        "GitLab PAT",
        "glpat-[a-zA-Z0-9_]{20}",
        TestValue::Single("glpat-RkE_BG5p_bbjML21WSfy"),
    ),
    (
        "Slack OAuth v2 bot",
        "xoxb-[0-9]{11}-[0-9]{11}-[0-9a-zA-Z]{24}",
        TestValue::Single("xoxb-17653672481-19874698323-pdFZKVeTuE8sk7oOcBrzbqgy"),
    ),
    (
        "Slack OAuth v2 user token",
        "xoxp-[0-9]{11}-[0-9]{11}-[0-9a-zA-Z]{24}",
        TestValue::Single("xoxp-17653672481-19874698323-pdFZKVeTuE8sk7oOcBrzbqgy"),
    ),
    (
        "Slack webhook",
        "T[a-zA-Z0-9_]{8}/B[a-zA-Z0-9_]{8}/[a-zA-Z0-9_]{24}",
        TestValue::Single("https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX"),
    ),
    ("Stripe test key", "sk_test_[0-9a-zA-Z]{24}", TestValue::Single("sk_test_1234567890abcdefghijklmnop")),
    ("Stripe live key", "sk_live_[0-9a-zA-Z]{24}", TestValue::Single("sk_live_1234567890abcdefghijklmnop")),
    (
        "Netlify authentication token",
        "nf[pcoub]_[0-9a-zA-Z]{36}",
        TestValue::Single("nfp_nBh7BdJxUwyaBBwFzpyD29MMFT6pZ9wq5634"),
    ),
    (
        "npm token",
        "npm_[A-Za-z0-9]{36}",
        TestValue::Single("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"),
    ),
    (
        "Pulumi personal access token",
        "pul-[0-9a-f]{40}",
        TestValue::Single("pul-683c2770662c51d960d72ec27613be7653c5cb26"),
    ),
];

#[cfg(test)]
mod tests {
    use regex::Regex;

    use crate::secrets::{TestValue, SECRET_PATTERNS};

    #[test]
    fn test_secrets() {
        for (name, regex, test) in SECRET_PATTERNS {
            let re =
                Regex::new(regex).unwrap_or_else(|_| panic!("Failed to compile regex for {name}"));

            match test {
                TestValue::Single(test) => {
                    assert!(re.is_match(test), "{name} test failed!");
                }
                TestValue::Multiple(tests) => {
                    for test_str in tests.iter() {
                        assert!(
                            re.is_match(test_str),
                            "{name} test with value \"{test_str}\" failed!"
                        );
                    }
                }
            }
        }
    }
}
