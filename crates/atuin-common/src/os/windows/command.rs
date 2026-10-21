//! Spawning processes that inherit none of our handles.
//!
//! Arguments are escaped into a command line by Windows' rules. Every byte those rules care about
//! (`"`, `\`, space, tab) is ASCII, and ASCII never occurs inside a multi-byte character, so
//! escaping an `OsStr`'s encoded bytes gives the same result as escaping its UTF-16.
#![allow(unsafe_code, reason = "win32 API calls all require unsafe.")]

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::{io, iter, ptr};

use windows_sys::Win32::System::Threading::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};

use super::fallible_do;
use super::process::Handle;
use crate::string::NonNulStr;

/// A process builder mirroring [`std::process::Command`], whose child inherits none of our handles.
///
/// std always lets the child inherit every inheritable handle, so a long-lived child holds open
/// pipes such as the one a shell reads our output from. In exchange the child has no stdio,
/// `program` must be a path with its extension (PATH is not searched), and batch files are
/// refused, since cmd.exe re-parses their arguments.
// TODO(markovejnovic): replace with std's Command and `.inherit_handles(false)` once
// rust-lang/rust#146407 stabilizes.
#[derive(Debug)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    current_dir: Option<PathBuf>,
    creation_flags: u32,
}

impl Command {
    #[must_use]
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            args: Vec::new(),
            current_dir: None,
            creation_flags: 0,
        }
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args.extend(args.into_iter().map(|arg| arg.as_ref().to_owned()));
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.current_dir = Some(dir.as_ref().to_owned());
        self
    }

    /// Set the [process creation flags] passed to `CreateProcessW`.
    ///
    /// [process creation flags]: https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags
    pub fn creation_flags(&mut self, flags: u32) -> &mut Self {
        self.creation_flags = flags;
        self
    }

    /// Start the child, returning a handle to it.
    pub fn spawn(&self) -> io::Result<Handle> {
        if self.is_batch_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "refusing to spawn batch file {}: cmd.exe re-parses its arguments",
                    self.program.display()
                ),
            ));
        }
        let program = Self::non_nul(&self.program)?;
        let args =
            self.args.iter().map(|arg| Self::non_nul(arg)).collect::<io::Result<Vec<_>>>()?;
        let current_dir =
            self.current_dir.as_deref().map(|dir| Self::non_nul(dir.as_os_str())).transpose()?;

        let application = Self::nul_terminated(program);
        // Escaping NUL-free arguments adds no NUL.
        let mut command_line: Vec<u16> =
            Self::argv_to_command_line(program, &args)?.encode_wide().chain([0]).collect();
        let current_dir = current_dir.map(Self::nul_terminated);
        let startup_info = STARTUPINFOW {
            cb: u32::try_from(size_of::<STARTUPINFOW>()).expect("STARTUPINFOW is ~100 bytes"),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();

        // SAFETY: the strings are NUL-terminated, `command_line` is writable as CreateProcessW
        // requires, and every pointer outlives the call.
        fallible_do(|| unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                self.creation_flags,
                ptr::null(),
                current_dir.as_ref().map_or(ptr::null(), |dir| dir.as_ptr()),
                &raw const startup_info,
                &raw mut info,
            )
        })?;

        // SAFETY: CreateProcessW succeeded, so `hProcess` is a valid handle the caller owns.
        let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
        // SAFETY: as above, for `hThread`, which we never use.
        drop(unsafe { OwnedHandle::from_raw_handle(info.hThread) });
        Ok(Handle::new(process, info.dwProcessId))
    }

    /// Whether `program` names a batch file once Windows strips its trailing dots and spaces.
    fn is_batch_file(&self) -> bool {
        let name = self.program.to_string_lossy();
        Path::new(name.trim_end_matches(['.', ' ']))
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd"))
    }

    fn non_nul(s: &OsStr) -> io::Result<NonNulStr<&OsStr>> {
        NonNulStr::new(s).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))
    }

    fn nul_terminated(s: NonNulStr<&OsStr>) -> Vec<u16> {
        s.as_os_str().encode_wide().chain([0]).collect()
    }

    /// Quote `program` and escape `args` into a command line, as std does.
    fn argv_to_command_line(
        program: NonNulStr<&OsStr>,
        args: &[NonNulStr<&OsStr>],
    ) -> io::Result<OsString> {
        // A Windows path cannot contain `"`, so argv[0] is quoted without escaping.
        if program.as_os_str().as_encoded_bytes().contains(&b'"') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "program path contains a quote",
            ));
        }
        let mut line = OsString::from("\"");
        line.push(program.as_os_str());
        line.push("\"");
        for &arg in args {
            line.push(" ");
            line.push(Self::escape_arg(arg));
        }
        Ok(line)
    }

    /// Escape `arg` so `CommandLineToArgvW` reads it back, quoting it only if it is empty or
    /// contains whitespace.
    fn escape_arg(arg: NonNulStr<&OsStr>) -> OsString {
        let bytes = arg.as_os_str().as_encoded_bytes();
        let quote = bytes.is_empty() || bytes.iter().any(|&b| b == b' ' || b == b'\t');
        let mut escaped = Vec::with_capacity(bytes.len() + 2);
        if quote {
            escaped.push(b'"');
        }

        let mut backslashes = 0;
        for &byte in bytes {
            match byte {
                b'\\' => backslashes += 1,
                // 2n+1 backslashes before an embedded quote: n literal ones and the escape.
                b'"' => {
                    escaped.extend(iter::repeat_n(b'\\', backslashes + 1));
                    backslashes = 0;
                }
                _ => backslashes = 0,
            }
            escaped.push(byte);
        }

        if quote {
            // Double any trailing backslashes so they don't escape the closing quote.
            escaped.extend(iter::repeat_n(b'\\', backslashes));
            escaped.push(b'"');
        }
        // SAFETY: `escaped` is `arg`'s encoded bytes with ASCII inserted only before a `"` and at
        // the ends, which are boundaries where `from_encoded_bytes_unchecked` allows joining UTF-8.
        unsafe { OsString::from_encoded_bytes_unchecked(escaped) }
    }
}

