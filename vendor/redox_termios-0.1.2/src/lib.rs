#![allow(non_camel_case_types)]
#![no_std]

extern crate syscall;

use core::{mem, slice};
use core::ops::{Deref, DerefMut};

pub type tcflag_t = u32;
pub type cc_t = u8;

/* c_cc { */
pub const VEOF: usize = 0;
pub const VEOL: usize = 1;
pub const VEOL2: usize = 2;
pub const VERASE: usize = 3;
pub const VWERASE: usize = 4;
pub const VKILL: usize = 5;
pub const VREPRINT: usize = 6;
pub const VSWTC: usize = 7;
pub const VINTR: usize = 8;
pub const VQUIT: usize = 9;
pub const VSUSP: usize = 10;
pub const VSTART: usize = 12;
pub const VSTOP: usize = 13;
pub const VLNEXT: usize = 14;
pub const VDISCARD: usize = 15;
pub const VMIN: usize = 16;
pub const VTIME: usize = 17;
pub const NCCS: usize = 32;
/* } c_cc */

/* c_iflag { */
pub const IGNBRK: tcflag_t = 0o000001;
pub const BRKINT: tcflag_t = 0o000002;
pub const IGNPAR: tcflag_t = 0o000004;
pub const PARMRK: tcflag_t = 0o000010;
pub const INPCK: tcflag_t = 0o000020;
pub const ISTRIP: tcflag_t = 0o000040;
pub const INLCR: tcflag_t = 0o000100;
pub const IGNCR: tcflag_t = 0o000200;
pub const ICRNL: tcflag_t = 0o000400;
pub const IXON: tcflag_t = 0o001000;
pub const IXOFF: tcflag_t = 0o002000;
/* } c_iflag */

/* c_oflag { */
pub const OPOST: tcflag_t = 0o000001;
pub const ONLCR: tcflag_t = 0o000002;
pub const OLCUC: tcflag_t = 0o000004;

pub const OCRNL: tcflag_t = 0o000010;
pub const ONOCR: tcflag_t = 0o000020;
pub const ONLRET: tcflag_t = 0o000040;

pub const OFILL: tcflag_t = 0o0000100;
pub const OFDEL: tcflag_t = 0o0000200;
/* } c_oflag */

/* c_cflag { */
pub const  B0: tcflag_t = 0o000000;
pub const  B50: tcflag_t = 0o000001;
pub const  B75: tcflag_t = 0o000002;
pub const  B110: tcflag_t = 0o000003;
pub const  B134: tcflag_t = 0o000004;
pub const  B150: tcflag_t = 0o000005;
pub const  B200: tcflag_t = 0o000006;
pub const  B300: tcflag_t = 0o000007;
pub const  B600: tcflag_t = 0o000010;
pub const  B1200: tcflag_t = 0o000011;
pub const  B1800: tcflag_t = 0o000012;
pub const  B2400: tcflag_t = 0o000013;
pub const  B4800: tcflag_t = 0o000014;
pub const  B9600: tcflag_t = 0o000015;
pub const  B19200: tcflag_t = 0o000016;
pub const  B38400: tcflag_t = 0o000017;
pub const  B57600: tcflag_t = 0o0020;
pub const  B115200: tcflag_t = 0o0021;
pub const  B230400: tcflag_t = 0o0022;
pub const  B460800: tcflag_t = 0o0023;
pub const  B500000: tcflag_t = 0o0024;
pub const  B576000: tcflag_t = 0o0025;
pub const  B921600: tcflag_t = 0o0026;
pub const  B1000000: tcflag_t = 0o0027;
pub const  B1152000: tcflag_t = 0o0030;
pub const  B1500000: tcflag_t = 0o0031;
pub const  B2000000: tcflag_t = 0o0032;
pub const  B2500000: tcflag_t = 0o0033;
pub const  B3000000: tcflag_t = 0o0034;
pub const  B3500000: tcflag_t = 0o0035;
pub const  B4000000: tcflag_t = 0o0036;

pub const __MAX_BAUD: tcflag_t = B4000000;

