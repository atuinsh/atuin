use std::io;

use crate::buffer::Cell;
use crate::layout::Rect;

#[cfg(feature = "rustbox")]
mod rustbox;
#[cfg(feature = "rustbox")]
pub use self::rustbox::RustboxBackend;

#[cfg(feature = "termion")]
mod termion;
#[cfg(feature = "termion")]
pub use self::termion::TermionBackend;

#[cfg(feature = "crossterm")]
mod crossterm;
#[cfg(feature = "crossterm")]
pub use self::crossterm::CrosstermBackend;

#[cfg(feature = "curses")]
mod curses;
#[cfg(feature = "curses")]
pub use self::curses::CursesBackend;

mod test;
pub use self::test::TestBackend;

pub trait Backend {
    fn draw<'a, I>(&mut self, content: I) -> Result<(), io::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>;
    fn hide_cursor(&mut self) -> Result<(), io::Error>;
    fn show_cursor(&mut self) -> Result<(), io::Error>;
    fn get_cursor(&mut self) -> Result<(u16, u16), io::Error>;
    fn set_cursor(&mut self, x: u16, y: u16) -> Result<(), io::Error>;
    fn clear(&mut self) -> Result<(), io::Error>;
    fn size(&self) -> Result<Rect, io::Error>;
    fn flush(&mut self) -> Result<(), io::Error>;
}
