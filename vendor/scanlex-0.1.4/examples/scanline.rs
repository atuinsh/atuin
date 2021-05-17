// ScanLines is a struct that is used to 'iterate' over a Scanner
// for each line in a readable source. It cannot (currently) be
// an actual iterator because of lifetime constraints, because
// it returns a Scanner that borrows a string from the struct. This
// however makes it more efficient.
//
// This example prints out the first token of each line in this file
extern crate scanlex;
use scanlex::ScanLines;
use std::fs::File;

fn main() {
    let f = File::open("scanline.rs").expect("cannot open scanline.rs");
    let mut iter = ScanLines::new(&f);
    while let Some(s) = iter.next() {
        let mut s = s.expect("cannot read line");
        println!("{:?}",s.get());
    }
}
