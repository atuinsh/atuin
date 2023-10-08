// This file will probably trigger a lot of scanners. Sorry.

// A list of (name, regex, test), where test should match against regex
pub static SECRET_PATTERNS: &[(&str, &str, &str)] = &[
    (
        "AWS Access Key ID",
        "AKIA[0-9A-Z]{16}",
        "AKIAIOSFODNN7EXAMPLE",
    ),
    (
        "GitHub PAT (old)",
        "^ghp_[a-zA-Z0-9]{36}$",
        "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", // legit, I expired it
    ),
    (
        "GitHub PAT (new)",
        "^github_pat_[a-zA-Z0-9]{22}_[a-zA-Z0-9]{59}$",
        "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", // also legit, also expired
    ),
    (
        "Slack OAuth v2 bot",
        "xoxb-[0-9]{11}-[0-9]{11}-[0-9a-zA-Z]{24}",
        "xoxb-17653672481-19874698323-pdFZKVeTuE8sk7oOcBrzbqgy",
    ),
    (
        "Slack OAuth v2 user token",
        "xoxp-[0-9]{11}-[0-9]{11}-[0-9a-zA-Z]{24}",
        "xoxp-17653672481-19874698323-pdFZKVeTuE8sk7oOcBrzbqgy",
    ),
    (
        "Slack webhook",
        "T[a-zA-Z0-9_]{8}/B[a-zA-Z0-9_]{8}/[a-zA-Z0-9_]{24}",
        "https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX",
    ),
    ("Stripe test key", "sk_test_[0-9a-zA-Z]{24}", "sk_test_1234567890abcdefghijklmnop"),
    ("Stripe live key", "sk_live_[0-9a-zA-Z]{24}", "sk_live_1234567890abcdefghijklmnop"),
];

#[cfg(test)]
mod tests {
    use regex::Regex;

    use crate::secrets::SECRET_PATTERNS;

    #[test]
    fn test_secrets() {
        for (name, regex, test) in SECRET_PATTERNS {
            let re =
                Regex::new(regex).unwrap_or_else(|_| panic!("Failed to compile regex for {name}"));

            assert!(re.is_match(test), "{name} test failed!");
        }
    }
}