#[cfg(test)]
mod tests {
    use std::io::PipeWriter;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::slice;
    use std::time::{Duration, Instant};

    use proptest::prelude::*;
    use rstest::{fixture, rstest};
    use windows_sys::Win32::Foundation::{
        ERROR_BROKEN_PIPE, HANDLE_FLAG_INHERIT, LocalFree, SetHandleInformation,
    };
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;
    use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

    use super::*;
    use crate::os::windows::get_last_error;

    const PROGRAM: &str = r"C:\Program Files\atuin\atuin.exe";

    #[fixture]
    fn cmd() -> OsString {
        crate::env::var_os("ComSpec").expect("Windows sets ComSpec")
    }

    fn arg(s: &str) -> NonNulStr<&OsStr> {
        NonNulStr::new(OsStr::new(s)).unwrap()
    }

    /// Split `line` by the rules of std's Windows `parse_lp_cmd_line`, which is how a Rust child
    /// such as the daemon reads its arguments, dropping argv[0].
    fn parse_args(line: &OsStr) -> Vec<String> {
        let is_blank = |&b: &u8| b == b' ' || b == b'\t';
        let mut bytes = line.as_encoded_bytes().iter().copied().peekable();

        // argv[0] has no escapes: quotes only toggle, and unquoted whitespace ends it.
        let mut in_quotes = false;
        for byte in bytes.by_ref() {
            match byte {
                b'"' => in_quotes = !in_quotes,
                b' ' | b'\t' if !in_quotes => break,
                _ => {}
            }
        }
        while bytes.next_if(is_blank).is_some() {}

        let mut args = Vec::new();
        let mut cur = Vec::new();
        let mut in_quotes = false;
        while let Some(byte) = bytes.next() {
            match byte {
                b' ' | b'\t' if !in_quotes => {
                    args.push(std::mem::take(&mut cur));
                    while bytes.next_if(is_blank).is_some() {}
                }
                b'\\' => {
                    let mut count = 1;
                    while bytes.next_if_eq(&b'\\').is_some() {
                        count += 1;
                    }
                    if bytes.peek() == Some(&b'"') {
                        cur.extend(iter::repeat_n(b'\\', count / 2));
                        if count % 2 == 1 {
                            bytes.next();
                            cur.push(b'"');
                        }
                    } else {
                        cur.extend(iter::repeat_n(b'\\', count));
                    }
                }
                b'"' if in_quotes => match bytes.peek() {
                    Some(&b'"') => {
                        bytes.next();
                        cur.push(b'"');
                    }
                    Some(_) => in_quotes = false,
                    None => break,
                },
                b'"' => in_quotes = true,
                _ => cur.push(byte),
            }
        }
        if !cur.is_empty() || in_quotes {
            args.push(cur);
        }
        args.into_iter().map(|arg| String::from_utf8(arg).unwrap()).collect()
    }

