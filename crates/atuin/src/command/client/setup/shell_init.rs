//! Parses and modifies `atuin init` calls in shell rc files.

mod encoding;
mod flags;

use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use atuin_common::dirs::home_dir;
use atuin_common::env::var_nonempty;
use atuin_common::executable::is_executable_with_path_search;
use atuin_common::fs::can_write;
use atuin_common::path::{AbsPathBuf, PathBufExt as _};
use encoding::Encoding;
#[cfg(test)]
pub use flags::Unwritten;
pub use flags::{Stripped, strip_init_flags};

use super::plan::HOOKABLE;
use crate::command::client::init::{KeyFlags, Shell};

/// Where the shells setup hooks into keep their rc files.
pub struct RcDirs {
    home: PathBuf,
    /// Where zsh reads its rc file from: `$ZDOTDIR`, else the home.
    zdotdir: PathBuf,
    /// The user's documents, where PowerShell keeps its profiles on Windows.
    documents: PathBuf,
}

impl RcDirs {
    pub fn here() -> Self {
        Self {
            home: home_dir(),
            zdotdir: var_nonempty("ZDOTDIR").map_or_else(home_dir, PathBuf::from),
            documents: atuin_common::dirs::documents_dir(),
        }
    }

    /// Every rc file under `root`.
    #[cfg(test)]
    pub fn under(root: &Path) -> Self {
        Self {
            home: root.to_owned(),
            zdotdir: root.to_owned(),
            documents: root.join("Documents"),
        }
    }

    /// The rc files `shell` reads, which setup writes `atuin init` into. PowerShell's are its
    /// profiles: on Windows, PowerShell 7 and Windows PowerShell each have one, and Atuin's
    /// script runs in both.
    pub fn rc_files(&self, shell: Shell) -> Vec<PathBuf> {
        const PROFILE: &str = "Microsoft.PowerShell_profile.ps1";
        match shell {
            Shell::Zsh => vec![self.zdotdir.join(".zshrc")],
            Shell::Bash => vec![self.home.join(".bashrc")],
            Shell::Fish => vec![self.home.join(".config").joined("fish").joined("config.fish")],
            Shell::PowerShell if cfg!(windows) => ["PowerShell", "WindowsPowerShell"]
                .map(|dir| self.documents.join(dir).joined(PROFILE))
                .into(),
            Shell::PowerShell => {
                vec![self.home.join(".config").joined("powershell").joined(PROFILE)]
            }
            Shell::Nu | Shell::Xonsh => Vec::new(),
        }
    }
}

/// What in an rc file shows that it loads Atuin, for the shells setup hooks into.
const fn init_marker(shell: Shell) -> Option<&'static str> {
    match shell {
        Shell::Zsh => Some("atuin init zsh"),
        Shell::Bash => Some("atuin init bash"),
        Shell::Fish => Some("atuin init fish"),
        Shell::PowerShell => Some("atuin init powershell"),
        Shell::Nu | Shell::Xonsh => None,
    }
}

/// The whitespace between the arguments of an `atuin init` call.
const GAP: [char; 2] = [' ', '\t'];

/// The characters that end an `atuin init` call in `shell`.
///
/// Zsh, Bash, fish and PowerShell each end a word at `)`, `|`, `;`, `&`, `<` and `>`. Zsh and Bash
/// also end one at a backtick, which closes a command substitution. Fish reads a backtick as a
/// plain character, and PowerShell reads it as an escape.
const fn ends_call(shell: Shell) -> &'static [char] {
    match shell {
        Shell::Zsh | Shell::Bash => &[')', '|', ';', '&', '<', '>', '`'],
        Shell::Fish | Shell::PowerShell | Shell::Nu | Shell::Xonsh => {
            &[')', '|', ';', '&', '<', '>']
        }
    }
}

/// Whether a `#` that comes right after `before` starts a comment in `shell`. It does at the start
/// of a word, which occurs after whitespace or after a character that [ends a call](ends_call).
fn hash_starts_comment(before: &str, shell: Shell) -> bool {
    before.is_empty() || before.ends_with(GAP) || before.ends_with(ends_call(shell))
}

