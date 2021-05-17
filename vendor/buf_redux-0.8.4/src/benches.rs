// Copyright 2016 Austin Bonander <austin.bonander@gmail.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
extern crate test;

mod construction {
    use super::test;

    use {BufWriter, BufReader};

    use std::io;

    #[bench]
    fn bufreader(b: &mut test::Bencher) {
        b.iter(|| {
            BufReader::new(io::empty())
        });
    }

    #[bench]
    fn std_bufreader(b: &mut test::Bencher) {
        b.iter(|| {
            io::BufReader::new(io::empty())
        });
    }

    #[bench]
    fn bufwriter(b: &mut test::Bencher) {
        b.iter(|| {
            BufWriter::new(io::sink())
        });
    }

    #[bench]
    fn std_bufwriter(b: &mut test::Bencher) {
        b.iter(|| {
            io::BufWriter::new(io::sink())
        });
    }
}



