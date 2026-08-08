// This file will probably trigger a lot of scanners. Sorry.

use regex::RegexSet;
use std::sync::LazyLock;

pub enum TestValue<'a> {
    Single(&'a str),
    Multiple(&'a [&'a str]),
}

/// A list of `(name, regex, test)`, where `test` should match against `regex`.
pub static SECRET_PATTERNS: &[(&str, &str, TestValue)] = &[
    (
        "AWS Access Key ID",
        "A[KS]IA[0-9A-Z]{16}",
        TestValue::Single("AKIAIOSFODNN7EXAMPLE"),
    ),
    (
        "AWS Secret Access Key env var",
        "AWS_SECRET_ACCESS_KEY",
        TestValue::Single("AWS_SECRET_ACCESS_KEY=KEYDATA"),
    ),
    (
        "AWS Session Token env var",
        "AWS_SESSION_TOKEN",
        TestValue::Single("AWS_SESSION_TOKEN=KEYDATA"),
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
        TestValue::Single(
            "atuin login -u mycoolusername -p mycoolpassword -k \"lots of random words\"",
        ),
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
        ]),
    ),
    (
        "GitHub OAuth Access Token",
        "gho_[A-Za-z0-9]{36}",
        TestValue::Single("gho_1234567890abcdefghijklmnopqrstuvwx000"), // not a real token
    ),
    (
        "GitHub OAuth Access Token (user)",
        "ghu_[A-Za-z0-9]{36}",
        TestValue::Single("ghu_1234567890abcdefghijklmnopqrstuvwx000"), // not a real token
    ),
    (
        "GitHub App Installation Access Token",
        "ghs_[A-Za-z0-9._-]{36,}",
        TestValue::Multiple(&[
            "ghs_1234567890abcdefghijklmnopqrstuvwx000", // not a real token
            "ghs_abc-def.ghi_jklMNOP0123456789qrstuv-wxyzABCD", // new token format, fake data
        ]),
    ),
    (
        "GitHub Refresh Token",
        "ghr_[A-Za-z0-9]{76}",
        TestValue::Single(
            "ghr_1234567890abcdefghijklmnopqrstuvwx1234567890abcdefghijklmnopqrstuvwx1234567890abcdefghijklmnopqrstuvwx",
        ), // not a real token
    ),
    (
        "GitHub App Installation Access Token v1",
        "v1\\.[0-9A-Fa-f]{40}",
        TestValue::Single("v1.1234567890abcdef1234567890abcdef12345678"), // not a real token
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
        "T[a-zA-Z0-9_]{8,12}/B[a-zA-Z0-9_]{8,12}/[a-zA-Z0-9_]{24}",
        TestValue::Multiple(&[
            "https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX",
            // Slack has since grown its team/bot IDs past the original 9 characters
            "https://hooks.slack.com/services/T00000000A00/B00000000A00/XXXXXXXXXXXXXXXXXXXXXXXX",
        ]),
    ),
    (
        "Stripe test key",
        "sk_test_[0-9a-zA-Z]{24}",
        TestValue::Single("sk_test_1234567890abcdefghijklmnop"),
    ),
    (
        "Stripe live key",
        "sk_live_[0-9a-zA-Z]{24}",
        TestValue::Single("sk_live_1234567890abcdefghijklmnop"),
    ),
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
    (
        // Covers Anthropic (sk-ant-…), OpenAI (sk-proj-…, sk-…), OpenRouter (sk-or-v1-…)
        // and the many smaller providers that copied the prefix.
        "`sk-` prefixed API key",
        "sk-[A-Za-z0-9_-]{32,}",
        TestValue::Multiple(&[
            "sk-ant-api03-0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000AA",
            "sk-proj-000000000000000000000000000000000000000000000000",
            "sk-or-v1-0000000000000000000000000000000000000000000000000000000000000000",
        ]),
    ),
    (
        "Atuin Hub API token",
        "atapi_[0-9a-f]{48}",
        TestValue::Single("atapi_000000000000000000000000000000000000000000000000"),
    ),
    (
        "Tailscale key",
        "tskey-(?:auth|api|client|scim)-[A-Za-z0-9]{10,}-[A-Za-z0-9]{10,}",
        TestValue::Single("tskey-auth-k0000000000CNTRL-m00000000000000000000000000"),
    ),
    (
        "PlanetScale password/token",
        "pscale_(?:pw|tkn|oauth)_[A-Za-z0-9_-]{32,}",
        TestValue::Single("pscale_pw_00000000000000000000000000000000000000"),
    ),
    (
        "Google OAuth client secret",
        "GOCSPX-[A-Za-z0-9_-]{28}",
        TestValue::Single("GOCSPX-0000000000000000000000000000"),
    ),
    (
        "PostHog API key",
        "ph[cxsp]_[A-Za-z0-9]{40,}",
        TestValue::Single("phc_0000000000000000000000000000000000000000000"),
    ),
    (
        "Resend API key",
        "re_[A-Za-z0-9]{8,}_[A-Za-z0-9]{20,}",
        TestValue::Single("re_00000000_000000000000000000000000"),
    ),
    (
        "Tigris storage key",
        "tid_[A-Za-z0-9_-]{32,}|tsec_[A-Za-z0-9_-]{48,}",
        TestValue::Multiple(&[
            "tid_eh_00000000000000000000000000000000000000000000000",
            "tsec_000000000000000000000000000000000000000000000000000000000000000000000000",
        ]),
    ),
    (
        "Fireworks AI API key",
        "fw_[A-Za-z0-9]{20,}",
        TestValue::Single("fw_00000000000000000000000"),
    ),
    (
        "Firecrawl API key",
        "fc-[0-9a-f]{32}",
        TestValue::Single("fc-00000000000000000000000000000000"),
    ),
    (
        "JSON web token",
        r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
        TestValue::Single(
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIwMDAwMDAwMCJ9.0000000000000000000000000000000",
        ),
    ),
    (
        "Phoenix signed token",
        r"SFMyNTY\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}",
        TestValue::Single(
            "SFMyNTY.g2gDbQAAACQwMDAwMDAwMC0wMDAwbgYA00000000dwhpbmZpbml0eQ.0000000000000000000000000000000000000000000",
        ),
    ),
    (
        "Connection URI with inline credentials",
        r"\b(?:postgres|postgresql|mysql|mariadb|mongodb\+srv|mongodb|rediss|redis|amqps|amqp|clickhouse|https|http)://[^\s:@/]+:[^\s:@/]+@",
        TestValue::Multiple(&[
            "psql 'postgres://someuser:hunter2@db.example.com:6432/mydb?sslmode=require'",
            "http://default:hunter2@10.0.0.1:8123/analytics",
        ]),
    ),
    (
        "Authorization header with a literal token",
        r"(?i)authorization:\s*(?:bearer|basic|token)\s+[A-Za-z0-9][A-Za-z0-9._~+/=-]{15,}",
        TestValue::Single(
            r#"curl -H "Authorization: Bearer 000000000000000000000000000000000000""#,
        ),
    ),
    (
        // `kubectl create secret generic … --from-literal=…` puts the secret value
        // straight on the command line.
        "Kubernetes secret literal",
        r"--from-literal[= ]",
        TestValue::Single("kubectl create secret generic mysecret --from-literal=password=hunter2"),
    ),
    (
        // `kubectl` is required so this doesn't swallow every command that merely
        // mentions creating a secret.
        "Kubernetes secret create/patch",
        r"kubectl\s.*\b(?:create|patch|replace|edit)\s+secrets?\b",
        TestValue::Multiple(&[
            "kubectl -n hub create secret generic hub-secret",
            "kubectl -n hub patch secret hub-secret --type merge -p '{}'",
        ]),
    ),
    (
        // Catch-all for the common `NAME=value` shape, whichever vendor it belongs to.
        "Secret-shaped environment variable assignment",
        r"[A-Z][A-Z0-9_]*(?:SECRET|TOKEN|PASSWORD|PASSWD|API_?KEY|ACCESS_KEY|CREDENTIALS?)[A-Z0-9_]*\s*=\s*\S",
        TestValue::Multiple(&[
            "export ANTHROPIC_API_KEY=hunter2",
            "PGPASSWORD=hunter2 psql -h db.example.com",
            "--from-literal=OAUTH_GH_CLIENT_SECRET='hunter2'",
        ]),
    ),
];

