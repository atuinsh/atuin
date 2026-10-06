//! Credentials recognised by where they sit rather than by a format of their own: a private key's
//! armour, a URL's password, an `Authorization` header, a value assigned to a name like
//! `DB_PASSWORD` or `"api_key"`. Several follow
//! [gitleaks](https://github.com/gitleaks/gitleaks/blob/master/config/gitleaks.toml) (MIT).
//!
//! These only redact: unlike the formats in the parent module and in `vendors`, they don't decide
//! what history keeps (`secrets_filter`). A value that is plainly not a credential -- a placeholder
//! (`${TOKEN}`, `<password>`, `changeme`), a type (`token: String`), a number -- is left as it is
//! (see [`is_placeholder`] and [`is_word`]).

use super::Pattern;
#[cfg(test)]
use super::REDACTED;
#[cfg(test)]
use super::tests::Test;

/// The words a name ends with when it holds a credential, as an environment variable spells them.
macro_rules! env_secret_name {
    () => {
        r"(?-u:\b)[A-Z0-9_]*(?:SECRET|TOKEN|PASSWORD|PASSWD|_PWD|API_?KEY|APIKEY|PRIVATE_KEY|ACCESS_KEY|SECRET_KEY|CREDENTIALS?|AUTH)S?(?-u:\b)"
    };
}

/// The same, as code, JSON and config files spell them (`apiKey`, `"client_secret"`,
/// `db.password`). Not `auth` alone (`"auth": "required"`) nor `tokens` (`max_tokens`).
macro_rules! code_secret_name {
    () => {
        r"(?i-u:[\w.-]*?(?:passw(?:or)?d|secret|api[_-]?key|private[_-]?key|access[_-]?key|(?:auth|access|refresh|bearer|id|session)[_-]?token|token))"
    };
}

/// An assignment's separator: `=`, `:`, `:=`, `=>`, `?=`, with a closing quote or `]` before it.
macro_rules! separator {
    () => {
        r#"["'\]]?[ \t]*(?::=|=>|\?=|[=:])[ \t]*"#
    };
}

/// A quoted value of at least six characters, quotes included.
macro_rules! quoted {
    () => {
        r#""(?:[^"\\\n]|\\.){6,}"|'[^'\n]{6,}'"#
    };
}