    /// Split `line` the way a child reads its command line, dropping argv[0].
    fn command_line_to_argv(line: &OsStr) -> Vec<String> {
        let line: Vec<u16> = line.encode_wide().chain([0]).collect();
        let mut argc = 0;
        // SAFETY: `line` is NUL-terminated.
        let argv = unsafe { CommandLineToArgvW(line.as_ptr(), &raw mut argc) };
        assert!(!argv.is_null(), "CommandLineToArgvW failed: {}", get_last_error());
        let argc = usize::try_from(argc).unwrap();
        // SAFETY: on success, `argv` points at `argc` string pointers.
        let args = unsafe { slice::from_raw_parts(argv, argc) }
            .iter()
            .skip(1)
            .map(|&arg| {
                let mut len = 0;
                // SAFETY: `arg` is NUL-terminated, so every read up to the NUL is in bounds.
                while unsafe { arg.wrapping_add(len).read() } != 0 {
                    len += 1;
                }
                // SAFETY: the `len` units before the NUL were just read.
                String::from_utf16(unsafe { slice::from_raw_parts(arg, len) }).unwrap()
            })
            .collect();
        // SAFETY: CommandLineToArgvW allocates `argv` for the caller to free with LocalFree.
        unsafe { LocalFree(argv.cast()) };
        args
    }

    #[rstest]
    #[case::plain("plain", "plain")]
    #[case::empty("", r#""""#)]
    #[case::space("a b", r#""a b""#)]
    #[case::quote(r#"a"b"#, r#"a\"b"#)]
    #[case::lone_backslash(r"a\b", r"a\b")]
    #[case::backslash_before_a_quote(r#"a\"b"#, r#"a\\\"b"#)]
    #[case::trailing_backslash_in_quotes(r"a b\", r#""a b\\""#)]
    fn escapes_exactly_as_std_does(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(Command::escape_arg(arg(input)), expected);
    }

    #[rstest]
    #[case::bare(&["daemon", "start"])]
    #[case::empty(&[""])]
    #[case::space(&["a b"])]
    #[case::tab(&["a\tb"])]
    #[case::quote(&["a\"b"])]
    #[case::backslashes_before_a_quote(&[r#"a\\"b"#])]
    #[case::trailing_backslash(&[r"a\"])]
    #[case::trailing_backslash_in_quotes(&[r"a b\\"])]
    #[case::non_ascii(&["é ü"])]
    fn arguments_round_trip(#[case] args: &[&str]) {
        let argv: Vec<_> = args.iter().copied().map(arg).collect();
        let line = Command::argv_to_command_line(arg(PROGRAM), &argv).unwrap();
        assert_eq!(command_line_to_argv(&line), args);
    }

