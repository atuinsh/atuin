extern crate termion;

use termion::{color, style};

fn main() {
    println!("{lighgreen}-- src/test/ui/borrow-errors.rs at 82:18 --\n\
              {red}error: {reset}{bold}two closures require unique access to `vec` at the same time {reset}{bold}{magenta}[E0524]{reset}\n\
              {line_num_fg}{line_num_bg}79 {reset}     let append = |e| {{\n\
              {line_num_fg}{line_num_bg}{info_line}{reset}                  {red}^^^{reset} {error_fg}first closure is constructed here\n\
              {line_num_fg}{line_num_bg}80 {reset}         vec.push(e)\n\
              {line_num_fg}{line_num_bg}{info_line}{reset}                 {red}^^^{reset} {error_fg}previous borrow occurs due to use of `vec` in closure\n\
              {line_num_fg}{line_num_bg}84 {reset}     }};\n\
              {line_num_fg}{line_num_bg}85 {reset} }}\n\
              {line_num_fg}{line_num_bg}{info_line}{reset} {red}^{reset} {error_fg}borrow from first closure ends here",
             lighgreen = color::Fg(color::LightGreen),
             red = color::Fg(color::Red),
             bold = style::Bold,
             reset = style::Reset,
             magenta = color::Fg(color::Magenta),
             line_num_bg = color::Bg(color::AnsiValue::grayscale(3)),
             line_num_fg = color::Fg(color::AnsiValue::grayscale(18)),
             info_line = "|  ",
             error_fg = color::Fg(color::AnsiValue::grayscale(17)))
}