pub(super) static STRUCTURAL_PATTERNS: &[Pattern] = &[
    // gitleaks private-key; the armour lines stay, so it still says what was there. A key cut off
    // before its end line (clipped output) loses the rest.
    Pattern {
        name: "Private key",
        regex: r"-----BEGIN[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----(?<secret>(?s:.+?))(?:-----END[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----|\z)",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!(
                    "-----BEGIN OPENSSH PRIVATE ",
                    "KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ\nAAAAAAAAAB\n-----END \
                     OPENSSH PRIVATE KEY-----"
                ),
                redacted: "-----BEGIN OPENSSH PRIVATE KEY-----****-----END OPENSSH PRIVATE \
                           KEY-----",
            },
            // Escaped, in JSON.
            Test {
                input: concat!(
                    "-----BEGIN RSA PRIVATE ",
                    "KEY-----\\nMIIEowIBAAKCAQEA7\\n-----END RSA PRIVATE KEY-----"
                ),
                redacted: "-----BEGIN RSA PRIVATE KEY-----****-----END RSA PRIVATE KEY-----",
            },
        ],
    },
    // gitleaks jwt
    Pattern {
        name: "JSON web token",
        regex: r"(?-u:\b)(?<secret>eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: concat!(
                "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
                ".eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"
            ),
            redacted: REDACTED,
        }],
    },
    Pattern {
        name: "Bearer token",
        regex: r"(?i-u:\bbearer)[ \t]+(?<secret>[A-Za-z0-9._~+/-]{16,}=*)",
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: "Authorization: Bearer 8f2d0c1e9b7a4f6e8d2c",
            redacted: "Authorization: Bearer ****",
        }],
    },
    Pattern {
        name: "Authorization header",
        regex: r#"(?i-u:\bauthorization)["']?[ \t]*[:=][ \t]*["']?(?i-u:basic|token)[ \t]+(?<secret>[A-Za-z0-9._~+/-]{8,}=*)"#,
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: "-H 'Authorization: Basic dXNlcjpodW50ZXIy'",
            redacted: "-H 'Authorization: Basic ****'",
        }],
    },
    // gitleaks curl-auth-user
    Pattern {
        name: "curl user password",
        regex: r#"(?-u:\bcurl\b)[^\n]*?[ \t](?:-u|--user)(?:=|[ \t]+)["']?[^:\s"']*:(?<secret>[^\s"']+)"#,
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: "curl -u admin:hunter2 https://example.com",
            redacted: "curl -u admin:**** https://example.com",
        }],
    },
    // A URL's userinfo: postgres://user:pass@host, redis://:pass@host, https://u:p@h.
    Pattern {
        name: "URL password",
        regex: r#"(?-u:\b)(?i-u:[a-z][a-z0-9+.-]{1,31})://[^\s/?#@"'`<>:]*:(?<secret>[^\s/?#@"'`<>]+)@"#,
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: "DATABASE_URL=postgres://app:s3cr3t-pw@db.internal:5432/app",
                redacted: "DATABASE_URL=postgres://app:****@db.internal:5432/app",
            },
            Test {
                input: "redis://:hunter22@localhost:6379/0",
                redacted: "redis://:****@localhost:6379/0",
            },
            Test {
                input: "postgres://app:123456@db/app",
                redacted: "postgres://app:****@db/app",
            },
        ],
    },
    // A connection string's `password=` (keyword DSNs, JDBC and ODBC strings, query strings).
    Pattern {
        name: "Connection string password",
        regex: r#"(?:^|[?&;\s])(?i-u:password|pwd)=(?<secret>"[^"\n]*"|'[^'\n]*'|[^&;\s"']+)"#,
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: "host=db user=app password=hunter22 dbname=app",
                redacted: "host=db user=app password=**** dbname=app",
            },
            Test {
                input: "Server=db;User Id=sa;Password=Hunter2!;",
                redacted: "Server=db;User Id=sa;Password=****;",
            },
        ],
    },
    // gitleaks twilio-api-key: only redacted, as `SK` and 32 hex digits is too plain a shape to
    // keep a command out of history.
    Pattern {
        name: "Twilio API key",
        regex: r"(?-u:\b)(?<secret>SK[0-9a-fA-F]{32})(?-u:\b)",
        prefilter: None,
        #[cfg(test)]
        tests: &[Test {
            input: concat!("SK", "0f9e8d7c6b5a49382716051f2e3d4c5b"),
            redacted: REDACTED,
        }],
    },
];

