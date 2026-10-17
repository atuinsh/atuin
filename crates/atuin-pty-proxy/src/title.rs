//! Names the program in the foreground in the title of the terminal the proxy runs in.
//!
//! A terminal that names the program running in it, in a tab or a window title, finds the proxy in
//! the foreground of its pty and shows `atuin` whatever the user runs. The proxy does what tmux's
//! `automatic-rename` does for a window name: it looks at the foreground process group of the pty
//! its child runs on, at most every [`MIN_CHECK_INTERVAL`] while there is output and every
//! [`IDLE_CHECK_INTERVAL`] without, and titles the terminal (`OSC 0`) with the name of the group's
//! leader.
//!
//! A title the child sets itself, with `OSC 0`, `1` or `2`, wins over the proxy's. tmux keeps a
//! window's name apart from its pane's title and never has to choose; the proxy does:
//!
//! * One the shell sets stands until a program sets another. A shell that titles the terminal
//!   keeps doing so around the commands it runs.
//! * One a program sets stands while its process group stays in the foreground, and the proxy
//!   takes over again once it has gone, rather than leave a finished program's title behind.
//!
//! The child's output and the proxy's titles reach the terminal through one [`TerminalOutput`],
//! which writes them under one lock and keeps titles out of the middle of escape sequences.

use std::ffi::OsStr;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::time::Duration;

use atuin_common::os::unix::io::WriteAllExt;
use atuin_common::os::unix::process;
use parking_lot::Mutex;
use portable_pty::MasterPty;
use rustix::io::Errno;
use rustix::process::Pid;

use crate::cwd_updater;
use crate::escape_tracker::EscapeTracker;

/// The least time between two looks at the foreground.
///
/// tmux waits as long, which keeps looking cheap while output streams and keeps most commands too
/// short to matter out of the title.
const MIN_CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// The longest the updater waits for output before it looks at the foreground anyway, so a program
/// that prints nothing is still named.
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// The pty the child runs on, as seen from its master side.
pub struct Foreground {
    pty: Option<OwnedFd>,
    /// The proxy's child, which leads the shell's process group.
    shell: Option<Pid>,
}

impl Foreground {
    pub fn new(master: &dyn MasterPty, shell: Option<Pid>) -> Self {
        Self {
            pty: cwd_updater::pty_parent_fd(master),
            shell,
        }
    }

    /// The process group in the foreground, if any.
    fn group(&self) -> Option<Pid> {
        self.pty.as_ref().and_then(|pty| cwd_updater::tcgetpgrp(pty.as_fd()))
    }

    /// The title naming `group`'s leader, if it can be read.
    fn title(group: Pid) -> Option<String> {
        Self::printable(&process::name(group)?)
    }

    /// `name` without its control characters, if anything is left.
    ///
    /// A process can call itself anything, and a control character would end the title early and
    /// leave the rest of the name for the terminal to run.
    fn printable(name: &OsStr) -> Option<String> {
        let name: String = name.to_string_lossy().chars().filter(|c| !c.is_control()).collect();
        (!name.is_empty()).then_some(name)
    }
}

/// Who set the title the terminal shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// The proxy, or nobody yet.
    Proxy,
    /// The shell, until a program sets a title.
    Shell,
    /// The program leading this process group, while the group is in the foreground.
    Program(Pid),
}

impl Owner {
    /// The owner of a title set while `group` was in the foreground of `shell`'s pty.
    fn of_title_set_with(group: Option<Pid>, shell: Option<Pid>) -> Self {
        match group {
            Some(group) if Some(group) != shell => Self::Program(group),
            // With nothing in the foreground to own it, the title is safest with the shell, which
            // keeps it the longest.
            _ => Self::Shell,
        }
    }

    /// The owner once `group` is in the foreground.
    fn with_foreground(self, group: Pid) -> Self {
        match self {
            Self::Program(program) if program != group => Self::Proxy,
            Self::Proxy | Self::Shell | Self::Program(_) => self,
        }
    }
}

/// What the proxy knows of the terminal's title.
struct TitleState {
    /// The escape sequences in everything written to the terminal so far.
    escapes: EscapeTracker,
    owner: Owner,
    /// The title the proxy set, while the terminal still shows it.
    shown: Option<String>,
}

/// The proxy's output to the terminal it runs in: the child's output, and titles of its own.
pub struct TerminalOutput<T> {
    terminal: T,
    foreground: Foreground,
    /// Held while writing to the terminal, so a title lands between two writes of the child's
    /// output and never inside one.
    state: Mutex<TitleState>,
}

impl<T: AsFd> TerminalOutput<T> {
    pub fn new(terminal: T, foreground: Foreground) -> Self {
        Self {
            terminal,
            foreground,
            state: Mutex::new(TitleState {
                escapes: EscapeTracker::default(),
                owner: Owner::Proxy,
                shown: None,
            }),
        }
    }

