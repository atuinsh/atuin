#![cfg_attr(feature = "cargo-clippy", allow(cast_lossless))]
#![feature(test)]
#![allow(non_snake_case)]

extern crate itoa;
extern crate test;

macro_rules! benches {
    (
        $(
            $(#[$attr:meta])*
            $name:ident($value:expr)
        ),*
    ) => {
        mod bench_itoa_write {
            use test::{Bencher, black_box};
            $(
                $(#[$attr])*
                #[bench]
                fn $name(b: &mut Bencher) {
                    use itoa;

                    let mut buf = Vec::with_capacity(40);

                    b.iter(|| {
                        buf.clear();
                        itoa::write(&mut buf, black_box($value)).unwrap()
                    });
                }
            )*
        }

        mod bench_itoa_fmt {
            use test::{Bencher, black_box};
            $(
                $(#[$attr])*
                #[bench]
                fn $name(b: &mut Bencher) {
                    use itoa;

                    let mut buf = String::with_capacity(40);

                    b.iter(|| {
                        buf.clear();
                        itoa::fmt(&mut buf, black_box($value)).unwrap()
                    });
                }
            )*
        }

        mod bench_std_fmt {
            use test::{Bencher, black_box};
            $(
                $(#[$attr])*
                #[bench]
                fn $name(b: &mut Bencher) {
                    use std::io::Write;

                    let mut buf = Vec::with_capacity(40);

                    b.iter(|| {
                        buf.clear();
                        write!(&mut buf, "{}", black_box($value)).unwrap()
                    });
                }
            )*
        }
    }
}

benches! {
    bench_u64_0(0u64),
    bench_u64_half(<u32>::max_value() as u64),
    bench_u64_max(<u64>::max_value()),

    bench_i16_0(0i16),
    bench_i16_min(<i16>::min_value()),

    #[cfg(feature = "i128")]
    bench_u128_0(0u128),
    #[cfg(feature = "i128")]
    bench_u128_max(<u128>::max_value())
}
