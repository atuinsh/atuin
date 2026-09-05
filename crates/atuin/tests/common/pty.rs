use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use nix_pty::sys::termios::LocalFlags;
use parking_lot::{Condvar, Mutex};

use crate::common::{FreshEnv, TIMEOUT, wait_until};
use crate::shell::PROMPT;

struct PtyState {
    parser: vt100::Parser,
    pending: Vec<u8>,
    closed: bool,
}

/// A PTY shell with a rendered screen and replies to terminal queries.
pub struct PtyShell {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    // Keeps the master side (and thus the PTY) alive for the session.
    master: Box<dyn portable_pty::MasterPty + Send>,
    state: Arc<(Mutex<PtyState>, Condvar)>,
}

/// Process output in order so cursor reports reflect the position at the query.
fn answer_queries(state: &mut PtyState) -> Vec<u8> {
    const QUERIES: [&[u8]; 6] =
        [b"\x1b[6n", b"\x1b[c", b"\x1b[0c", b"\x1b[>c", b"\x1b[>0c", b"\x1b[5n"];
    let mut replies: Vec<u8> = Vec::new();
    let mut i = 0;
    'scan: while i < state.pending.len() {
        let tail = &state.pending[i..];
        for query in QUERIES {
            if tail.starts_with(query) {
                state.parser.process(query);
                let reply: Vec<u8> = match query {
                    b"\x1b[6n" => {
                        let (row, col) = state.parser.screen().cursor_position();
                        format!("\x1b[{};{}R", row + 1, col + 1).into_bytes()
                    }
                    // primary DA: "VT100 with advanced video option"
                    b"\x1b[c" | b"\x1b[0c" => b"\x1b[?1;2c".to_vec(),
                    // secondary DA: xterm-ish
                    b"\x1b[>c" | b"\x1b[>0c" => b"\x1b[>1;10;0c".to_vec(),
                    b"\x1b[5n" => b"\x1b[0n".to_vec(),
                    _ => unreachable!(),
                };
                replies.extend_from_slice(&reply);
                i += query.len();
                continue 'scan;
            }
            // A query split across reads: stop here and wait for the rest.
            if tail.len() < query.len() && query.starts_with(tail) {
                break 'scan;
            }
        }
        state.parser.process(&state.pending[i..=i]);
        i += 1;
    }
    state.pending.drain(..i);
    replies
}

impl PtyShell {
    pub fn spawn(
        shell_path: &Path,
        args: &[String],
        env: &FreshEnv,
        vars: &BTreeMap<String, String>,
    ) -> Self {
        // Taller than the default inline_height (40) so the search UI never
        // has to scroll the prompt out of view.
        let (rows, cols) = (50, 120);
        let pty = portable_pty::native_pty_system();
        let pair = pty
            .openpty(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty");

        let mut cmd = portable_pty::CommandBuilder::new(shell_path);
        cmd.args(args);
        cmd.env_clear();
        for (k, v) in env.env_vars() {
            cmd.env(k, v);
        }
        for (key, value) in vars {
            cmd.env(key, value);
        }
        cmd.env("SHELL", shell_path);
        cmd.cwd(env.home());

        let child = pair.slave.spawn_command(cmd).expect("failed to spawn shell");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("failed to clone pty reader");
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer().expect("failed to take pty writer")));
        let state = Arc::new((
            Mutex::new(PtyState {
                parser: vt100::Parser::new(
                    NonZeroU16::new(rows).unwrap(),
                    NonZeroU16::new(cols).unwrap(),
                    0,
                ),
                pending: Vec::new(),
                closed: false,
            }),
            Condvar::new(),
        ));