    /// Write the child's `data` to the terminal.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the lock is held through the write, so no title lands inside it"
    )]
    pub fn forward(&self, data: &[u8]) -> Result<(), Errno> {
        let mut state = self.state.lock();
        if state.escapes.push(data) {
            // The program that set the title has most likely not left the foreground yet.
            state.owner = Owner::of_title_set_with(self.foreground.group(), self.foreground.shell);
            state.shown = None;
        }
        self.terminal.write_all_retrying(data)
    }

    /// Title the terminal with the name of the program in the foreground, unless the child's own
    /// title stands.
    pub fn update_title(&self) {
        let Some(group) = self.foreground.group() else {
            return;
        };
        // Reading the name takes the longest, so it happens before the lock the child's output
        // waits on.
        let Some(name) = Foreground::title(group) else {
            return;
        };

        let mut state = self.state.lock();
        // A program may have taken the foreground, and set a title that must stand, while its
        // predecessor's name was read. The next look names it.
        if self.foreground.group() != Some(group) || !state.escapes.at_ground() {
            return;
        }
        state.owner = state.owner.with_foreground(group);
        if state.owner != Owner::Proxy || state.shown.as_ref() == Some(&name) {
            return;
        }
        let sequence = format!("\x1b]0;{name}\x07");
        if self.terminal.write_all_retrying(sequence.as_bytes()).is_ok() {
            state.shown = Some(name);
        }
    }
}

/// Keeps the terminal's title on the program in the foreground, from a thread of its own.
pub struct TitleUpdater {
    /// Used to tell the update thread to look at the foreground now.
    tx: SyncSender<()>,
}

impl TitleUpdater {
    /// Start titling `output`'s terminal, until the updater is dropped.
    pub fn new<T: AsFd + Send + Sync + 'static>(output: Arc<TerminalOutput<T>>) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);

        std::thread::spawn(move || {
            loop {
                match rx.recv_timeout(IDLE_CHECK_INTERVAL) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                output.update_title();
                std::thread::sleep(MIN_CHECK_INTERVAL);
            }
        });