/// An `atuin init` call that [`init_call`] found in a line of an rc file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InitCall<'a> {
    /// A call that setup can edit. For example, the `atuin init bash` in
    /// `eval "$(atuin init bash)"`.
    Editable(&'a str),
    /// A call that setup can't be sure will run as written, so it won't be edited. For example,
    /// the `atuin init bash` in `source ~/init-atuin.sh  # runs atuin init bash`.
    ReadOnly(&'a str),
}

impl<'a> InitCall<'a> {
    /// The [init marker](init_marker) and its arguments.
    const fn text(self) -> &'a str {
        match self {
            Self::Editable(text) | Self::ReadOnly(text) => text,
        }
    }
}

/// The `atuin init` call in a line of `shell`'s rc file: the [init marker](init_marker) and its
/// arguments.
fn init_call(line: &str, shell: Shell) -> Option<InitCall<'_>> {
    let marker = init_marker(shell)?;
    if line.trim_start().starts_with('#') {
        return None;
    }
    let ends = ends_call(shell);
    let in_word = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_');
    let (start, rest) = line.match_indices(marker).find_map(|(start, _)| {
        let rest = &line[start + marker.len()..];
        let alone = !line[..start].ends_with(in_word) && !rest.starts_with(in_word);
        alone.then_some((start, rest))
    })?;
    let args = if rest.starts_with(GAP) {
        rest.char_indices()
            .find(|&(at, c)| {
                ends.contains(&c) || (c == '#' && hash_starts_comment(&rest[..at], shell))
            })
            .map_or(rest.len(), |(at, _)| at)
    } else {
        0
    };

    let text = &line[start..start + marker.len() + args];
    let before = &line[..start];
    let commented =
        before.char_indices().any(|(at, c)| c == '#' && hash_starts_comment(&before[..at], shell));
    Some(if commented {
        InitCall::ReadOnly(text)
    } else {
        InitCall::Editable(text)
    })
}

/// The programs that run `shell`, any of which on `PATH` means it's here.
const fn programs(shell: Shell) -> &'static [&'static str] {
    match shell {
        Shell::Zsh => &["zsh"],
        Shell::Bash => &["bash"],
        Shell::Fish => &["fish"],
        Shell::PowerShell => &["pwsh", "powershell"],
        Shell::Nu => &["nu"],
        Shell::Xonsh => &["xonsh"],
    }
}

/// The shells whose rc file loads Atuin, and the keybinding flags of those loading lines.
pub struct ShellInits {
    pub shells: Vec<Shell>,
    pub flags: KeyFlags,
    /// Whether [`strip_init_flags`] can take off every flag that [`Self::flags`] holds. It can't
    /// if an rc file that it would change is read-only, or if one of the flags is in a
    /// [read-only call](InitCall::ReadOnly).
    pub strippable: bool,
}

pub async fn shell_inits(dirs: &RcDirs) -> ShellInits {
    let mut inits = ShellInits {
        shells: Vec::new(),
        flags: KeyFlags::default(),
        strippable: true,
    };
    for shell in HOOKABLE {
        let mut loads = false;
        for rc in dirs.rc_files(shell) {
            let read = tokio::fs::read(&rc).await;
            let Some((_, text)) = read.ok().and_then(|bytes| Encoding::decode(&bytes).ok()) else {
                continue;
            };
            let mut to_strip = false;
            for call in text.lines().filter_map(|line| init_call(line, shell)) {
                let found = flags::from_init(call.text());
                inits.flags = inits.flags.or(found);
                match call {
                    InitCall::Editable(text) => to_strip |= flags::any_to_strip(text),
                    InitCall::ReadOnly(_) if found != KeyFlags::default() => {
                        inits.strippable = false;
                    }
                    InitCall::ReadOnly(_) => {}
                }
                loads = true;
            }
            if to_strip && !can_write(&rc) {
                inits.strippable = false;
            }
        }
        if loads {
            inits.shells.push(shell);
        }
    }
    inits
}