        let thread_state = Arc::clone(&state);
        let thread_writer = Arc::clone(&writer);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut state = thread_state.0.lock();
                state.pending.extend_from_slice(&buf[..n]);
                let replies = answer_queries(&mut state);
                thread_state.1.notify_all();
                drop(state);
                if !replies.is_empty() {
                    let mut writer = thread_writer.lock();
                    if writer.write_all(&replies).and_then(|()| writer.flush()).is_err() {
                        break;
                    }
                }
            }
            thread_state.0.lock().closed = true;
            thread_state.1.notify_all();
        });

        Self {
            child,
            writer,
            master: pair.master,
            state,
        }
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        let mut state = self.state.0.lock();
        state
            .parser
            .screen_mut()
            .set_size(NonZeroU16::new(rows).unwrap(), NonZeroU16::new(cols).unwrap());
        self.master
            .resize(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to resize pty");
        drop(state);
    }

    /// The current rendered screen contents.
    pub fn screen(&self) -> String {
        self.state.0.lock().parser.screen().contents()
    }

    pub fn send(&self, bytes: &[u8]) {
        let mut writer = self.writer.lock();
        writer.write_all(bytes).expect("failed to write to pty");
        writer.flush().expect("failed to flush pty");
    }

    /// Wait for each character to be rendered before sending the next one.
    pub fn send_str(&self, text: &str) {
        for ch in text.chars() {
            let (cursor, line) = {
                let state = self.state.0.lock();
                let screen = state.parser.screen();
                let cursor = screen.cursor_position();
                let snapshot =
                    (cursor, screen.rows(0, screen.size().1.get()).nth(usize::from(cursor.0)));
                drop(state);
                snapshot
            };
            self.send(ch.encode_utf8(&mut [0; 4]).as_bytes());
            self.wait_for_terminal("typed character", |state| {
                let screen = state.parser.screen();
                screen.cursor_position() != cursor
                    || screen.rows(0, screen.size().1.get()).nth(usize::from(cursor.0)) != line
            });
        }
    }

    pub fn send_line(&self, text: &str) {
        self.send_str(text);
        self.send_enter();
        self.wait_for_prompt();
    }

    pub fn wait_for_prompt(&self) {
        self.wait_for_terminal("empty shell prompt", |state| {
            let screen = state.parser.screen();
            screen
                .rows(0, screen.size().1.get())
                .nth(usize::from(screen.cursor_position().0))
                .is_some_and(|line| line.trim() == PROMPT)
        });
        wait_until("shell line editor ready", || {
            self.master.get_termios().is_some_and(|termios| {
                !termios.local_flags.intersects(LocalFlags::ICANON | LocalFlags::ECHO)
            })
        });
    }

    pub fn send_ctrl_r(&self) {
        self.send(&[0x12]);
    }

    pub fn send_enter(&self) {
        self.send(b"\r");
    }

    fn wait_for_terminal(&self, what: &str, pred: impl Fn(&PtyState) -> bool) -> String {
        let deadline = Instant::now() + TIMEOUT;
        let mut state = self.state.0.lock();
        let contents = loop {
            let screen = state.parser.screen();
            if pred(&state) {
                break screen.contents();
            }
            assert!(
                !state.closed && Instant::now() < deadline,
                "waiting for {what} (PTY closed: {}); screen:\n{}",
                state.closed,
                screen.contents()
            );
            self.state.1.wait_for(&mut state, deadline.saturating_duration_since(Instant::now()));
        };
        drop(state);
        contents
    }

    pub fn wait_for_screen(&self, what: &str, pred: impl Fn(&str) -> bool) -> String {
        self.wait_for_terminal(what, |state| pred(&state.parser.screen().contents()))
    }

    /// Wait until `needle` appears anywhere on the screen.
    pub fn wait_for(&self, needle: &str) -> String {
        self.wait_for_screen(&format!("{needle:?} on screen"), |s| s.contains(needle))
    }

    /// Wait until some screen line, trimmed, is exactly `line` — e.g. command
    /// output as opposed to the echoed command line itself.
    pub fn wait_for_line(&self, line: &str) -> String {
        self.wait_for_screen(&format!("line {line:?} on screen"), |s| {
            s.lines().any(|l| l.trim() == line)
        })
    }
}

impl Drop for PtyShell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[rstest::rstest]
fn cursor_reports_follow_output_order(#[values(0, 1, 5, 7, 9, 12, 16)] split: usize) {
    let mut state = PtyState {
        parser: vt100::Parser::new(NonZeroU16::new(24).unwrap(), NonZeroU16::new(80).unwrap(), 0),
        pending: Vec::new(),
        closed: false,
    };
    let output = b"abc\x1b[6ndef\x1b[6nghi";
    state.pending.extend_from_slice(&output[..split]);
    let mut replies = answer_queries(&mut state);
    state.pending.extend_from_slice(&output[split..]);
    replies.extend(answer_queries(&mut state));
    assert_eq!(replies, b"\x1b[1;4R\x1b[1;7R");
    assert_eq!(state.parser.screen().contents(), "abcdefghi");
    assert!(state.pending.is_empty());
}