/// Values assigned to a name a credential is kept under. A value of one of these that is a word
/// (see [`is_word`]) is kept too.
pub(super) static NAMED_PATTERNS: &[Pattern] = &[
    Pattern {
        name: "Credential variable",
        regex: concat!(
            env_secret_name!(),
            separator!(),
            r#"(?<secret>"#,
            quoted!(),
            r#"|[^\s"'`,;)\]}{(<>]{6,})"#
        ),
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: "export GITHUB_TOKEN=4f1c0e7d9b2a",
                redacted: "export GITHUB_TOKEN=****",
            },
            Test {
                input: "DB_PASSWORD: \"correct horse 9\"",
                redacted: "DB_PASSWORD: ****",
            },
            Test {
                input: "DB_PASSWORD=123456",
                redacted: "DB_PASSWORD=****",
            },
            Test {
                input: "ADMIN_PASSWORD=Violetmeadow",
                redacted: "ADMIN_PASSWORD=****",
            },
            // A reference, not a value.
            Test {
                input: "OPENAI_API_KEY=${OPENAI_API_KEY}",
                redacted: "OPENAI_API_KEY=${OPENAI_API_KEY}",
            },
        ],
    },
    Pattern {
        name: "Credential key",
        regex: concat!(
            r#"(?:^|[^A-Za-z0-9_.-])["']?"#,
            code_secret_name!(),
            separator!(),
            r#"(?<secret>"#,
            quoted!(),
            ")"
        ),
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: r#"{"client_secret": "a8f3kq0z7x1m", "client_id": "app"}"#,
                redacted: r#"{"client_secret": ****, "client_id": "app"}"#,
            },
            Test {
                input: "const apiKey = 'live-9f8e7d6c';",
                redacted: "const apiKey = ****;",
            },
            Test {
                input: r#"{"password": "<password>"}"#,
                redacted: r#"{"password": "<password>"}"#,
            },
            // A word, quoted, is a literal.
            Test {
                input: r#"{"password": "violetmeadow"}"#,
                redacted: r#"{"password": ****}"#,
            },
        ],
    },
    // YAML, `.env`, INI and `.properties` lines: an unquoted value to the end of the line.
    Pattern {
        name: "Credential setting",
        regex: concat!(
            r#"(?m)^[ \t]*(?:-[ \t]+)?["']?"#,
            code_secret_name!(),
            r#"["']?[ \t]*[:=][ \t]*(?<secret>[^\s"'#|>&*!{}\[\],=][^\n#="]*?)[ \t]*$"#
        ),
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: "database:\n  password: hunter2\n  host: db",
                redacted: "database:\n  password: ****\n  host: db",
            },
            Test {
                input: "\nspring.datasource.password=s3cret!\n",
                redacted: "\nspring.datasource.password=****\n",
            },
            // A struct's field, not a setting.
            Test {
                input: "    token: String,",
                redacted: "    token: String,",
            },
            Test {
                input: "\npassword: violetmeadow\n",
                redacted: "\npassword: ****\n",
            },
        ],
    },
];

/// Values a structural pattern finds that are plainly not credentials: references to one
/// (`$TOKEN`, `${{ secrets.X }}`, `os.environ[...]`), documentation's stand-ins (`<password>`,
/// `changeme`, `xxxx`), the marker itself, and numbers.
pub(super) fn is_placeholder(value: &str) -> bool {
    let value = value.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if value.is_empty() {
        return true;
    }
    let lower = value.to_ascii_lowercase();
    let reference = ["$", "%", "{{", "os.environ", "process.env", "env.", "env(", "env[", "getenv"];
    if reference.iter().any(|r| lower.starts_with(r)) {
        return true;
    }
    if value.len() > 2
        && value.starts_with('<')
        && value.ends_with('>')
        && value[1..value.len() - 1].chars().all(|c| c.is_ascii_alphabetic() || "_- ".contains(c))
    {
        return true;
    }
    let first = value.chars().next().unwrap_or(' ');
    if value.len() >= 3 && "*x.-•".contains(first) && value.chars().all(|c| c == first) {
        return true;
    }
    const STAND_INS: &[&str] = &[
        "changeme",
        "change_me",
        "change-me",
        "example",
        "password",
        "passwd",
        "secret",
        "token",
        "your_password",
        "your-password",
        "yourpassword",
        "your_token",
        "your_api_key",
        "redacted",
        "placeholder",
        "dummy",
        "undefined",
        "required",
        "optional",
    ];
    if STAND_INS.contains(&lower.as_str()) {
        return true;
    }
    false
}

/// [`is_placeholder`], or, unquoted, a type: what a name's value in code is far more often than
/// a credential (`token: String,`, `Option<String>`, `typing.Optional[str]`, `str`). A quoted
/// value is a literal, and a credential whatever it looks like; so is a number or a word.
pub(super) fn is_word(value: &str) -> bool {
    const TYPES: &[&str] = &[
        "str", "string", "bool", "boolean", "int", "integer", "number", "float", "any", "object",
        "bytes", "char", "none", "null", "nil", "true", "false",
    ];
    let value = value.trim();
    if is_placeholder(value) {
        return true;
    }
    if value.starts_with(['"', '\'']) || value.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    value.contains(['<', '>', '[', ']', '&', ':', '(', ')', '{', '}', ',', ';'])
        || TYPES.contains(&value.to_ascii_lowercase().as_str())
}
