//! User input.

use std::io::{self, Read, Write};
use std::ops;

use event::{self, Event, Key};
use raw::IntoRawMode;

/// An iterator over input keys.
pub struct Keys<R> {
    iter: Events<R>,
}

impl<R: Read> Iterator for Keys<R> {
    type Item = Result<Key, io::Error>;

    fn next(&mut self) -> Option<Result<Key, io::Error>> {
        loop {
            match self.iter.next() {
                Some(Ok(Event::Key(k))) => return Some(Ok(k)),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Some(Err(e)),
                None => return None,
            };
        }
    }
}

/// An iterator over input events.
pub struct Events<R>  {
    inner: EventsAndRaw<R>
}

impl<R: Read> Iterator for Events<R> {
    type Item = Result<Event, io::Error>;

    fn next(&mut self) -> Option<Result<Event, io::Error>> {
        self.inner.next().map(|tuple| tuple.map(|(event, _raw)| event))
    }
}

/// An iterator over input events and the bytes that define them.
pub struct EventsAndRaw<R> {
    source: R,
    leftover: Option<u8>,
}

impl<R: Read> Iterator for EventsAndRaw<R> {
    type Item = Result<(Event, Vec<u8>), io::Error>;

    fn next(&mut self) -> Option<Result<(Event, Vec<u8>), io::Error>> {
        let source = &mut self.source;

        if let Some(c) = self.leftover {
            // we have a leftover byte, use it
            self.leftover = None;
            return Some(parse_event(c, &mut source.bytes()));
        }

        // Here we read two bytes at a time. We need to distinguish between single ESC key presses,
        // and escape sequences (which start with ESC or a x1B byte). The idea is that if this is
        // an escape sequence, we will read multiple bytes (the first byte being ESC) but if this
        // is a single ESC keypress, we will only read a single byte.
        let mut buf = [0u8; 2];
        let res = match source.read(&mut buf) {
            Ok(0) => return None,
            Ok(1) => {
                match buf[0] {
                    b'\x1B' => Ok((Event::Key(Key::Esc), vec![b'\x1B'])),
                    c => parse_event(c, &mut source.bytes()),
                }
            }
            Ok(2) => {
                let option_iter = &mut Some(buf[1]).into_iter();
                let result = {
                    let mut iter = option_iter.map(|c| Ok(c)).chain(source.bytes());
                    parse_event(buf[0], &mut iter)
                };
                // If the option_iter wasn't consumed, keep the byte for later.
                self.leftover = option_iter.next();
                result
            }
            Ok(_) => unreachable!(),
            Err(e) => Err(e),
        };

        Some(res)
    }
}

fn parse_event<I>(item: u8, iter: &mut I) -> Result<(Event, Vec<u8>), io::Error>
    where I: Iterator<Item = Result<u8, io::Error>>
{
    let mut buf = vec![item];
    let result = {
        let mut iter = iter.inspect(|byte| if let &Ok(byte) = byte {
                                        buf.push(byte);
                                    });
        event::parse_event(item, &mut iter)
    };
    result.or(Ok(Event::Unsupported(buf.clone()))).map(|e| (e, buf))
}


/// Extension to `Read` trait.
pub trait TermRead {
    /// An iterator over input events.
    fn events(self) -> Events<Self> where Self: Sized;

    /// An iterator over key inputs.
    fn keys(self) -> Keys<Self> where Self: Sized;

    /// Read a line.
    ///
    /// EOT and ETX will abort the prompt, returning `None`. Newline or carriage return will
    /// complete the input.
    fn read_line(&mut self) -> io::Result<Option<String>>;

    /// Read a password.
    ///
    /// EOT and ETX will abort the prompt, returning `None`. Newline or carriage return will
    /// complete the input.
    fn read_passwd<W: Write>(&mut self, writer: &mut W) -> io::Result<Option<String>> {
        let _raw = writer.into_raw_mode()?;
        self.read_line()
    }
}


impl<R: Read + TermReadEventsAndRaw> TermRead for R {
    fn events(self) -> Events<Self> {
        Events {
            inner: self.events_and_raw()
        }
    }
    fn keys(self) -> Keys<Self> {
        Keys { iter: self.events() }
    }

    fn read_line(&mut self) -> io::Result<Option<String>> {
        let mut buf = Vec::with_capacity(30);

        for c in self.bytes() {
            match c {
                Err(e) => return Err(e),
                Ok(0) | Ok(3) | Ok(4) => return Ok(None),
                Ok(0x7f) => {
                    buf.pop();
                }
                Ok(b'\n') | Ok(b'\r') => break,
                Ok(c) => buf.push(c),
            }
        }

        let string = String::from_utf8(buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(string))
    }
}

