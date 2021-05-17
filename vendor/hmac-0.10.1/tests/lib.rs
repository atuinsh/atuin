//! Test vectors from:
//! - md5: RFC 2104, plus wiki test
//! - sha2: RFC 4231

#![no_std]

use crypto_mac::new_test;
use hmac::Hmac;

new_test!(hmac_md5, "md5", Hmac<md5::Md5>);
new_test!(hmac_sha224, "sha224", Hmac<sha2::Sha224>);
new_test!(hmac_sha256, "sha256", Hmac<sha2::Sha256>);
new_test!(hmac_sha384, "sha384", Hmac<sha2::Sha384>);
new_test!(hmac_sha512, "sha512", Hmac<sha2::Sha512>);
