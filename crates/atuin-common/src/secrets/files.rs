//! Files whose contents are credentials (`.env`, `~/.ssh/id_ed25519`, `~/.aws/credentials`, an
//! agent's own login), so that what reads them out, or writes to them, is not kept: a coding
//! agent's file reads and writes, and a command naming one (`cat .env`).
//!
//! Redaction catches the credentials it knows the format or the name of; a file like these holds
//! nothing else, in whatever format, so nothing read out of one is kept at all.

use std::path::{Path, PathBuf};

use glob_match::glob_match;

/// Files whose name alone says they hold credentials, wherever they are.
const NAMES: &[&str] = &[
    ".env",
    ".env.*",
    "*.env",
    ".envrc",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "id_ecdsa_sk",
    "id_ed25519_sk",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.jks",
    "*.keystore",
    "*.tfstate",
    "*.tfstate.backup",
    "*.tfvars",
    ".netrc",
    "_netrc",
    ".pgpass",
    ".git-credentials",
    ".npmrc",
    ".pypirc",
    ".htpasswd",
    ".vault-token",
];

/// Of [`NAMES`], the ones that are a template or a public half, not a credential. The user's own
/// names aren't excepted: one they name is one they mean.
const NOT_NAMES: &[&str] = &["*.example", "*.sample", "*.template", "*.dist", "*.pub"];

/// Files holding credentials where a tool keeps them: cloud and package registry logins, and the
/// coding agents' own.
const PATHS: &[&str] = &[
    "~/.aws/credentials",
    "~/.aws/sso/cache/**",
    "~/.aws/cli/cache/**",
    "~/.config/gcloud/application_default_credentials.json",
    "~/.config/gcloud/credentials.db",
    "~/.config/gcloud/access_tokens.db",
    "~/.config/gcloud/legacy_credentials/**",
    "~/.azure/msal_token_cache.*",
    "~/.kube/config",
    "~/.docker/config.json",
    "~/.config/gh/hosts.yml",
    "~/.config/glab-cli/config.yml",
    "~/.config/hub",
    "~/.terraform.d/credentials.tfrc.json",
    "~/.cargo/credentials",
    "~/.cargo/credentials.toml",
    "~/.gem/credentials",
    "~/.gnupg/private-keys-v1.d/**",
    "~/.password-store/**",
    "~/.claude/.credentials.json",
    "~/.codex/auth.json",
    "~/.local/share/opencode/auth.json",
    "~/.pi/agent/auth.json",
];

/// Which files hold credentials: the built-in ones, and the user's (`[security] sensitive_files`).
#[derive(Clone, Debug)]
pub struct SensitiveFiles {
    /// Globs matched against a file's name: the built-in ones, then the user's.
    names: Vec<String>,
    /// Globs matched against a file's whole path, `~` expanded.
    paths: Vec<String>,
    home: Option<PathBuf>,
}

impl Default for SensitiveFiles {
    fn default() -> Self {
        Self::new(&[], &[], dirs::home_dir())
    }
}

impl SensitiveFiles {
    /// The built-in files, `extra` exact paths (Atuin's own key), and the user's `patterns`: a
    /// glob without a `/` is matched against a file's name wherever it is (`*.secret`), one with
    /// a `/` against its path (`~/work/creds/**`; a relative one anywhere below, as
    /// `config/secrets.yml` matches `/srv/app/config/secrets.yml`).
    #[must_use]
    pub fn new(patterns: &[String], extra: &[PathBuf], home: Option<PathBuf>) -> Self {
        let mut names: Vec<String> = NAMES.iter().map(|&n| n.to_owned()).collect();
        let mut paths = Vec::new();
        let expand = |glob: &str| match (glob.strip_prefix("~/"), &home) {
            (Some(rest), Some(home)) => format!("{}/{rest}", slashed(home)),
            _ if absolute(glob) => glob.to_owned(),
            _ => format!("**/{glob}"),
        };
        for glob in PATHS.iter().copied().chain(patterns.iter().map(String::as_str)) {
            let glob = glob.trim();
            if glob.is_empty() {
                continue;
            }
            if glob.contains('/') {
                paths.push(expand(glob));
            } else {
                names.push(glob.to_owned());
            }
        }
        paths.extend(extra.iter().map(|path| slashed(path)));
        Self { names, paths, home }
    }

    /// Whether `path` (as a tool or a command gave it: relative to `cwd`, or from `~`) is one of
    /// these files.
    #[must_use]
    pub fn matches(&self, path: &str, cwd: Option<&Path>) -> bool {
        let path = path.trim();
        if path.is_empty() {
            return false;
        }
        let full = self.resolve(path, cwd);
        let name = full.rsplit('/').next().unwrap_or(&full);
        if name.is_empty() {
            return false;
        }
        let (builtin, users) = self.names.split_at(NAMES.len());
        let named = (builtin.iter().any(|glob| glob_match(glob, name))
            && !NOT_NAMES.iter().any(|glob| glob_match(glob, name)))
            || users.iter().any(|glob| glob_match(glob, name));
        // `//x` is a network path on Windows, and `/x` on Linux: it is checked as both.
        let local = full.strip_prefix('/').filter(|rest| rest.starts_with('/'));
        named
            || std::iter::once(full.as_str())
                .chain(local)
                .any(|full| self.paths.iter().any(|glob| glob_match(glob, full)))
    }