/// Extension to `TermRead` trait. A separate trait in order to maintain backwards compatibility.
pub trait TermReadEventsAndRaw {
    /// An iterator over input events and the bytes that define them.
    fn events_and_raw(self) -> EventsAndRaw<Self> where Self: Sized;
}

impl<R: Read> TermReadEventsAndRaw for R {
    fn events_and_raw(self) -> EventsAndRaw<Self> {
        EventsAndRaw {
            source: self,
            leftover: None,
        }
    }
}

/// A sequence of escape codes to enable terminal mouse support.
const ENTER_MOUSE_SEQUENCE: &'static str = csi!("?1000h\x1b[?1002h\x1b[?1015h\x1b[?1006h");

/// A sequence of escape codes to disable terminal mouse support.
const EXIT_MOUSE_SEQUENCE: &'static str = csi!("?1006l\x1b[?1015l\x1b[?1002l\x1b[?1000l");

/// A terminal with added mouse support.
///
/// This can be obtained through the `From` implementations.
pub struct MouseTerminal<W: Write> {
    term: W,
}

impl<W: Write> From<W> for MouseTerminal<W> {
    fn from(mut from: W) -> MouseTerminal<W> {
        from.write_all(ENTER_MOUSE_SEQUENCE.as_bytes()).unwrap();

        MouseTerminal { term: from }
    }
}

impl<W: Write> Drop for MouseTerminal<W> {
    fn drop(&mut self) {
        self.term.write_all(EXIT_MOUSE_SEQUENCE.as_bytes()).unwrap();
    }
}

impl<W: Write> ops::Deref for MouseTerminal<W> {
    type Target = W;

    fn deref(&self) -> &W {
        &self.term
    }
}

impl<W: Write> ops::DerefMut for MouseTerminal<W> {
    fn deref_mut(&mut self) -> &mut W {
        &mut self.term
    }
}

