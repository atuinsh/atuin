//! Development-related functionality

pub use blobby;

/// Define test
#[macro_export]
#[cfg_attr(docsrs, doc(cfg(feature = "dev")))]
macro_rules! new_test {
    ($name:ident, $test_name:expr, $mac:ty) => {
        #[test]
        fn $name() {
            use crypto_mac::dev::blobby::Blob3Iterator;
            use crypto_mac::{Mac, NewMac};

            fn run_test(key: &[u8], input: &[u8], tag: &[u8]) -> Option<&'static str> {
                let mut mac = <$mac as NewMac>::new_varkey(key).unwrap();
                mac.update(input);
                let result = mac.finalize_reset();
                if &result.into_bytes()[..] != tag {
                    return Some("whole message");
                }
                // test if reset worked correctly
                mac.update(input);
                if mac.verify(&tag).is_err() {
                    return Some("after reset");
                }

                let mut mac = <$mac as NewMac>::new_varkey(key).unwrap();
                // test reading byte by byte
                for i in 0..input.len() {
                    mac.update(&input[i..i + 1]);
                }
                if let Err(_) = mac.verify(tag) {
                    return Some("message byte-by-byte");
                }
                None
            }

            let data = include_bytes!(concat!("data/", $test_name, ".blb"));

            for (i, row) in Blob3Iterator::new(data).unwrap().enumerate() {
                let [key, input, tag] = row.unwrap();
                if let Some(desc) = run_test(key, input, tag) {
                    panic!(
                        "\n\
                         Failed test №{}: {}\n\
                         key:\t{:?}\n\
                         input:\t{:?}\n\
                         tag:\t{:?}\n",
                        i, desc, key, input, tag,
                    );
                }
            }
        }
    };
}

/// Define benchmark
#[macro_export]
#[cfg_attr(docsrs, doc(cfg(feature = "dev")))]
macro_rules! bench {
    ($name:ident, $engine:path, $bs:expr) => {
        #[bench]
        fn $name(b: &mut Bencher) {
            let key = Default::default();
            let mut mac = <$engine>::new(&key);
            let data = [0; $bs];

            b.iter(|| {
                mac.update(&data);
            });

            b.bytes = $bs;
        }
    };

    ($engine:path) => {
        extern crate test;

        use crypto_mac::{Mac, NewMac};
        use test::Bencher;

        $crate::bench!(bench1_10, $engine, 10);
        $crate::bench!(bench2_100, $engine, 100);
        $crate::bench!(bench3_1000, $engine, 1000);
        $crate::bench!(bench3_10000, $engine, 10000);
    };
}