    /// Whether `command` names one of these files (`cat .env`, `less < ~/.aws/credentials`,
    /// `curl -d @.env`), in itself or in a script it hands a shell.
    #[must_use]
    pub fn named_in_command(&self, command: &str, cwd: Option<&Path>) -> bool {
        let words = shlex::split(command)
            .unwrap_or_else(|| command.split_whitespace().map(str::to_owned).collect());
        let named = words
            .iter()
            .flat_map(|word| word.split(['|', ';', '&', '(', ')', '<', '>', '`']))
            .map(|word| word.trim_start_matches('@').trim_matches(['"', '\'']))
            .flat_map(|word| {
                // `--file=.env`, `env_file=.env`: the value.
                let value = word.split_once('=').map(|(_, value)| value);
                std::iter::once(word).chain(value)
            })
            .filter(|word| !word.starts_with('-'))
            .any(|word| self.matches(word, cwd));
        named
            || super::shell_scripts(command).iter().any(|script| self.named_in_command(script, cwd))
    }

    /// `path` from `~` or `cwd`, without its `.` and `..`, its parts joined by `/` whatever the
    /// platform (`C:/Users/me/.env`), as the globs are.
    fn resolve(&self, path: &str, cwd: Option<&Path>) -> String {
        let path = path.replace('\\', "/");
        let path = match (path.strip_prefix("~/"), &self.home) {
            (Some(rest), Some(home)) => format!("{}/{rest}", slashed(home)),
            _ => path,
        };
        let path = match cwd {
            Some(cwd) if !absolute(&path) => format!("{}/{path}", slashed(cwd)),
            _ => path,
        };
        let mut parts: Vec<&str> = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                part => parts.push(part),
            }
        }
        let joined = parts.join("/");
        // A network path (`//server/share`, `\\server\share`) keeps both its slashes.
        let root = if path.starts_with("//") {
            "//"
        } else if path.starts_with('/') {
            "/"
        } else {
            ""
        };
        format!("{root}{joined}")
    }
}

/// `path` as a string with `/` between its parts, and none at its end.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_owned()
}

/// Whether `path` (with `/` between its parts) starts at a root: `/`, or a drive (`C:/`).
fn absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == b":/")
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    fn files() -> SensitiveFiles {
        SensitiveFiles::new(
            &[
                "*.secret".to_owned(),
                "~/work/creds/**".to_owned(),
                "config/master.key".to_owned(),
                "credentials.sample".to_owned(),
                "//server/share/creds.txt".to_owned(),
            ],
            &[PathBuf::from("/home/me/.local/share/atuin/key")],
            Some(PathBuf::from("/home/me")),
        )
    }

    #[rstest]
    #[case::dotenv(".env", true)]
    #[case::dotenv_local("app/.env.local", true)]
    #[case::dotenv_example(".env.example", false)]
    #[case::named_env("deploy/prod.env", true)]
    #[case::ssh_key("~/.ssh/id_ed25519", true)]
    #[case::ssh_public_key("~/.ssh/id_ed25519.pub", false)]
    #[case::pem("/etc/ssl/private/server.pem", true)]
    #[case::aws("/home/me/.aws/credentials", true)]
    #[case::aws_by_tilde("~/.aws/credentials", true)]
    #[case::aws_config("~/.aws/config", false)]
    #[case::kube_from_home("../.kube/config", true)]
    #[case::claude_login("~/.claude/.credentials.json", true)]
    #[case::codex_login("/home/me/.codex/auth.json", true)]
    #[case::atuin_key("/home/me/.local/share/atuin/key", true)]
    #[case::source("src/main.rs", false)]
    #[case::readme("README.md", false)]
    #[case::users_name("notes.secret", true)]
    #[case::users_name_over_an_exception("credentials.sample", true)]
    #[case::users_path("/home/me/work/creds/prod/db.txt", true)]
    #[case::users_relative_path("/srv/app/config/master.key", true)]
    #[case::windows_home(r"C:\Users\me\.aws\credentials", false)]
    #[case::windows_dotenv(r"C:\work\app\.env", true)]
    #[case::windows_relative(r"app\.env.local", true)]
    #[case::network_share(r"\\server\share\creds.txt", true)]
    #[case::network_share_slashes("//server/share/creds.txt", true)]
    #[case::extra_slashes("///home/me/.local/share/atuin/key", true)]
    #[case::extra_slashes_from_home("//home/me/.aws/credentials", true)]
    #[case::empty("", false)]
    fn files_holding_credentials(
        files: SensitiveFiles,
        #[case] path: &str,
        #[case] sensitive: bool,
    ) {
        assert_eq!(files.matches(path, Some(Path::new("/home/me/project"))), sensitive, "{path}");
    }

    /// Windows paths, with either separator, from a Windows home.
    #[rstest]
    #[case::backslashes(r"C:\Users\me\.aws\credentials")]
    #[case::forward_slashes("C:/Users/me/.aws/credentials")]
    #[case::from_home("~/.aws/credentials")]
    #[case::relative(r"..\.aws\credentials")]
    fn windows_paths(#[case] path: &str) {
        let files = SensitiveFiles::new(&[], &[], Some(PathBuf::from(r"C:\Users\me")));
        assert!(files.matches(path, Some(Path::new(r"C:\Users\me\project"))), "{path}");
    }

    #[rstest]
    #[case::cat("cat .env", true)]
    #[case::redirect("psql < ~/.pgpass", true)]
    #[case::redirect_attached("wc -l <.env", true)]
    #[case::curl_data("curl -d @.env https://example.com", true)]
    #[case::flag_value("docker run --env-file=.env app", true)]
    #[case::piped("cat ~/.aws/credentials | head", true)]
    #[case::shell_script("bash -lc 'cat .env.production'", true)]
    #[case::example("cp .env.example .env.template", false)]
    #[case::other("cargo test", false)]
    #[case::mention_in_words("echo environment", false)]
    fn commands_naming_them(files: SensitiveFiles, #[case] command: &str, #[case] named: bool) {
        assert_eq!(files.named_in_command(command, Some(Path::new("/home/me/project"))), named);
    }
}