pub const CSIZE: tcflag_t = 0o0001400;
pub const   CS5: tcflag_t = 0o0000000;
pub const   CS6: tcflag_t = 0o0000400;
pub const   CS7: tcflag_t = 0o0001000;
pub const   CS8: tcflag_t = 0o0001400;

pub const CSTOPB: tcflag_t = 0o0002000;
pub const CREAD: tcflag_t = 0o0004000;
pub const PARENB: tcflag_t = 0o0010000;
pub const PARODD: tcflag_t = 0o0020000;
pub const HUPCL: tcflag_t = 0o0040000;

pub const CLOCAL: tcflag_t = 0o0100000;
/* } c_clfag */

/* c_lflag { */
pub const ISIG: tcflag_t = 0x00000080;
pub const ICANON: tcflag_t = 0x00000100;
pub const ECHO: tcflag_t = 0x00000008;
pub const ECHOE: tcflag_t = 0x00000002;
pub const ECHOK: tcflag_t = 0x00000004;
pub const ECHONL: tcflag_t = 0x00000010;
pub const NOFLSH: tcflag_t = 0x80000000;
pub const TOSTOP: tcflag_t = 0x00400000;
pub const IEXTEN: tcflag_t = 0x00000400;
/* } c_lflag */

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Termios {
    pub c_iflag: tcflag_t,
    pub c_oflag: tcflag_t,
    pub c_cflag: tcflag_t,
    pub c_lflag: tcflag_t,
    pub c_cc: [cc_t; 32]
}

impl Default for Termios {
    fn default() -> Termios {
        let mut termios = Termios {
            c_iflag: ICRNL | IXON,
            c_oflag: OPOST | ONLCR,
            c_cflag: B38400 | CS8 | CREAD | HUPCL,
            c_lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN,
            c_cc: [0; 32]
        };

        {
            let mut cc = |i: usize, b: cc_t| {
                termios.c_cc[i] = b;
            };

            cc(VEOF, 0o004);    // CTRL-D
            cc(VEOL, 0o000);    // NUL
            cc(VEOL2, 0o000);   // NUL
            cc(VERASE, 0o177);  // DEL
            cc(VWERASE, 0o027); // CTRL-W
            cc(VKILL, 0o025);   // CTRL-U
            cc(VREPRINT, 0o022);// CTRL-R
            cc(VINTR, 0o003);   // CTRL-C
            cc(VQUIT, 0o034);   // CTRL-\
            cc(VSUSP, 0o032);   // CTRL-Z
            cc(VSTART, 0o021);  // CTRL-Q
            cc(VSTOP, 0o023);   // CTRL-S
            cc(VLNEXT, 0o026);  // CTRL-V
            cc(VDISCARD, 0o017);// CTRL-U
            cc(VMIN, 1);
            cc(VTIME, 0);
        }

        termios
    }
}

impl Termios {
    pub fn make_raw(&mut self) {
        self.c_iflag &= !(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON);
        self.c_oflag &= !OPOST;
        self.c_cflag &= !(CSIZE | PARENB);
        self.c_cflag |= CS8;
        self.c_lflag &= !(ECHO | ECHONL | ICANON | ISIG | IEXTEN);
    }
}

impl Deref for Termios {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe {
            slice::from_raw_parts(self as *const Termios as *const u8, mem::size_of::<Termios>()) as &[u8]
        }
    }
}

impl DerefMut for Termios {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe {
            slice::from_raw_parts_mut(self as *mut Termios as *mut u8, mem::size_of::<Termios>()) as &mut [u8]
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Winsize {
    pub ws_row: u16,
    pub ws_col: u16
}

impl Deref for Winsize {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe {
            slice::from_raw_parts(self as *const Winsize as *const u8, mem::size_of::<Winsize>()) as &[u8]
        }
    }
}

impl DerefMut for Winsize {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe {
            slice::from_raw_parts_mut(self as *mut Winsize as *mut u8, mem::size_of::<Winsize>()) as &mut [u8]
        }
    }
}
