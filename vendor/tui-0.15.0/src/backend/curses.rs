use std::io;

use crate::backend::Backend;
use crate::buffer::Cell;
use crate::layout::Rect;
use crate::style::{Color, Modifier};
use crate::symbols::{bar, block};
#[cfg(unix)]
use crate::symbols::{line, DOT};
#[cfg(unix)]
use pancurses::{chtype, ToChtype};
use unicode_segmentation::UnicodeSegmentation;

pub struct CursesBackend {
    curses: easycurses::EasyCurses,
}

impl CursesBackend {
    pub fn new() -> Option<CursesBackend> {
        let curses = easycurses::EasyCurses::initialize_system()?;
        Some(CursesBackend { curses })
    }

    pub fn with_curses(curses: easycurses::EasyCurses) -> CursesBackend {
        CursesBackend { curses }
    }

    pub fn get_curses(&self) -> &easycurses::EasyCurses {
        &self.curses
    }

    pub fn get_curses_mut(&mut self) -> &mut easycurses::EasyCurses {
        &mut self.curses
    }
}

impl Backend for CursesBackend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut last_col = 0;
        let mut last_row = 0;
        let mut fg = Color::Reset;
        let mut bg = Color::Reset;
        let mut modifier = Modifier::empty();
        let mut curses_style = CursesStyle {
            fg: easycurses::Color::White,
            bg: easycurses::Color::Black,
        };
        let mut update_color = false;
        for (col, row, cell) in content {
            if row != last_row || col != last_col + 1 {
                self.curses.move_rc(i32::from(row), i32::from(col));
            }
            last_col = col;
            last_row = row;
            if cell.modifier != modifier {
                apply_modifier_diff(&mut self.curses.win, modifier, cell.modifier);
                modifier = cell.modifier;
            };
            if cell.fg != fg {
                update_color = true;
                if let Some(ccolor) = cell.fg.into() {
                    fg = cell.fg;
                    curses_style.fg = ccolor;
                } else {
                    fg = Color::White;
                    curses_style.fg = easycurses::Color::White;
                }
            };
            if cell.bg != bg {
                update_color = true;
                if let Some(ccolor) = cell.bg.into() {
                    bg = cell.bg;
                    curses_style.bg = ccolor;
                } else {
                    bg = Color::Black;
                    curses_style.bg = easycurses::Color::Black;
                }
            };
            if update_color {
                self.curses
                    .set_color_pair(easycurses::ColorPair::new(curses_style.fg, curses_style.bg));
            };
            update_color = false;
            draw(&mut self.curses, cell.symbol.as_str());
        }
        self.curses.win.attrset(pancurses::Attribute::Normal);
        self.curses.set_color_pair(easycurses::ColorPair::new(
            easycurses::Color::White,
            easycurses::Color::Black,
        ));
        Ok(())
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.curses
            .set_cursor_visibility(easycurses::CursorVisibility::Invisible);
        Ok(())
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.curses
            .set_cursor_visibility(easycurses::CursorVisibility::Visible);
        Ok(())
    }
    fn get_cursor(&mut self) -> io::Result<(u16, u16)> {
        let (y, x) = self.curses.get_cursor_rc();
        Ok((x as u16, y as u16))
    }
    fn set_cursor(&mut self, x: u16, y: u16) -> io::Result<()> {
        self.curses.move_rc(i32::from(y), i32::from(x));
        Ok(())
    }
    fn clear(&mut self) -> io::Result<()> {
        self.curses.clear();
        // self.curses.refresh();
        Ok(())
    }
    fn size(&self) -> Result<Rect, io::Error> {
        let (nrows, ncols) = self.curses.get_row_col_count();
        Ok(Rect::new(0, 0, ncols as u16, nrows as u16))
    }
    fn flush(&mut self) -> io::Result<()> {
        self.curses.refresh();
        Ok(())
    }
}

struct CursesStyle {
    fg: easycurses::Color,
    bg: easycurses::Color,
}

#[cfg(unix)]
/// Deals with lack of unicode support for ncurses on unix
fn draw(curses: &mut easycurses::EasyCurses, symbol: &str) {
    for grapheme in symbol.graphemes(true) {
        let ch = match grapheme {
            line::TOP_RIGHT => pancurses::ACS_URCORNER(),
            line::VERTICAL => pancurses::ACS_VLINE(),
            line::HORIZONTAL => pancurses::ACS_HLINE(),
            line::TOP_LEFT => pancurses::ACS_ULCORNER(),
            line::BOTTOM_RIGHT => pancurses::ACS_LRCORNER(),
            line::BOTTOM_LEFT => pancurses::ACS_LLCORNER(),
            line::VERTICAL_LEFT => pancurses::ACS_RTEE(),
            line::VERTICAL_RIGHT => pancurses::ACS_LTEE(),
            line::HORIZONTAL_DOWN => pancurses::ACS_TTEE(),
            line::HORIZONTAL_UP => pancurses::ACS_BTEE(),
            block::FULL => pancurses::ACS_BLOCK(),
            block::SEVEN_EIGHTHS => pancurses::ACS_BLOCK(),
            block::THREE_QUARTERS => pancurses::ACS_BLOCK(),
            block::FIVE_EIGHTHS => pancurses::ACS_BLOCK(),
            block::HALF => pancurses::ACS_BLOCK(),
            block::THREE_EIGHTHS => ' ' as chtype,
            block::ONE_QUARTER => ' ' as chtype,
            block::ONE_EIGHTH => ' ' as chtype,
            bar::SEVEN_EIGHTHS => pancurses::ACS_BLOCK(),
            bar::THREE_QUARTERS => pancurses::ACS_BLOCK(),
            bar::FIVE_EIGHTHS => pancurses::ACS_BLOCK(),
            bar::HALF => pancurses::ACS_BLOCK(),
            bar::THREE_EIGHTHS => pancurses::ACS_S9(),
            bar::ONE_QUARTER => pancurses::ACS_S9(),
            bar::ONE_EIGHTH => pancurses::ACS_S9(),
            DOT => pancurses::ACS_BULLET(),
            unicode_char => {
                if unicode_char.is_ascii() {
                    let mut chars = unicode_char.chars();
                    if let Some(ch) = chars.next() {
                        ch.to_chtype()
                    } else {
                        pancurses::ACS_BLOCK()
                    }
                } else {
                    pancurses::ACS_BLOCK()
                }
            }
        };
        curses.win.addch(ch);
    }
}

