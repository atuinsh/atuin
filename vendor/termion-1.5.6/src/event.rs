//! Mouse and key events.

use std::io::{Error, ErrorKind};
use std::str;

/// An event reported by the terminal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Event {
    /// A key press.
    Key(Key),
    /// A mouse button press, release or wheel use at specific coordinates.
    Mouse(MouseEvent),
    /// An event that cannot currently be evaluated.
    Unsupported(Vec<u8>),
}

/// A mouse related event.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum MouseEvent {
    /// A mouse button was pressed.
    ///
    /// The coordinates are one-based.
    Press(MouseButton, u16, u16),
    /// A mouse button was released.
    ///
    /// The coordinates are one-based.
    Release(u16, u16),
    /// A mouse button is held over the given coordinates.
    ///
    /// The coordinates are one-based.
    Hold(u16, u16),
}

/// A mouse button.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// The left mouse button.
    Left,
    /// The right mouse button.
    Right,
    /// The middle mouse button.
    Middle,
    /// Mouse wheel is going up.
    ///
    /// This event is typically only used with Mouse::Press.
    WheelUp,
    /// Mouse wheel is going down.
    ///
    /// This event is typically only used with Mouse::Press.
    WheelDown,
}

/// A key.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// Backspace.
    Backspace,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Page Up key.
    PageUp,
    /// Page Down key.
    PageDown,
    /// Backward Tab key.
    BackTab,
    /// Delete key.
    Delete,
    /// Insert key.
    Insert,
    /// Function keys.
    ///
    /// Only function keys 1 through 12 are supported.
    F(u8),
    /// Normal character.
    Char(char),
    /// Alt modified character.
    Alt(char),
    /// Ctrl modified character.
    ///
    /// Note that certain keys may not be modifiable with `ctrl`, due to limitations of terminals.
    Ctrl(char),
    /// Null byte.
    Null,
    /// Esc key.
    Esc,

    #[doc(hidden)]
    __IsNotComplete,
}

/// Parse an Event from `item` and possibly subsequent bytes through `iter`.
pub fn parse_event<I>(item: u8, iter: &mut I) -> Result<Event, Error>
    where I: Iterator<Item = Result<u8, Error>>
{
    let error = Error::new(ErrorKind::Other, "Could not parse an event");
    match item {
        b'\x1B' => {
            // This is an escape character, leading a control sequence.
            Ok(match iter.next() {
                   Some(Ok(b'O')) => {
                match iter.next() {
                    // F1-F4
                    Some(Ok(val @ b'P'..=b'S')) => Event::Key(Key::F(1 + val - b'P')),
                    _ => return Err(error),
                }
            }
                   Some(Ok(b'[')) => {
                // This is a CSI sequence.
                parse_csi(iter).ok_or(error)?
            }
                   Some(Ok(c)) => {
                let ch = parse_utf8_char(c, iter)?;
                Event::Key(Key::Alt(ch))
            }
                   Some(Err(_)) | None => return Err(error),
               })
        }
        b'\n' | b'\r' => Ok(Event::Key(Key::Char('\n'))),
        b'\t' => Ok(Event::Key(Key::Char('\t'))),
        b'\x7F' => Ok(Event::Key(Key::Backspace)),
        c @ b'\x01'..=b'\x1A' => Ok(Event::Key(Key::Ctrl((c as u8 - 0x1 + b'a') as char))),
        c @ b'\x1C'..=b'\x1F' => Ok(Event::Key(Key::Ctrl((c as u8 - 0x1C + b'4') as char))),
        b'\0' => Ok(Event::Key(Key::Null)),
        c => {
            Ok({
                   let ch = parse_utf8_char(c, iter)?;
                   Event::Key(Key::Char(ch))
               })
        }
    }
}

