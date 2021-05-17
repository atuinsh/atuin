use std::fmt::Write;

use md5::{Digest, Md5};

use crate::io::{BufMutExt, Encode};
use crate::postgres::io::PgBufMutExt;

#[derive(Debug)]
pub enum Password<'a> {
    Cleartext(&'a str),

    Md5 {
        password: &'a str,
        username: &'a str,
        salt: [u8; 4],
    },
}

impl Password<'_> {
    #[inline]
    fn len(&self) -> usize {
        match self {
            Password::Cleartext(s) => s.len() + 5,
            Password::Md5 { .. } => 35 + 5,
        }
    }
}

impl Encode<'_> for Password<'_> {
    fn encode_with(&self, buf: &mut Vec<u8>, _: ()) {
        buf.reserve(1 + 4 + self.len());
        buf.push(b'p');

        buf.put_length_prefixed(|buf| {
            match self {
                Password::Cleartext(password) => {
                    buf.put_str_nul(password);
                }

                Password::Md5 {
                    username,
                    password,
                    salt,
                } => {
                    // The actual `PasswordMessage` can be computed in SQL as
                    // `concat('md5', md5(concat(md5(concat(password, username)), random-salt)))`.

                    // Keep in mind the md5() function returns its result as a hex string.

                    let mut hasher = Md5::new();

                    hasher.update(password);
                    hasher.update(username);

                    let mut output = String::with_capacity(35);

                    let _ = write!(output, "{:x}", hasher.finalize_reset());

                    hasher.update(&output);
                    hasher.update(salt);

                    output.clear();

                    let _ = write!(output, "md5{:x}", hasher.finalize());

                    buf.put_str_nul(&output);
                }
            }
        });
    }
}

#[test]
fn test_encode_clear_password() {
    const EXPECTED: &[u8] = b"p\0\0\0\rpassword\0";

    let mut buf = Vec::new();
    let m = Password::Cleartext("password");

    m.encode(&mut buf);

    assert_eq!(buf, EXPECTED);
}

#[test]
fn test_encode_md5_password() {
    const EXPECTED: &[u8] = b"p\0\0\0(md53e2c9d99d49b201ef867a36f3f9ed62c\0";

    let mut buf = Vec::new();
    let m = Password::Md5 {
        password: "password",
        username: "root",
        salt: [147, 24, 57, 152],
    };

    m.encode(&mut buf);

    assert_eq!(buf, EXPECTED);
}

#[cfg(all(test, not(debug_assertions)))]
#[bench]
fn bench_encode_clear_password(b: &mut test::Bencher) {
    use test::black_box;

    let mut buf = Vec::with_capacity(128);

    b.iter(|| {
        buf.clear();

        black_box(Password::Cleartext("password")).encode(&mut buf);
    });
}

#[cfg(all(test, not(debug_assertions)))]
#[bench]
fn bench_encode_md5_password(b: &mut test::Bencher) {
    use test::black_box;

    let mut buf = Vec::with_capacity(128);

    b.iter(|| {
        buf.clear();

        black_box(Password::Md5 {
            password: "password",
            username: "root",
            salt: [147, 24, 57, 152],
        })
        .encode(&mut buf);
    });
}
