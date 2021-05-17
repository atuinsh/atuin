use std::io;

use super::{cvt, syscall, Termios};

pub fn get_terminal_attr() -> io::Result<Termios> {
    let mut termios = Termios::default();

    let fd = cvt(syscall::dup(1, b"termios"))?;
    let res = cvt(syscall::read(fd, &mut termios));
    let _ = syscall::close(fd);

    if res? == termios.len() {
        Ok(termios)
    } else {
        Err(io::Error::new(io::ErrorKind::Other, "Unable to get the terminal attributes."))
    }
}

pub fn set_terminal_attr(termios: &Termios) -> io::Result<()> {
    let fd = cvt(syscall::dup(1, b"termios"))?;
    let res = cvt(syscall::write(fd, termios));
    let _ = syscall::close(fd);

    if res? == termios.len() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Other, "Unable to set the terminal attributes."))
    }
}

pub fn raw_terminal_attr(ios: &mut Termios) {
    ios.make_raw()
}
