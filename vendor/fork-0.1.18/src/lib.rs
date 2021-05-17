//! Library for creating a new process detached from the controling terminal (daemon).
//!
//! Example:
//! ```
//!use fork::{daemon, Fork};
//!use std::process::Command;
//!
//!if let Ok(Fork::Child) = daemon(false, false) {
//!    Command::new("sleep")
//!        .arg("3")
//!        .output()
//!        .expect("failed to execute process");
//!}
//!```

use std::ffi::CString;
use std::process::exit;

/// Fork result
pub enum Fork {
    Parent(libc::pid_t),
    Child,
}

/// Change dir to `/` [see chdir(2)](https://www.freebsd.org/cgi/man.cgi?query=chdir&sektion=2)
///
/// Upon successful completion, 0 shall be returned. Otherwise, -1 shall be
/// returned, the current working directory shall remain unchanged, and errno
/// shall be set to indicate the error.
///
/// Example:
///
///```
///use fork::chdir;
///use std::env;
///
///match chdir() {
///    Ok(_) => {
///       let path = env::current_dir().expect("failed current_dir");
///       assert_eq!(Some("/"), path.to_str());
///    }
///    _ => panic!(),
///}
///```
///
/// # Errors
/// returns `-1` if error
pub fn chdir() -> Result<libc::c_int, i32> {
    let dir = CString::new("/").expect("CString::new failed");
    let res = unsafe { libc::chdir(dir.as_ptr()) };
    match res {
        -1 => Err(-1),
        res => Ok(res),
    }
}

/// Close file descriptors stdin,stdout,stderr
///
/// # Errors
/// returns `-1` if error
pub fn close_fd() -> Result<(), i32> {
    match unsafe { libc::close(0) } {
        -1 => Err(-1),
        _ => match unsafe { libc::close(1) } {
            -1 => Err(-1),
            _ => match unsafe { libc::close(2) } {
                -1 => Err(-1),
                _ => Ok(()),
            },
        },
    }
}

/// Create a new child process [see fork(2)](https://www.freebsd.org/cgi/man.cgi?fork)
///
/// Upon successful completion, fork() returns a value of 0 to the child process
/// and returns the process ID of the child process to the parent process.
/// Otherwise, a value of -1 is returned to the parent process, no child process
/// is created.
///
/// Example:
///
/// ```
///use fork::{fork, Fork};
///
///match fork() {
///    Ok(Fork::Parent(child)) => {
///        println!("Continuing execution in parent process, new child has pid: {}", child);
///    }
///    Ok(Fork::Child) => println!("I'm a new child process"),
///    Err(_) => println!("Fork failed"),
///}
///```
/// This will print something like the following (order indeterministic).
///
/// ```text
/// Continuing execution in parent process, new child has pid: 1234
/// I'm a new child process
/// ```
///
/// The thing to note is that you end up with two processes continuing execution
/// immediately after the fork call but with different match arms.
///
/// # [`nix::unistd::fork`](https://docs.rs/nix/0.15.0/nix/unistd/fn.fork.html)
///
/// The example has been taken from the [`nix::unistd::fork`](https://docs.rs/nix/0.15.0/nix/unistd/fn.fork.html),
/// please check the [Safety](https://docs.rs/nix/0.15.0/nix/unistd/fn.fork.html#safety) section
///
/// # Errors
/// returns `-1` if error
pub fn fork() -> Result<Fork, i32> {
    let res = unsafe { libc::fork() };
    match res {
        -1 => Err(-1),
        0 => Ok(Fork::Child),
        res => Ok(Fork::Parent(res)),
    }
}

/// Create session and set process group ID [see setsid(2)](https://www.freebsd.org/cgi/man.cgi?setsid)
///
/// Upon successful completion, the setsid() system call returns the value of the
/// process group ID of the new process group, which is the same as the process ID
/// of the calling process. If an error occurs, setsid() returns -1
///
/// # Errors
/// returns `-1` if error
pub fn setsid() -> Result<libc::pid_t, i32> {
    let res = unsafe { libc::setsid() };
    match res {
        -1 => Err(-1),
        res => Ok(res),
    }
}

/// The process group of the current process [see getgrp(2)](https://www.freebsd.org/cgi/man.cgi?query=getpgrp)
///
/// # Errors
/// returns `-1` if error
pub fn getpgrp() -> Result<libc::pid_t, i32> {
    let res = unsafe { libc::getpgrp() };
    match res {
        -1 => Err(-1),
        res => Ok(res),
    }
}

/// The daemon function is for programs wishing to detach themselves from the
/// controlling terminal and run in the background as system daemons.
///
/// * `nochdir = false`, changes the current working directory to the root (`/`).
/// * `noclose = false`, will close standard input, standard output, and standard error
///
/// # Errors
/// If an error occurs, returns -1
///
/// Example:
///
///```
///// The parent forks the child
///// The parent exits
///// The child calls setsid() to start a new session with no controlling terminals
///// The child forks a grandchild
///// The child exits
///// The grandchild is now the daemon
///use fork::{daemon, Fork};
///use std::process::Command;
///
///if let Ok(Fork::Child) = daemon(false, false) {
///    Command::new("sleep")
///        .arg("3")
///        .output()
///        .expect("failed to execute process");
///}
///```
pub fn daemon(nochdir: bool, noclose: bool) -> Result<Fork, i32> {
    match fork() {
        Ok(Fork::Parent(_)) => exit(0),
        Ok(Fork::Child) => setsid().and_then(|_| {
            if !nochdir {
                chdir()?;
            }
            if !noclose {
                close_fd()?;
            }
            fork()
        }),
        Err(n) => Err(n),
    }
}

#[cfg(test)]
mod tests {
    use super::{fork, Fork};

    #[test]
    fn test_fork() {
        if let Ok(Fork::Parent(child)) = fork() {
            assert!(child > 0);
        }
    }
}