    proptest! {
        #[test]
        fn any_arguments_round_trip(
            args in prop::collection::vec(
                prop_oneof![r#"[a é\t\\"]{0,8}"#, any::<String>()]
                    .prop_filter("NUL cannot be passed", |arg| !arg.contains('\0')),
                0..6,
            ),
        ) {
            let argv: Vec<_> = args.iter().map(String::as_str).map(arg).collect();
            let line = Command::argv_to_command_line(arg(PROGRAM), &argv).unwrap();
            prop_assert_eq!(&parse_args(&line), &args);
            prop_assert_eq!(&command_line_to_argv(&line), &args);
        }
    }

    #[rstest]
    #[case::bat(r"C:\run.bat", &[])]
    #[case::cmd_in_capitals(r"C:\run.CMD", &[])]
    #[case::bat_with_trailing_dot(r"C:\run.bat.", &[])]
    #[case::cmd_with_trailing_space(r"C:\run.cmd ", &[])]
    #[case::quote_in_program(r#"C:\a"b.exe"#, &[])]
    #[case::nul_in_program("C:\\a\0b.exe", &[])]
    #[case::nul_in_arg(PROGRAM, &["a\0b"])]
    fn refuses_input_it_cannot_pass_on_faithfully(#[case] program: &str, #[case] args: &[&str]) {
        let err = Command::new(program).args(args).spawn().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    /// Spawn a suspended `cmd.exe` with our `Command`, returning what kills it.
    fn spawn_suspended(cmd: &OsStr) -> Box<dyn FnOnce()> {
        let child =
            Command::new(cmd).creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW).spawn().unwrap();
        Box::new(move || child.terminate().unwrap())
    }

    /// Spawn a suspended `cmd.exe` with std's `Command`, returning what kills it.
    fn spawn_suspended_with_std(cmd: &OsStr) -> Box<dyn FnOnce()> {
        let mut child = std::process::Command::new(cmd)
            .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        Box::new(move || {
            child.kill().unwrap();
            child.wait().unwrap();
        })
    }

    /// Whether anyone besides `reader` still holds the pipe, given we closed our writer.
    fn has_writer(reader: &std::io::PipeReader) -> bool {
        // SAFETY: `reader` is a live pipe handle, and every out-pointer may be null.
        let peeked = fallible_do(|| unsafe {
            PeekNamedPipe(
                reader.as_raw_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        });
        match peeked {
            Ok(()) => true,
            Err(err) if err.raw_os_error() == Some(ERROR_BROKEN_PIPE.cast_signed()) => false,
            Err(err) => panic!("PeekNamedPipe failed: {err}"),
        }
    }

    fn make_inheritable(writer: &PipeWriter) {
        // SAFETY: `writer` is a live pipe handle.
        fallible_do(|| unsafe {
            SetHandleInformation(writer.as_raw_handle(), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT)
        })
        .unwrap();
    }

    /// A child must not keep open a pipe we hold, such as the one a shell reads our output from,
    /// or whoever reads it waits until the child exits. std's `Command` is the control: it does
    /// inherit, which proves the check can fail.
    // Relies on nextest running each test in its own process: under `cargo test`, a sibling
    // test's child could inherit the writer too.
    #[rstest]
    #[case::ours(spawn_suspended, false)]
    #[case::std_inherits(spawn_suspended_with_std, true)]
    fn a_child_holds_our_pipe_only_if_it_inherits_handles(
        cmd: OsString,
        #[case] spawn: fn(&OsStr) -> Box<dyn FnOnce()>,
        #[case] inherits: bool,
    ) {
        let (reader, writer) = std::io::pipe().unwrap();
        make_inheritable(&writer);
        // A suspended child's handle table is complete before it runs, so nothing here races.
        let kill = spawn(&cmd);
        drop(writer);

        let held = has_writer(&reader);
        kill();
        assert_eq!(held, inherits);
    }

    #[rstest]
    fn runs_the_child_in_the_given_directory(cmd: OsString) {
        let dir = tempfile::tempdir().unwrap();
        let child = Command::new(cmd)
            .args(["/c", "cd", ">", "cwd.txt"])
            .current_dir(dir.path())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while child.is_alive() {
            assert!(Instant::now() < deadline, "cmd.exe did not exit within 10s");
            std::thread::sleep(Duration::from_millis(10));
        }

        let reported = std::fs::read_to_string(dir.path().join("cwd.txt")).unwrap();
        assert_eq!(
            Path::new(reported.trim_end()).canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }
}