        Self { tx }
    }

    /// Tell the updater the child wrote output, which it does when something new takes the
    /// foreground.
    pub fn update(&self) {
        // A full channel isn't an error; it means the updater has already been signaled to run.
        let _ = self.tx.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::time::Instant;

    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use rstest::{fixture, rstest};
    use tempfile::NamedTempFile;

    use super::*;

    const SHELL: i32 = 100;
    const VIM: i32 = 200;
    const LESS: i32 = 300;

    fn pid(raw: i32) -> Pid {
        Pid::from_raw(raw).expect("a test pid is positive")
    }

    #[rstest]
    #[case::by_the_shell(Some(SHELL), Some(SHELL), Owner::Shell)]
    #[case::by_a_program(Some(VIM), Some(SHELL), Owner::Program(pid(VIM)))]
    #[case::by_a_program_of_an_unknown_shell(Some(VIM), None, Owner::Program(pid(VIM)))]
    #[case::with_nothing_in_the_foreground(None, Some(SHELL), Owner::Shell)]
    fn a_title_belongs_to_whoever_was_in_the_foreground(
        #[case] group: Option<i32>,
        #[case] shell: Option<i32>,
        #[case] owner: Owner,
    ) {
        assert_eq!(Owner::of_title_set_with(group.map(pid), shell.map(pid)), owner);
    }

    #[rstest]
    #[case::program_still_in_the_foreground(
        Owner::Program(pid(VIM)),
        VIM,
        Owner::Program(pid(VIM))
    )]
    #[case::program_gone_back_to_the_shell(Owner::Program(pid(VIM)), SHELL, Owner::Proxy)]
    #[case::program_gone_to_another(Owner::Program(pid(VIM)), LESS, Owner::Proxy)]
    #[case::shell_while_a_program_runs(Owner::Shell, VIM, Owner::Shell)]
    #[case::proxy(Owner::Proxy, VIM, Owner::Proxy)]
    fn only_a_programs_title_lapses_when_it_leaves_the_foreground(
        #[case] owner: Owner,
        #[case] group: i32,
        #[case] expected: Owner,
    ) {
        assert_eq!(owner.with_foreground(pid(group)), expected);
    }

    #[rstest]
    #[case::plain("vim", Some("vim"))]
    #[case::escape("vim\x1b]0;pwned\x07", Some("vim]0;pwned"))]
    #[case::newline("vim\n", Some("vim"))]
    #[case::c1_terminator("vim\u{9c}", Some("vim"))]
    #[case::unicode("ñvim", Some("ñvim"))]
    #[case::only_controls("\x1b\x07", None)]
    #[case::empty("", None)]
    fn titles_carry_no_control_characters(#[case] name: &str, #[case] expected: Option<&str>) {
        assert_eq!(Foreground::printable(OsStr::new(name)).as_deref(), expected);
    }

    /// `/bin/sh` on a pty of its own, with a [`TerminalOutput`] writing to a file.
    struct Shell {
        output: TerminalOutput<File>,
        /// Where `output` writes.
        written: NamedTempFile,
        pid: Pid,
        /// The terminal, for typing at the shell.
        input: File,
        child: Box<dyn portable_pty::Child + Send + Sync>,
    }

    impl Shell {
        /// Wait for the foreground to be `shell`'s, or not, then return it.
        fn foreground_settles(&self, shells: bool) -> Pid {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(group) = self.output.foreground.group()
                    && (group == self.pid) == shells
                {
                    return group;
                }
                assert!(Instant::now() < deadline, "the foreground never changed hands");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Run `sleep` in the foreground and wait for it to get there.
        fn sleep(&mut self) {
            writeln!(self.input, "sleep 30").expect("write to pty");
            let group = self.foreground_settles(false);
            // The shell hands the foreground to its child before the child execs `sleep`.
            let deadline = Instant::now() + Duration::from_secs(10);
            while process::name(group).as_deref() != Some(OsStr::new("sleep")) {
                assert!(Instant::now() < deadline, "the shell's child never became sleep");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Interrupt the program in the foreground and wait for the shell to take it back.
        fn interrupt(&mut self) {
            self.input.write_all(&[0x03]).expect("write to pty");
            self.foreground_settles(true);
        }

        /// Everything written to the terminal so far.
        fn written(&self) -> Vec<u8> {
            std::fs::read(self.written.path()).expect("read the terminal's output")
        }

        fn titled(name: &str) -> Vec<u8> {
            format!("\x1b]0;{name}\x07").into_bytes()
        }

        fn shell_title(&self) -> Vec<u8> {
            let name = process::name(self.pid).expect("the shell has a name");
            Self::titled(&name.to_string_lossy())
        }
    }

    impl Drop for Shell {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[fixture]
    fn shell() -> Shell {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty");
        let child = pair.slave.spawn_command(CommandBuilder::new("/bin/sh")).expect("spawn shell");
        drop(pair.slave);

        // A shell whose output has nowhere to go stalls once the pty fills.
        let mut drain = pair.master.try_clone_reader().expect("clone pty reader");
        let (printed, prompt) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while drain.read(&mut buf).is_ok_and(|read| read > 0) {
                let _ = printed.send(());
            }
        });
        // Until then, the shell may not have exec'd, and is named after the test.
        prompt.recv_timeout(Duration::from_secs(10)).expect("the shell never printed a prompt");

        let written = NamedTempFile::new().expect("create the terminal's file");
        let pid = Pid::from_raw(child.process_id().expect("shell pid").cast_signed()).unwrap();
        let shell = Shell {
            output: TerminalOutput::new(
                written.reopen().expect("open the terminal's file"),
                Foreground::new(pair.master.as_ref(), Some(pid)),
            ),
            written,
            pid,
            input: File::from(cwd_updater::pty_parent_fd(pair.master.as_ref()).unwrap()),
            child,
        };
        shell.foreground_settles(true);
        shell
    }

    #[rstest]
    fn names_the_program_in_the_foreground(mut shell: Shell) {
        shell.output.update_title();
        assert_eq!(shell.written(), shell.shell_title());

        shell.sleep();
        shell.output.update_title();
        assert_eq!(shell.written(), [shell.shell_title(), Shell::titled("sleep")].concat());
    }

    #[rstest]
    fn does_not_repeat_a_title_the_terminal_shows(shell: Shell) {
        shell.output.update_title();
        shell.output.update_title();

        assert_eq!(shell.written(), shell.shell_title());
    }

    #[rstest]
    fn a_programs_title_stands_until_it_leaves_the_foreground(mut shell: Shell) {
        // What vim's `set title` sends.
        let program_title = b"\x1b]2;file.txt - VIM\x07";
        shell.sleep();
        shell.output.forward(program_title).unwrap();

        shell.output.update_title();
        assert_eq!(shell.written(), program_title);

        shell.interrupt();
        shell.output.update_title();
        assert_eq!(shell.written(), [program_title.to_vec(), shell.shell_title()].concat());
    }

    #[rstest]
    fn a_title_the_shell_sets_stands_while_its_commands_run(mut shell: Shell) {
        let shell_title = b"\x1b]0;~/project\x07";
        shell.output.forward(shell_title).unwrap();

        shell.output.update_title();
        shell.sleep();
        shell.output.update_title();
        shell.interrupt();
        shell.output.update_title();

        assert_eq!(shell.written(), shell_title);
    }

    #[rstest]
    fn titles_wait_for_an_escape_sequence_to_finish(shell: Shell) {
        shell.output.forward(b"\x1b[3").unwrap();
        shell.output.update_title();
        assert_eq!(shell.written(), b"\x1b[3");

        shell.output.forward(b"1m").unwrap();
        shell.output.update_title();
        assert_eq!(shell.written(), [b"\x1b[31m".to_vec(), shell.shell_title()].concat());
    }
}