impl<W: Write> Write for MouseTerminal<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.term.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.term.flush()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::io;
    use event::{Key, Event, MouseEvent, MouseButton};

    #[test]
    fn test_keys() {
        let mut i = b"\x1Bayo\x7F\x1B[D".keys();

        assert_eq!(i.next().unwrap().unwrap(), Key::Alt('a'));
        assert_eq!(i.next().unwrap().unwrap(), Key::Char('y'));
        assert_eq!(i.next().unwrap().unwrap(), Key::Char('o'));
        assert_eq!(i.next().unwrap().unwrap(), Key::Backspace);
        assert_eq!(i.next().unwrap().unwrap(), Key::Left);
        assert!(i.next().is_none());
    }

    #[test]
    fn test_events() {
        let mut i =
            b"\x1B[\x00bc\x7F\x1B[D\
                    \x1B[M\x00\x22\x24\x1B[<0;2;4;M\x1B[32;2;4M\x1B[<0;2;4;m\x1B[35;2;4Mb"
                    .events();

        assert_eq!(i.next().unwrap().unwrap(),
                   Event::Unsupported(vec![0x1B, b'[', 0x00]));
        assert_eq!(i.next().unwrap().unwrap(), Event::Key(Key::Char('b')));
        assert_eq!(i.next().unwrap().unwrap(), Event::Key(Key::Char('c')));
        assert_eq!(i.next().unwrap().unwrap(), Event::Key(Key::Backspace));
        assert_eq!(i.next().unwrap().unwrap(), Event::Key(Key::Left));
        assert_eq!(i.next().unwrap().unwrap(),
                   Event::Mouse(MouseEvent::Press(MouseButton::WheelUp, 2, 4)));
        assert_eq!(i.next().unwrap().unwrap(),
                   Event::Mouse(MouseEvent::Press(MouseButton::Left, 2, 4)));
        assert_eq!(i.next().unwrap().unwrap(),
                   Event::Mouse(MouseEvent::Press(MouseButton::Left, 2, 4)));
        assert_eq!(i.next().unwrap().unwrap(),
                   Event::Mouse(MouseEvent::Release(2, 4)));
        assert_eq!(i.next().unwrap().unwrap(),
                   Event::Mouse(MouseEvent::Release(2, 4)));
        assert_eq!(i.next().unwrap().unwrap(), Event::Key(Key::Char('b')));
        assert!(i.next().is_none());
    }

    #[test]
    fn test_events_and_raw() {
        let input = b"\x1B[\x00bc\x7F\x1B[D\
                    \x1B[M\x00\x22\x24\x1B[<0;2;4;M\x1B[32;2;4M\x1B[<0;2;4;m\x1B[35;2;4Mb";
        let mut output = Vec::<u8>::new();
        {
            let mut i = input.events_and_raw().map(|res| res.unwrap())
                .inspect(|&(_, ref raw)| { output.extend(raw); }).map(|(event, _)| event);

            assert_eq!(i.next().unwrap(),
            Event::Unsupported(vec![0x1B, b'[', 0x00]));
            assert_eq!(i.next().unwrap(), Event::Key(Key::Char('b')));
            assert_eq!(i.next().unwrap(), Event::Key(Key::Char('c')));
            assert_eq!(i.next().unwrap(), Event::Key(Key::Backspace));
            assert_eq!(i.next().unwrap(), Event::Key(Key::Left));
            assert_eq!(i.next().unwrap(),
            Event::Mouse(MouseEvent::Press(MouseButton::WheelUp, 2, 4)));
            assert_eq!(i.next().unwrap(),
            Event::Mouse(MouseEvent::Press(MouseButton::Left, 2, 4)));
            assert_eq!(i.next().unwrap(),
            Event::Mouse(MouseEvent::Press(MouseButton::Left, 2, 4)));
            assert_eq!(i.next().unwrap(),
            Event::Mouse(MouseEvent::Release(2, 4)));
            assert_eq!(i.next().unwrap(),
            Event::Mouse(MouseEvent::Release(2, 4)));
            assert_eq!(i.next().unwrap(), Event::Key(Key::Char('b')));
            assert!(i.next().is_none());
        }

        assert_eq!(input.iter().map(|b| *b).collect::<Vec<u8>>(), output)
    }

    #[test]
    fn test_function_keys() {
        let mut st = b"\x1BOP\x1BOQ\x1BOR\x1BOS".keys();
        for i in 1..5 {
            assert_eq!(st.next().unwrap().unwrap(), Key::F(i));
        }

        let mut st = b"\x1B[11~\x1B[12~\x1B[13~\x1B[14~\x1B[15~\
        \x1B[17~\x1B[18~\x1B[19~\x1B[20~\x1B[21~\x1B[23~\x1B[24~"
                .keys();
        for i in 1..13 {
            assert_eq!(st.next().unwrap().unwrap(), Key::F(i));
        }
    }

    #[test]
    fn test_special_keys() {
        let mut st = b"\x1B[2~\x1B[H\x1B[7~\x1B[5~\x1B[3~\x1B[F\x1B[8~\x1B[6~".keys();
        assert_eq!(st.next().unwrap().unwrap(), Key::Insert);
        assert_eq!(st.next().unwrap().unwrap(), Key::Home);
        assert_eq!(st.next().unwrap().unwrap(), Key::Home);
        assert_eq!(st.next().unwrap().unwrap(), Key::PageUp);
        assert_eq!(st.next().unwrap().unwrap(), Key::Delete);
        assert_eq!(st.next().unwrap().unwrap(), Key::End);
        assert_eq!(st.next().unwrap().unwrap(), Key::End);
        assert_eq!(st.next().unwrap().unwrap(), Key::PageDown);
        assert!(st.next().is_none());
    }

    #[test]
    fn test_esc_key() {
        let mut st = b"\x1B".keys();
        assert_eq!(st.next().unwrap().unwrap(), Key::Esc);
        assert!(st.next().is_none());
    }

    fn line_match(a: &str, b: Option<&str>) {
        let mut sink = io::sink();

        let line = a.as_bytes().read_line().unwrap();
        let pass = a.as_bytes().read_passwd(&mut sink).unwrap();

        // godammit rustc

        assert_eq!(line, pass);

        if let Some(l) = line {
            assert_eq!(Some(l.as_str()), b);
        } else {
            assert!(b.is_none());
        }
    }

    #[test]
    fn test_read() {
        let test1 = "this is the first test";
        let test2 = "this is the second test";

        line_match(test1, Some(test1));
        line_match(test2, Some(test2));
    }

    #[test]
    fn test_backspace() {
        line_match("this is the\x7f first\x7f\x7f test",
                   Some("this is th fir test"));
        line_match("this is the seco\x7fnd test\x7f",
                   Some("this is the secnd tes"));
    }

    #[test]
    fn test_end() {
        line_match("abc\nhttps://www.youtube.com/watch?v=dQw4w9WgXcQ",
                   Some("abc"));
        line_match("hello\rhttps://www.youtube.com/watch?v=yPYZpwSpKmA",
                   Some("hello"));
    }

    #[test]
    fn test_abort() {
        line_match("abc\x03https://www.youtube.com/watch?v=dQw4w9WgXcQ", None);
        line_match("hello\x04https://www.youtube.com/watch?v=yPYZpwSpKmA", None);
    }

}
