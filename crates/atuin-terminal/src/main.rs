#[cfg(all(unix, not(target_os = "illumos")))]
use std::io::{Read, Write};

#[cfg(all(unix, not(target_os = "illumos")))]
use crossterm::terminal;
#[cfg(all(unix, not(target_os = "illumos")))]
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

#[cfg(all(unix, not(target_os = "illumos")))]
fn main() {
    if let Err(e) = run() {
        let _ = terminal::disable_raw_mode();
        eprintln!("atuin-terminal: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(not(all(unix, not(target_os = "illumos"))))]
fn main() {
    eprintln!("atuin-terminal currently supports unix platforms excluding illumos");
    std::process::exit(1);
}

#[cfg(all(unix, not(target_os = "illumos")))]
fn run() -> eyre::Result<()> {
    let (cols, rows) = terminal::size()?;

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| eyre::eyre!("{e:#}"))?;

    let mut cmd = CommandBuilder::new_default_prog();
    cmd.cwd(std::env::current_dir()?);
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| eyre::eyre!("{e:#}"))?;

    // Close slave side in parent process
    drop(pair.slave);

    let mut pty_reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| eyre::eyre!("{e:#}"))?;
    let mut pty_writer = pair
        .master
        .take_writer()
        .map_err(|e| eyre::eyre!("{e:#}"))?;

    // Handle terminal resize via SIGWINCH
    {
        use signal_hook::consts::SIGWINCH;
        use signal_hook::iterator::Signals;

        let master = pair.master;
        let mut signals = Signals::new([SIGWINCH])?;

        std::thread::spawn(move || {
            for _ in signals.forever() {
                if let Ok((cols, rows)) = terminal::size() {
                    let _ = master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
        });
    }

    terminal::enable_raw_mode()?;

    // PTY -> stdout
    let stdout_thread = std::thread::spawn(move || {
        let mut stdout = std::io::stdout();
        let mut buf = [0u8; 8192];
        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if stdout.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = stdout.flush();
                }
            }
        }
    });

    // stdin -> PTY
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 8192];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if pty_writer.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let status = child.wait()?;
    let _ = stdout_thread.join();

    let _ = terminal::disable_raw_mode();

    std::process::exit(process_exit_code(status.exit_code()));
}

#[cfg(all(unix, not(target_os = "illumos")))]
fn process_exit_code(code: u32) -> i32 {
    i32::try_from(code).unwrap_or(1)
}

#[cfg(test)]
#[cfg(all(unix, not(target_os = "illumos")))]
mod tests {
    use super::process_exit_code;

    #[test]
    fn process_exit_code_preserves_valid_values() {
        assert_eq!(process_exit_code(0), 0);
        assert_eq!(process_exit_code(127), 127);
        assert_eq!(process_exit_code(i32::MAX as u32), i32::MAX);
    }

    #[test]
    fn process_exit_code_defaults_when_out_of_range() {
        assert_eq!(process_exit_code(i32::MAX as u32 + 1), 1);
    }
}