/// Parses a CSI sequence, just after reading ^[
///
/// Returns None if an unrecognized sequence is found.
fn parse_csi<I>(iter: &mut I) -> Option<Event>
    where I: Iterator<Item = Result<u8, Error>>
{
    Some(match iter.next() {
             Some(Ok(b'[')) => match iter.next() {
                 Some(Ok(val @ b'A'..=b'E')) => Event::Key(Key::F(1 + val - b'A')),
                 _ => return None,
             },
             Some(Ok(b'D')) => Event::Key(Key::Left),
             Some(Ok(b'C')) => Event::Key(Key::Right),
             Some(Ok(b'A')) => Event::Key(Key::Up),
             Some(Ok(b'B')) => Event::Key(Key::Down),
             Some(Ok(b'H')) => Event::Key(Key::Home),
             Some(Ok(b'F')) => Event::Key(Key::End),
             Some(Ok(b'Z')) => Event::Key(Key::BackTab),
             Some(Ok(b'M')) => {
        // X10 emulation mouse encoding: ESC [ CB Cx Cy (6 characters only).
        let mut next = || iter.next().unwrap().unwrap();

        let cb = next() as i8 - 32;
        // (1, 1) are the coords for upper left.
        let cx = next().saturating_sub(32) as u16;
        let cy = next().saturating_sub(32) as u16;
        Event::Mouse(match cb & 0b11 {
                         0 => {
                             if cb & 0x40 != 0 {
                                 MouseEvent::Press(MouseButton::WheelUp, cx, cy)
                             } else {
                                 MouseEvent::Press(MouseButton::Left, cx, cy)
                             }
                         }
                         1 => {
                             if cb & 0x40 != 0 {
                                 MouseEvent::Press(MouseButton::WheelDown, cx, cy)
                             } else {
                                 MouseEvent::Press(MouseButton::Middle, cx, cy)
                             }
                         }
                         2 => MouseEvent::Press(MouseButton::Right, cx, cy),
                         3 => MouseEvent::Release(cx, cy),
                         _ => return None,
                     })
    }
             Some(Ok(b'<')) => {
        // xterm mouse encoding:
        // ESC [ < Cb ; Cx ; Cy (;) (M or m)
        let mut buf = Vec::new();
        let mut c = iter.next().unwrap().unwrap();
        while match c {
                  b'm' | b'M' => false,
                  _ => true,
              } {
            buf.push(c);
            c = iter.next().unwrap().unwrap();
        }
        let str_buf = String::from_utf8(buf).unwrap();
        let nums = &mut str_buf.split(';');

        let cb = nums.next()
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let cx = nums.next()
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let cy = nums.next()
            .unwrap()
            .parse::<u16>()
            .unwrap();

        let event = match cb {
            0..=2 | 64..=65 => {
                let button = match cb {
                    0 => MouseButton::Left,
                    1 => MouseButton::Middle,
                    2 => MouseButton::Right,
                    64 => MouseButton::WheelUp,
                    65 => MouseButton::WheelDown,
                    _ => unreachable!(),
                };
                match c {
                    b'M' => MouseEvent::Press(button, cx, cy),
                    b'm' => MouseEvent::Release(cx, cy),
                    _ => return None,
                }
            }
            32 => MouseEvent::Hold(cx, cy),
            3 => MouseEvent::Release(cx, cy),
            _ => return None,
        };

        Event::Mouse(event)
    }
             Some(Ok(c @ b'0'..=b'9')) => {
        // Numbered escape code.
        let mut buf = Vec::new();
        buf.push(c);
        let mut c = iter.next().unwrap().unwrap();
        // The final byte of a CSI sequence can be in the range 64-126, so
        // let's keep reading anything else.
        while c < 64 || c > 126 {
            buf.push(c);
            c = iter.next().unwrap().unwrap();
        }

        match c {
            // rxvt mouse encoding:
            // ESC [ Cb ; Cx ; Cy ; M
            b'M' => {
                let str_buf = String::from_utf8(buf).unwrap();

                let nums: Vec<u16> = str_buf.split(';').map(|n| n.parse().unwrap()).collect();

                let cb = nums[0];
                let cx = nums[1];
                let cy = nums[2];

                let event = match cb {
                    32 => MouseEvent::Press(MouseButton::Left, cx, cy),
                    33 => MouseEvent::Press(MouseButton::Middle, cx, cy),
                    34 => MouseEvent::Press(MouseButton::Right, cx, cy),
                    35 => MouseEvent::Release(cx, cy),
                    64 => MouseEvent::Hold(cx, cy),
                    96 | 97 => MouseEvent::Press(MouseButton::WheelUp, cx, cy),
                    _ => return None,
                };

                Event::Mouse(event)
            }
            // Special key code.
            b'~' => {
                let str_buf = String::from_utf8(buf).unwrap();

                // This CSI sequence can be a list of semicolon-separated
                // numbers.
                let nums: Vec<u8> = str_buf.split(';').map(|n| n.parse().unwrap()).collect();

                if nums.is_empty() {
                    return None;
                }

                // TODO: handle multiple values for key modififiers (ex: values
                // [3, 2] means Shift+Delete)
                if nums.len() > 1 {
                    return None;
                }

                match nums[0] {
                    1 | 7 => Event::Key(Key::Home),
                    2 => Event::Key(Key::Insert),
                    3 => Event::Key(Key::Delete),
                    4 | 8 => Event::Key(Key::End),
                    5 => Event::Key(Key::PageUp),
                    6 => Event::Key(Key::PageDown),
                    v @ 11..=15 => Event::Key(Key::F(v - 10)),
                    v @ 17..=21 => Event::Key(Key::F(v - 11)),
                    v @ 23..=24 => Event::Key(Key::F(v - 12)),
                    _ => return None,
                }
            }
            _ => return None,
        }
    }
             _ => return None,
         })

}

/// Parse `c` as either a single byte ASCII char or a variable size UTF-8 char.
fn parse_utf8_char<I>(c: u8, iter: &mut I) -> Result<char, Error>
    where I: Iterator<Item = Result<u8, Error>>
{
    let error = Err(Error::new(ErrorKind::Other, "Input character is not valid UTF-8"));
    if c.is_ascii() {
        Ok(c as char)
    } else {
        let bytes = &mut Vec::new();
        bytes.push(c);

        loop {
            match iter.next() {
                Some(Ok(next)) => {
                    bytes.push(next);
                    if let Ok(st) = str::from_utf8(bytes) {
                        return Ok(st.chars().next().unwrap());
                    }
                    if bytes.len() >= 4 {
                        return error;
                    }
                }
                _ => return error,
            }
        }
    }
}

#[cfg(test)]
#[test]
fn test_parse_utf8() {
    let st = "abcéŷ¤£€ù%323";
    let ref mut bytes = st.bytes().map(|x| Ok(x));
    let chars = st.chars();
    for c in chars {
        let b = bytes.next().unwrap().unwrap();
        assert!(c == parse_utf8_char(b, bytes).unwrap());
    }
}