/// The shells here that setup can hook into: those on `PATH`, or with an rc file.
pub fn shells_here(dirs: &RcDirs) -> Vec<Shell> {
    let here = |shell: &Shell| {
        let rc = dirs.rc_files(*shell).iter().any(|rc| rc.exists());
        rc || programs(*shell).iter().any(is_executable_with_path_search)
    };
    HOOKABLE.into_iter().filter(here).collect()
}

/// Add `atuin init` to the rc files of each of `shells`. Returns the shells it was added to, less
/// those it was in already.
pub fn install_shell_init(dirs: &RcDirs, shells: &[Shell]) -> io::Result<Vec<&'static str>> {
    let mut hooked = Vec::new();
    for &shell in shells {
        let mut added = false;
        for rc in dirs.rc_files(shell) {
            let init = match shell {
                Shell::Zsh => "\neval \"$(atuin init zsh)\"\n",
                // A `.bashrc` made here would be the only one bash reads, so it needs the
                // installer's PATH.
                Shell::Bash if !rc.exists() && installer_env(&dirs.home).exists() => {
                    ". \"$HOME/.atuin/bin/env\"\neval \"$(atuin init bash)\"\n"
                }
                Shell::Bash => "\neval \"$(atuin init bash)\"\n",
                Shell::Fish => "\nif status is-interactive\n    atuin init fish | source\nend\n",
                Shell::PowerShell => "\natuin init powershell | Out-String | Invoke-Expression\n",
                Shell::Nu | Shell::Xonsh => continue,
            };
            // Fish and PowerShell keep theirs in a directory a fresh install hasn't made.
            if let Some(dir) = rc.parent() {
                std::fs::create_dir_all(dir)?;
            }
            added |= append_once(&rc, shell, init)?;
        }
        if added {
            hooked.push(shell.name());
        }
    }
    Ok(hooked)
}

/// The script the installer writes to put Atuin on `PATH`.
fn installer_env(home: &Path) -> PathBuf {
    home.join(".atuin").joined("bin").joined("env")
}

/// The shells whose rc files `remove_shell_init` took `atuin init` out of, and the lines loading
/// Atuin it left, each with its rc file, which setup didn't write.
pub struct Unhooked {
    pub shells: Vec<&'static str>,
    pub left: Vec<(PathBuf, String)>,
}