#[cfg(windows)]
fn draw(curses: &mut easycurses::EasyCurses, symbol: &str) {
    for grapheme in symbol.graphemes(true) {
        let ch = match grapheme {
            block::SEVEN_EIGHTHS => block::FULL,
            block::THREE_QUARTERS => block::FULL,
            block::FIVE_EIGHTHS => block::HALF,
            block::THREE_EIGHTHS => block::HALF,
            block::ONE_QUARTER => block::HALF,
            block::ONE_EIGHTH => " ",
            bar::SEVEN_EIGHTHS => bar::FULL,
            bar::THREE_QUARTERS => bar::FULL,
            bar::FIVE_EIGHTHS => bar::HALF,
            bar::THREE_EIGHTHS => bar::HALF,
            bar::ONE_QUARTER => bar::HALF,
            bar::ONE_EIGHTH => " ",
            ch => ch,
        };
        // curses.win.addch(ch);
        curses.print(ch);
    }
}

impl From<Color> for Option<easycurses::Color> {
    fn from(color: Color) -> Option<easycurses::Color> {
        match color {
            Color::Reset => None,
            Color::Black => Some(easycurses::Color::Black),
            Color::Red | Color::LightRed => Some(easycurses::Color::Red),
            Color::Green | Color::LightGreen => Some(easycurses::Color::Green),
            Color::Yellow | Color::LightYellow => Some(easycurses::Color::Yellow),
            Color::Magenta | Color::LightMagenta => Some(easycurses::Color::Magenta),
            Color::Cyan | Color::LightCyan => Some(easycurses::Color::Cyan),
            Color::White | Color::Gray | Color::DarkGray => Some(easycurses::Color::White),
            Color::Blue | Color::LightBlue => Some(easycurses::Color::Blue),
            Color::Indexed(_) => None,
            Color::Rgb(_, _, _) => None,
        }
    }
}

fn apply_modifier_diff(win: &mut pancurses::Window, from: Modifier, to: Modifier) {
    remove_modifier(win, from - to);
    add_modifier(win, to - from);
}

fn remove_modifier(win: &mut pancurses::Window, remove: Modifier) {
    if remove.contains(Modifier::BOLD) {
        win.attroff(pancurses::Attribute::Bold);
    }
    if remove.contains(Modifier::DIM) {
        win.attroff(pancurses::Attribute::Dim);
    }
    if remove.contains(Modifier::ITALIC) {
        win.attroff(pancurses::Attribute::Italic);
    }
    if remove.contains(Modifier::UNDERLINED) {
        win.attroff(pancurses::Attribute::Underline);
    }
    if remove.contains(Modifier::SLOW_BLINK) || remove.contains(Modifier::RAPID_BLINK) {
        win.attroff(pancurses::Attribute::Blink);
    }
    if remove.contains(Modifier::REVERSED) {
        win.attroff(pancurses::Attribute::Reverse);
    }
    if remove.contains(Modifier::HIDDEN) {
        win.attroff(pancurses::Attribute::Invisible);
    }
    if remove.contains(Modifier::CROSSED_OUT) {
        win.attroff(pancurses::Attribute::Strikeout);
    }
}

fn add_modifier(win: &mut pancurses::Window, add: Modifier) {
    if add.contains(Modifier::BOLD) {
        win.attron(pancurses::Attribute::Bold);
    }
    if add.contains(Modifier::DIM) {
        win.attron(pancurses::Attribute::Dim);
    }
    if add.contains(Modifier::ITALIC) {
        win.attron(pancurses::Attribute::Italic);
    }
    if add.contains(Modifier::UNDERLINED) {
        win.attron(pancurses::Attribute::Underline);
    }
    if add.contains(Modifier::SLOW_BLINK) || add.contains(Modifier::RAPID_BLINK) {
        win.attron(pancurses::Attribute::Blink);
    }
    if add.contains(Modifier::REVERSED) {
        win.attron(pancurses::Attribute::Reverse);
    }
    if add.contains(Modifier::HIDDEN) {
        win.attron(pancurses::Attribute::Invisible);
    }
    if add.contains(Modifier::CROSSED_OUT) {
        win.attron(pancurses::Attribute::Strikeout);
    }
}
