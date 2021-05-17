//! Cursor movement.

use std::fmt;
use std::ops;
use std::io::{self, Write, Error, ErrorKind, Read};
use async::async_stdin_until;
use std::time::{SystemTime, Duration};
use raw::CONTROL_SEQUENCE_TIMEOUT;
use numtoa::NumToA;

derive_csi_sequence!("Hide the cursor.", Hide, "?25l");
derive_csi_sequence!("Show the cursor.", Show, "?25h");

derive_csi_sequence!("Restore the cursor.", Restore, "u");
derive_csi_sequence!("Save the cursor.", Save, "s");

derive_csi_sequence!("Change the cursor style to blinking block", BlinkingBlock, "\x31 q");
derive_csi_sequence!("Change the cursor style to steady block", SteadyBlock, "\x32 q");
derive_csi_sequence!("Change the cursor style to blinking underline", BlinkingUnderline, "\x33 q");
derive_csi_sequence!("Change the cursor style to steady underline", SteadyUnderline, "\x34 q");
derive_csi_sequence!("Change the cursor style to blinking bar", BlinkingBar, "\x35 q");
derive_csi_sequence!("Change the cursor style to steady bar", SteadyBar, "\x36 q");

/// Goto some position ((1,1)-based).
///
/// # Why one-based?
///
/// ANSI escapes are very poorly designed, and one of the many odd aspects is being one-based. This
/// can be quite strange at first, but it is not that big of an obstruction once you get used to
/// it.
///
/// # Example
///
/// ```rust
/// extern crate termion;
///
/// fn main() {
///     print!("{}{}Stuff", termion::clear::All, termion::cursor::Goto(5, 3));
/// }
/// ```
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Goto(pub u16, pub u16);

impl From<Goto> for String {
    fn from(this: Goto) -> String {
        let (mut x, mut y) = ([0u8; 20], [0u8; 20]);
        ["\x1B[", this.1.numtoa_str(10, &mut x), ";", this.0.numtoa_str(10, &mut y), "H"].concat()
    }
}

impl Default for Goto {
    fn default() -> Goto {
        Goto(1, 1)
    }
}

impl fmt::Display for Goto {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        debug_assert!(self != &Goto(0, 0), "Goto is one-based.");
        write!(f, "\x1B[{};{}H", self.1, self.0)
    }
}

/// Move cursor left.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Left(pub u16);

impl From<Left> for String {
    fn from(this: Left) -> String {
        let mut buf = [0u8; 20];
        ["\x1B[", this.0.numtoa_str(10, &mut buf), "D"].concat()
    }
}

impl fmt::Display for Left {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "\x1B[{}D", self.0)
    }
}

/// Move cursor right.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Right(pub u16);

impl From<Right> for String {
    fn from(this: Right) -> String {
        let mut buf = [0u8; 20];
        ["\x1B[", this.0.numtoa_str(10, &mut buf), "C"].concat()
    }
}

impl fmt::Display for Right {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "\x1B[{}C", self.0)
    }
}

/// Move cursor up.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Up(pub u16);

impl From<Up> for String {
    fn from(this: Up) -> String {
        let mut buf = [0u8; 20];
        ["\x1B[", this.0.numtoa_str(10, &mut buf), "A"].concat()
    }
}

impl fmt::Display for Up {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "\x1B[{}A", self.0)
    }
}

/// Move cursor down.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Down(pub u16);

impl From<Down> for String {
    fn from(this: Down) -> String {
        let mut buf = [0u8; 20];
        ["\x1B[", this.0.numtoa_str(10, &mut buf), "B"].concat()
    }
}

impl fmt::Display for Down {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "\x1B[{}B", self.0)
    }
}

/// Types that allow detection of the cursor position.
pub trait DetectCursorPos {
    /// Get the (1,1)-based cursor position from the terminal.
    fn cursor_pos(&mut self) -> io::Result<(u16, u16)>;
}

impl<W: Write> DetectCursorPos for W {
    fn cursor_pos(&mut self) -> io::Result<(u16, u16)> {
        let delimiter = b'R';
        let mut stdin = async_stdin_until(delimiter);

        // Where is the cursor?
        // Use `ESC [ 6 n`.
        write!(self, "\x1B[6n")?;
        self.flush()?;

        let mut buf: [u8; 1] = [0];
        let mut read_chars = Vec::new();

        let timeout = Duration::from_millis(CONTROL_SEQUENCE_TIMEOUT);
        let now = SystemTime::now();

        // Either consume all data up to R or wait for a timeout.
        while buf[0] != delimiter && now.elapsed().unwrap() < timeout {
            if stdin.read(&mut buf)? > 0 {
                read_chars.push(buf[0]);
            }
        }

        if read_chars.is_empty() {
            return Err(Error::new(ErrorKind::Other, "Cursor position detection timed out."));
        }

        // The answer will look like `ESC [ Cy ; Cx R`.

        read_chars.pop(); // remove trailing R.
        let read_str = String::from_utf8(read_chars).unwrap();
        let beg = read_str.rfind('[').unwrap();
        let coords: String = read_str.chars().skip(beg + 1).collect();
        let mut nums = coords.split(';');

        let cy = nums.next()
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let cx = nums.next()
            .unwrap()
            .parse::<u16>()
            .unwrap();

        Ok((cx, cy))
    }
}

/// Hide the cursor for the lifetime of this struct.
/// It will hide the cursor on creation with from() and show it back on drop().
pub struct HideCursor<W: Write> {
    /// The output target.
    output: W,
}

impl<W: Write> HideCursor<W> {
    /// Create a hide cursor wrapper struct for the provided output and hides the cursor.
    pub fn from(mut output: W) -> Self {
        write!(output, "{}", Hide).expect("hide the cursor");
        HideCursor { output: output }
    }
}

impl<W: Write> Drop for HideCursor<W> {
    fn drop(&mut self) {
        write!(self, "{}", Show).expect("show the cursor");
    }
}

impl<W: Write> ops::Deref for HideCursor<W> {
    type Target = W;

    fn deref(&self) -> &W {
        &self.output
    }
}

impl<W: Write> ops::DerefMut for HideCursor<W> {
    fn deref_mut(&mut self) -> &mut W {
        &mut self.output
    }
}

impl<W: Write> Write for HideCursor<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}
