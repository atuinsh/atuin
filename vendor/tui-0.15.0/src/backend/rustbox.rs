use crate::{
    backend::Backend,
    buffer::Cell,
    layout::Rect,
    style::{Color, Modifier},
};
use std::io;

pub struct RustboxBackend {
    rustbox: rustbox::RustBox,
}

impl RustboxBackend {
    pub fn new() -> Result<RustboxBackend, rustbox::InitError> {
        let rustbox = rustbox::RustBox::init(Default::default())?;
        Ok(RustboxBackend { rustbox })
    }

    pub fn with_rustbox(instance: rustbox::RustBox) -> RustboxBackend {
        RustboxBackend { rustbox: instance }
    }

    pub fn rustbox(&self) -> &rustbox::RustBox {
        &self.rustbox
    }
}

impl Backend for RustboxBackend {
    fn draw<'a, I>(&mut self, content: I) -> Result<(), io::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, cell) in content {
            self.rustbox.print(
                x as usize,
                y as usize,
                cell.modifier.into(),
                cell.fg.into(),
                cell.bg.into(),
                &cell.symbol,
            );
        }
        Ok(())
    }
    fn hide_cursor(&mut self) -> Result<(), io::Error> {
        Ok(())
    }
    fn show_cursor(&mut self) -> Result<(), io::Error> {
        Ok(())
    }
    fn get_cursor(&mut self) -> io::Result<(u16, u16)> {
        Err(io::Error::from(io::ErrorKind::Other))
    }
    fn set_cursor(&mut self, x: u16, y: u16) -> io::Result<()> {
        self.rustbox.set_cursor(x as isize, y as isize);
        Ok(())
    }
    fn clear(&mut self) -> Result<(), io::Error> {
        self.rustbox.clear();
        Ok(())
    }
    fn size(&self) -> Result<Rect, io::Error> {
        let term_width = self.rustbox.width();
        let term_height = self.rustbox.height();
        let max = u16::max_value();
        Ok(Rect::new(
            0,
            0,
            if term_width > usize::from(max) {
                max
            } else {
                term_width as u16
            },
            if term_height > usize::from(max) {
                max
            } else {
                term_height as u16
            },
        ))
    }
    fn flush(&mut self) -> Result<(), io::Error> {
        self.rustbox.present();
        Ok(())
    }
}

fn rgb_to_byte(r: u8, g: u8, b: u8) -> u16 {
    u16::from((r & 0xC0) + ((g & 0xE0) >> 2) + ((b & 0xE0) >> 5))
}

impl Into<rustbox::Color> for Color {
    fn into(self) -> rustbox::Color {
        match self {
            Color::Reset => rustbox::Color::Default,
            Color::Black | Color::Gray | Color::DarkGray => rustbox::Color::Black,
            Color::Red | Color::LightRed => rustbox::Color::Red,
            Color::Green | Color::LightGreen => rustbox::Color::Green,
            Color::Yellow | Color::LightYellow => rustbox::Color::Yellow,
            Color::Magenta | Color::LightMagenta => rustbox::Color::Magenta,
            Color::Cyan | Color::LightCyan => rustbox::Color::Cyan,
            Color::White => rustbox::Color::White,
            Color::Blue | Color::LightBlue => rustbox::Color::Blue,
            Color::Indexed(i) => rustbox::Color::Byte(u16::from(i)),
            Color::Rgb(r, g, b) => rustbox::Color::Byte(rgb_to_byte(r, g, b)),
        }
    }
}

impl Into<rustbox::Style> for Modifier {
    fn into(self) -> rustbox::Style {
        let mut result = rustbox::Style::empty();
        if self.contains(Modifier::BOLD) {
            result.insert(rustbox::RB_BOLD);
        }
        if self.contains(Modifier::UNDERLINED) {
            result.insert(rustbox::RB_UNDERLINE);
        }
        if self.contains(Modifier::REVERSED) {
            result.insert(rustbox::RB_REVERSE);
        }
        result
    }
}