/// Commands that must **not** be caught by [`SECRET_PATTERNS`].
///
/// The secrets filter drops matching commands from history entirely, so a false
/// positive silently loses real history. Anything added here is a shape that
/// looks secret-adjacent but isn't.
#[cfg(test)]
static NON_SECRETS: &[&str] = &[
    "export PATH=\"$PATH:~/bin/\"",
    "export RUSTC_WRAPPER=sccache",
    "echo $ANTHROPIC_API_KEY",
    "unset GITHUB_TOKEN",
    "psql 'postgres://localhost:5432/atuin_dev'",
    "psql 'postgres://ellie@localhost:5432/atuin_dev'",
    "curl -H \"Authorization: Bearer $FIRECRAWL_API\" http://localhost:4000",
    "curl -i -H \"Authorization: Bearer ${GITHUB_TEST}\" https://api.github.com/repos/atuinsh/infra",
    "kubectl -n hub get secret hub-secret -o yaml",
    "kubectl patch pvc server-volume-0 -n monitoring -p '{\"spec\":{}}'",
    "docker run -u 1000:1000 --rm alpine",
    "git commit -m 'create secret scanning for the new endpoint'",
    "git clone git@github.com:nixpulvis/oursh.git",
    "cargo run -p atuin -- search -i",
];

/// The `regex` expressions from [`SECRET_PATTERNS`] compiled into a `RegexSet`.
pub static SECRET_PATTERNS_RE: LazyLock<RegexSet> = LazyLock::new(|| {
    let exprs = SECRET_PATTERNS.iter().map(|f| f.1);
    RegexSet::new(exprs).expect("Failed to build secrets regex")
});

#[cfg(test)]
mod tests {
    use regex::Regex;
    use rstest::rstest;

    use crate::secrets::{NON_SECRETS, SECRET_PATTERNS, SECRET_PATTERNS_RE, TestValue};

    #[test]
    fn non_secrets() {
        for command in NON_SECRETS {
            let matches = SECRET_PATTERNS_RE.matches(command);
            let names: Vec<_> = matches.into_iter().map(|i| SECRET_PATTERNS[i].0).collect();
            assert!(
                names.is_empty(),
                "\"{command}\" is not a secret, but matched: {names:?}"
            );
        }
    }

    #[rstest]
    fn secrets(#[values(false, true)] embed: bool) {
        for (name, regex, test) in SECRET_PATTERNS {
            let re =
                Regex::new(regex).unwrap_or_else(|_| panic!("Failed to compile regex for {name}"));

            let label = if embed { "embedded test" } else { "test" };
            let wrap = |s: &str| {
                if embed {
                    format!("some random text {s} some more random text")
                } else {
                    s.to_string()
                }
            };

            match test {
                TestValue::Single(test) => {
                    assert!(re.is_match(&wrap(test)), "{name} {label} failed!");
                }
                TestValue::Multiple(tests) => {
                    for test_str in tests.iter() {
                        assert!(
                            re.is_match(&wrap(test_str)),
                            "{name} {label} with value \"{test_str}\" failed!"
                        );
                    }
                }
            }
        }
    }
}