/// Take the `atuin init` that [`install_shell_init`] writes out of the rc files of each of
/// `shells`.
pub fn remove_shell_init(dirs: &RcDirs, shells: &[Shell]) -> io::Result<Unhooked> {
    let mut unhooked = Unhooked {
        shells: Vec::new(),
        left: Vec::new(),
    };
    for &shell in shells {
        let mut removed = false;
        for rc in dirs.rc_files(shell) {
            // An rc file linked from a dotfiles repo is saved there, keeping the link.
            let target = AbsPathBuf::canonicalize(&rc).map_or_else(|_| rc.clone(), PathBuf::from);
            let read = match std::fs::read(&target) {
                Ok(read) => read,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let (encoding, text) = Encoding::decode(&read)?;
            let without = without_init(&text, shell);
            if let Some(without) = &without {
                let bytes = encoding.encode(without, true);
                atuin_common::fs::replace(&target, &bytes, |now| now == read)
                    .map_err(io::Error::other)?;
                removed = true;
            }
            let after = without.as_deref().unwrap_or(&text);
            let left = after.lines().filter(|line| init_call(line, shell).is_some());
            unhooked.left.extend(left.map(|line| (rc.clone(), line.trim().to_owned())));
        }
        if removed {
            unhooked.shells.push(shell.name());
        }
    }
    Ok(unhooked)
}

/// `rc` without the `atuin init` that [`install_shell_init`] writes for `shell`, and the blank
/// line it writes before it. `None` when it has none. Anything else loading Atuin stays, as
/// setup can't tell what a change to it would break: the same line indented, or inside a block
/// the user opened, as `if command -v atuin; then` does.
fn without_init(rc: &str, shell: Shell) -> Option<String> {
    let written: &[&str] = match shell {
        Shell::Zsh => &["eval \"$(atuin init zsh)\""],
        Shell::Bash => &["eval \"$(atuin init bash)\""],
        Shell::Fish => &["if status is-interactive", "    atuin init fish | source", "end"],
        Shell::PowerShell => &["atuin init powershell | Out-String | Invoke-Expression"],
        Shell::Nu | Shell::Xonsh => return None,
    };
    let lines: Vec<&str> = rc.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut at = 0;
    let mut removed = false;
    while at < lines.len() {
        let block = lines.get(at..at + written.len());
        let is_written = block.is_some_and(|block| {
            block.iter().zip(written).all(|(line, written)| line.trim_end() == *written)
        });
        let before = kept.iter().rev().find(|line| !line.trim().is_empty());
        let after = lines.iter().skip(at + written.len()).find(|line| !line.trim().is_empty());
        let nested = before.is_some_and(|line| opens_block(line))
            || after.is_some_and(|line| closes_block(line));
        if is_written && !nested {
            if kept.last().is_some_and(|line| line.trim().is_empty()) {
                kept.pop();
            }
            at += written.len();
            removed = true;
            continue;
        }
        kept.push(lines[at]);
        at += 1;
    }
    if !removed {
        return None;
    }
    // As the file ends its lines, which on Windows is often `\r\n`.
    let newline = newline_of(rc);
    let mut without = kept.join(newline);
    if rc.ends_with('\n') && !without.is_empty() {
        without.push_str(newline);
    }
    Some(without)
}

/// Whether `line` opens a block the lines after it are in, as `then` and `{` do.
fn opens_block(line: &str) -> bool {
    let last = line.trim_end().rsplit([' ', '\t', ';']).next();
    last.is_some_and(|word| matches!(word, "then" | "do" | "else" | "{"))
}

/// Whether `line` closes a block, as `fi` and `}` do.
fn closes_block(line: &str) -> bool {
    let first = line.trim_start().split([' ', '\t', ';']).next();
    first.is_some_and(|word| matches!(word, "fi" | "done" | "esac" | "}" | "end"))
}

/// How `text` ends its lines: `\r\n` if it does anywhere, else `\n`.
fn newline_of(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Append `text` to the file at `path` unless it holds an `atuin init` call for `shell`, with the
/// file's encoding and line endings. Returns whether it appended.
fn append_once(path: &Path, shell: Shell, text: &str) -> io::Result<bool> {
    let (encoding, newline) = match std::fs::read(path) {
        Ok(read) => match Encoding::decode(&read)? {
            (_, existing) if existing.lines().any(|line| init_call(line, shell).is_some()) => {
                return Ok(false);
            }
            (encoding, existing) => (encoding, newline_of(&existing)),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => (Encoding::Utf8, "\n"),
        Err(e) => return Err(e),
    };
    let text = encoding.encode(&text.replace('\n', newline), false);
    OpenOptions::new().create(true).append(true).open(path)?.write_all(&text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    #[rstest]
    #[case::zsh(Shell::Zsh, "export A=1\n\neval \"$(atuin init zsh)\"\n", Some("export A=1\n"))]
    #[case::bash_keeps_the_path(
        Shell::Bash,
        ". \"$HOME/.atuin/bin/env\"\neval \"$(atuin init bash)\"\n",
        Some(". \"$HOME/.atuin/bin/env\"\n")
    )]
    #[case::fish(
        Shell::Fish,
        "set -x A 1\n\nif status is-interactive\n    atuin init fish | source\nend\n",
        Some("set -x A 1\n")
    )]
    #[case::alone(Shell::Zsh, "\neval \"$(atuin init zsh)\"\n", Some(""))]
    #[case::powershell(
        Shell::PowerShell,
        "Set-Alias g git\r\n\r\natuin init powershell | Out-String | Invoke-Expression\r\n",
        Some("Set-Alias g git\r\n")
    )]
    #[case::its_own_flags(Shell::Zsh, "eval \"$(atuin init zsh --disable-up-arrow)\"\n", None)]
    #[case::guarded(
        Shell::Bash,
        "if command -v atuin >/dev/null; then\n  eval \"$(atuin init bash)\"\nfi\n",
        None
    )]
    #[case::guarded_unindented(
        Shell::Bash,
        "if command -v atuin >/dev/null; then\neval \"$(atuin init bash)\"\nfi\n",
        None
    )]
    #[case::in_a_function(Shell::Zsh, "atuin_on() {\neval \"$(atuin init zsh)\"\n}\n", None)]
    #[case::fish_in_a_function(
        Shell::Fish,
        "function atuin_on\nif status is-interactive\n    atuin init fish | source\nend\nend\n",
        None
    )]
    #[case::after_a_block(
        Shell::Zsh,
        "if true; then\n  export A=1\nfi\n\neval \"$(atuin init zsh)\"\n",
        Some("if true; then\n  export A=1\nfi\n")
    )]
    #[case::none(Shell::Zsh, "export A=1\n", None)]
    fn the_init_setup_wrote_comes_out_and_nothing_else(
        #[case] shell: Shell,
        #[case] rc: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(without_init(rc, shell).as_deref(), expected);
    }

    #[rstest]
    #[case::code("eval \"$(atuin init zsh)\"", Some("atuin init zsh"))]
    #[case::commented_out("# eval \"$(atuin init zsh)\"", None)]
    #[case::indented_comment(" \t#eval \"$(atuin init zsh)\"", None)]
    #[case::trailing_comment("export A=1 # atuin init zsh", Some("atuin init zsh"))]
    #[case::elsewhere("export A=1", None)]
    #[case::with_flags(
        "eval \"$(atuin init zsh --disable-up-arrow)\"",
        Some("atuin init zsh --disable-up-arrow")
    )]
    #[case::piped(
        "atuin init zsh --disable-ctrl-r | source",
        Some("atuin init zsh --disable-ctrl-r ")
    )]
    #[case::before_a_semicolon(
        "atuin init zsh --disable-ctrl-r; true",
        Some("atuin init zsh --disable-ctrl-r")
    )]
    #[case::before_a_comment(
        "atuin init zsh --disable-ctrl-r # --disable-up-arrow",
        Some("atuin init zsh --disable-ctrl-r ")
    )]
    #[case::flag_in_a_comment(
        "eval \"$(atuin init zsh)\"  # was --disable-up-arrow",
        Some("atuin init zsh")
    )]
    #[case::joined_to_a_hash("atuin init zsh --x#y z", Some("atuin init zsh --x#y z"))]
    #[case::shell_joined_to_a_quote("atuin init zsh\"x --disable-ctrl-r\"", Some("atuin init zsh"))]
    #[case::a_longer_word("atuin init zshx --disable-ctrl-r", None)]
    #[case::after_a_word("myatuin init zsh --disable-ctrl-r", None)]
    #[case::after_a_dash("x-atuin init zsh", None)]
    #[case::after_a_path("eval \"$(~/bin/atuin init zsh)\"", Some("atuin init zsh"))]
    #[case::in_backticks(
        "eval \"`atuin init zsh --disable-up-arrow`\"",
        Some("atuin init zsh --disable-up-arrow")
    )]
    #[case::after_a_longer_word(
        "myatuin init zsh; atuin init zsh --disable-ctrl-r",
        Some("atuin init zsh --disable-ctrl-r")
    )]
    fn init_call_ends_where_the_shell_ends_it(#[case] line: &str, #[case] expected: Option<&str>) {
        assert_eq!(init_call(line, Shell::Zsh).map(InitCall::text), expected);
    }

    #[rstest]
    #[case::code("atuin init zsh --disable-ctrl-r", false)]
    #[case::trailing_comment("export A=1 # atuin init zsh --disable-ctrl-r", true)]
    #[case::trailing_comment_after_a_tab("export A=1\t#atuin init zsh", true)]
    #[case::trailing_comment_after_a_semicolon("foo;# atuin init zsh --disable-up-arrow", true)]
    #[case::trailing_comment_after_a_pipe("foo |# atuin init zsh", true)]
    #[case::trailing_comment_after_a_parenthesis("(foo)# atuin init zsh", true)]
    #[case::hash_in_a_word("echo a#b; atuin init zsh", false)]
    #[case::comment_after_the_call("atuin init zsh # --disable-ctrl-r", false)]
    fn a_call_in_a_comment_is_read_only(#[case] line: &str, #[case] read_only: bool) {
        let call = init_call(line, Shell::Zsh).unwrap();
        assert_eq!(matches!(call, InitCall::ReadOnly(_)), read_only);
    }

    /// An init that's commented out reads as no init: setup adds its own, and leaves the comment
    /// when it takes its own out, without reporting the comment as loading Atuin.
    #[rstest]
    #[tokio::test]
    async fn commented_out_inits_are_not_inits() {
        let home = TempDir::new().unwrap();
        let bashrc = home.path().join(".bashrc");
        let commented = "# eval \"$(atuin init bash)\"\n";
        std::fs::write(&bashrc, commented).unwrap();
        let dirs = RcDirs::under(home.path());
        assert_eq!(shell_inits(&dirs).await.shells, []);

        assert_eq!(install_shell_init(&dirs, &[Shell::Bash]).unwrap(), ["bash"]);
        assert_eq!(shell_inits(&dirs).await.shells, [Shell::Bash]);
        let unhooked = remove_shell_init(&dirs, &[Shell::Bash]).unwrap();
        assert_eq!(unhooked.shells, ["bash"]);
        assert_eq!(unhooked.left, []);
        assert_eq!(std::fs::read_to_string(&bashrc).unwrap(), commented);
    }

    /// A profile written with Windows line endings gets the init with them too.
    #[rstest]
    fn shell_init_keeps_the_files_line_endings() {
        let home = TempDir::new().unwrap();
        let dirs = RcDirs::under(home.path());
        let profile = dirs.rc_files(Shell::PowerShell).remove(0);
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        std::fs::write(&profile, "Set-Alias g git\r\n").unwrap();

        install_shell_init(&dirs, &[Shell::PowerShell]).unwrap();
        let hooked = std::fs::read_to_string(&profile).unwrap();
        let init = "atuin init powershell | Out-String | Invoke-Expression";
        assert_eq!(hooked, format!("Set-Alias g git\r\n\r\n{init}\r\n"));
        remove_shell_init(&dirs, &[Shell::PowerShell]).unwrap();
        assert_eq!(std::fs::read_to_string(&profile).unwrap(), "Set-Alias g git\r\n");
    }

    /// A profile Windows PowerShell 5.1 wrote as UTF-16 gets the init in UTF-16, and setup finds it
    /// there and takes it out again.
    #[rstest]
    #[tokio::test]
    async fn shell_init_keeps_the_files_encoding(
        #[values(Encoding::Utf16Le, Encoding::Utf16Be)] encoding: Encoding,
    ) {
        let home = TempDir::new().unwrap();
        let dirs = RcDirs::under(home.path());
        let profile = dirs.rc_files(Shell::PowerShell).remove(0);
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        let mine = encoding.encode("Set-Alias g git\r\n", true);
        std::fs::write(&profile, &mine).unwrap();

        install_shell_init(&dirs, &[Shell::PowerShell]).unwrap();
        let init = "atuin init powershell | Out-String | Invoke-Expression";
        let hooked = format!("Set-Alias g git\r\n\r\n{init}\r\n");
        let read = std::fs::read(&profile).unwrap();
        assert_eq!(Encoding::decode(&read).unwrap(), (encoding, hooked));
        assert_eq!(shell_inits(&dirs).await.shells, [Shell::PowerShell]);
        remove_shell_init(&dirs, &[Shell::PowerShell]).unwrap();
        assert_eq!(std::fs::read(&profile).unwrap(), mine);
    }

    /// An rc file linked from a dotfiles repo loses the init there, and stays a link.
    #[cfg(unix)]
    #[rstest]
    fn shell_init_comes_out_where_a_link_leads() {
        let home = TempDir::new().unwrap();
        let dotfiles = home.path().join("dotfiles");
        std::fs::create_dir(&dotfiles).unwrap();
        let tracked = dotfiles.join("zshrc");
        std::fs::write(&tracked, "export A=1\n").unwrap();
        let zshrc = home.path().join(".zshrc");
        std::os::unix::fs::symlink(&tracked, &zshrc).unwrap();
        let dirs = RcDirs::under(home.path());
        install_shell_init(&dirs, &[Shell::Zsh]).unwrap();

        remove_shell_init(&dirs, &[Shell::Zsh]).unwrap();
        assert!(zshrc.symlink_metadata().unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&tracked).unwrap(), "export A=1\n");
    }

    /// Taken out, the init setup wrote leaves the rc file as it found it, and says what else loads
    /// Atuin that it left.
    #[rstest]
    fn shell_init_comes_out_as_it_went_in() {
        let home = TempDir::new().unwrap();
        let zshrc = home.path().join(".zshrc");
        std::fs::write(&zshrc, "export A=1\n").unwrap();
        let bashrc = home.path().join(".bashrc");
        let custom = "eval \"$(atuin init bash --disable-up-arrow)\"";
        std::fs::write(&bashrc, format!("{custom}\n")).unwrap();
        let dirs = RcDirs::under(home.path());
        install_shell_init(&dirs, &[Shell::Zsh]).unwrap();

        let shells = [Shell::Zsh, Shell::Bash];
        let unhooked = remove_shell_init(&dirs, &shells).unwrap();
        assert_eq!(unhooked.shells, ["zsh"]);
        assert_eq!(unhooked.left, [(bashrc, custom.to_owned())]);
        assert_eq!(std::fs::read_to_string(&zshrc).unwrap(), "export A=1\n");
    }

    const FLAGGED_ZSH: &str = "eval \"$(atuin init zsh --disable-up-arrow --bind-ai-resume)\"";

    /// A comment can describe what the code before it runs, so a flag in the comment still counts.
    #[rstest]
    #[tokio::test]
    async fn a_flag_in_a_comment_counts_but_stays() {
        let home = TempDir::new().unwrap();
        let line = "source ~/init-atuin.sh  # same as atuin init zsh --disable-up-arrow";
        std::fs::write(home.path().join(".zshrc"), format!("{line}\n")).unwrap();
        let inits = shell_inits(&RcDirs::under(home.path())).await;
        assert_eq!(inits.shells, [Shell::Zsh]);
        assert!(inits.flags.disable_up_arrow);
        assert!(!inits.strippable);
    }

    #[rstest]
    #[tokio::test]
    async fn init_flags_start_the_keys() {
        let home = TempDir::new().unwrap();
        std::fs::write(home.path().join(".zshrc"), format!("{FLAGGED_ZSH}\n")).unwrap();
        let inits = shell_inits(&RcDirs::under(home.path())).await;
        assert_eq!(inits.shells, [Shell::Zsh]);
        assert_eq!(inits.flags, KeyFlags {
            disable_ctrl_r: false,
            disable_up_arrow: true,
            bind_ai_resume: true,
        });
    }

    /// Setup can take the flags off only if it can write every rc file that has flags to take off.
    #[cfg(unix)]
    #[rstest]
    #[case::flags(FLAGGED_ZSH, false, true)]
    #[case::flags_in_a_read_only_file(FLAGGED_ZSH, true, false)]
    #[case::only_the_old_ai_flag_in_a_read_only_file("atuin init zsh --disable-ai", true, false)]
    #[case::no_flags_in_a_read_only_file("eval \"$(atuin init zsh)\"", true, true)]
    #[case::flags_in_a_comment("export A=1  # atuin init zsh --disable-up-arrow", false, false)]
    #[case::old_ai_flag_in_a_comment("export A=1  # atuin init zsh --disable-ai", false, true)]
    #[tokio::test]
    async fn flags_are_strippable_where_their_files_can_be_written(
        #[case] line: &str,
        #[case] read_only: bool,
        #[case] strippable: bool,
    ) {
        use std::os::unix::fs::PermissionsExt as _;

        let home = TempDir::new().unwrap();
        let zshrc = home.path().join(".zshrc");
        std::fs::write(&zshrc, format!("{line}\n")).unwrap();
        if read_only {
            std::fs::set_permissions(&zshrc, std::fs::Permissions::from_mode(0o444)).unwrap();
            if can_write(&zshrc) {
                // Root writes to read-only files, so the case doesn't arise.
                return;
            }
        }
        let inits = shell_inits(&RcDirs::under(home.path())).await;
        assert_eq!(inits.strippable, strippable);
    }

    #[rstest]
    #[tokio::test]
    async fn the_flags_of_every_shell_combine() {
        let home = TempDir::new().unwrap();
        let dirs = RcDirs::under(home.path());
        let zshrc = dirs.rc_files(Shell::Zsh).remove(0);
        std::fs::write(&zshrc, "eval \"$(atuin init zsh --disable-up-arrow)\"\n").unwrap();
        let bashrc = dirs.rc_files(Shell::Bash).remove(0);
        std::fs::write(&bashrc, "eval \"$(atuin init bash --disable-ctrl-r)\"\n").unwrap();
        let inits = shell_inits(&dirs).await;
        assert_eq!(inits.shells, [Shell::Zsh, Shell::Bash]);
        assert_eq!(inits.flags, KeyFlags {
            disable_ctrl_r: true,
            disable_up_arrow: true,
            bind_ai_resume: false,
        });
    }

    /// A line that had only keybinding flags is the line that setup writes once they're off, so
    /// unhooking takes it out.
    #[rstest]
    fn a_stripped_line_comes_out_with_the_hook() {
        let home = TempDir::new().unwrap();
        let dirs = RcDirs::under(home.path());
        let zshrc = dirs.rc_files(Shell::Zsh).remove(0);
        std::fs::write(&zshrc, format!("export A=1\n{FLAGGED_ZSH}\n")).unwrap();

        strip_init_flags(&dirs);
        let unhooked = remove_shell_init(&dirs, &[Shell::Zsh]).unwrap();
        assert_eq!(unhooked.shells, ["zsh"]);
        assert!(unhooked.left.is_empty());
        assert_eq!(std::fs::read_to_string(&zshrc).unwrap(), "export A=1\n");
    }

    #[rstest]
    fn shell_init_is_added_once() {
        let home = TempDir::new().unwrap();
        std::fs::create_dir_all(home.path().join(".atuin/bin")).unwrap();
        std::fs::write(home.path().join(".atuin/bin/env"), "").unwrap();

        let shells = [Shell::Zsh, Shell::Bash, Shell::Fish, Shell::PowerShell];
        let dirs = RcDirs::under(home.path());
        let first = install_shell_init(&dirs, &shells).unwrap();
        let again = install_shell_init(&dirs, &shells).unwrap();
        let all = vec!["zsh", "bash", "fish", "powershell"];
        assert_eq!((first, again), (all, Vec::<&str>::new()));

        let bashrc = std::fs::read_to_string(home.path().join(".bashrc")).unwrap();
        assert_eq!(bashrc, ". \"$HOME/.atuin/bin/env\"\neval \"$(atuin init bash)\"\n");
        let zshrc = std::fs::read_to_string(home.path().join(".zshrc")).unwrap();
        assert_eq!(zshrc.matches("atuin init zsh").count(), 1);
        // Into a config directory fish hadn't made yet.
        let fish = std::fs::read_to_string(home.path().join(".config/fish/config.fish")).unwrap();
        assert_eq!(fish.matches("atuin init fish").count(), 1);
        let profiles = dirs.rc_files(Shell::PowerShell);
        let profile = std::fs::read_to_string(&profiles[0]).unwrap();
        assert_eq!(profile, "\natuin init powershell | Out-String | Invoke-Expression\n");
    }

    #[rstest]
    fn shell_init_goes_only_where_it_is_asked_for() {
        let home = TempDir::new().unwrap();
        install_shell_init(&RcDirs::under(home.path()), &[Shell::Fish]).unwrap();
        assert!(!home.path().join(".zshrc").exists() && !home.path().join(".bashrc").exists());
    }
}
