extern crate unicode_segmentation;

use std::error::Error;
use std::io::{self, BufRead, Write};

use unicode_segmentation::UnicodeSegmentation;

fn main() -> Result<(), Box<dyn Error>> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut stdout = io::BufWriter::new(io::stdout());

    let mut line = String::new();
    while stdin.read_line(&mut line)? > 0 {
        let end = line
            .grapheme_indices(true)
            .map(|(start, g)| start + g.len())
            .take(10)
            .last()
            .unwrap_or(line.len());
        #[allow(deprecated)] // for Rust 1.28.0
        stdout.write_all(line[..end].trim_right().as_bytes())?;
        stdout.write_all(b"\n")?;

        line.clear();
    }
    Ok(())
}
