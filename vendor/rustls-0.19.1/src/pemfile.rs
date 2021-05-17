use crate::key;
use base64;
use std::io;

/// Extract and decode all PEM sections from `rd`, which begin with `start_mark`
/// and end with `end_mark`.  Apply the functor `f` to each decoded buffer,
/// and return a Vec of `f`'s return values.
fn extract<A>(
    rd: &mut dyn io::BufRead,
    start_mark: &str,
    end_mark: &str,
    f: &dyn Fn(Vec<u8>) -> A,
) -> Result<Vec<A>, ()> {
    let mut ders = Vec::new();
    let mut b64buf = String::new();
    let mut take_base64 = false;

    let mut raw_line = Vec::<u8>::new();
    loop {
        raw_line.clear();
        let len = rd
            .read_until(b'\n', &mut raw_line)
            .map_err(|_| ())?;

        if len == 0 {
            return Ok(ders);
        }
        let line = String::from_utf8_lossy(&raw_line);

        if line.starts_with(start_mark) {
            take_base64 = true;
            continue;
        }

        if line.starts_with(end_mark) {
            take_base64 = false;
            let der = base64::decode(&b64buf).map_err(|_| ())?;
            ders.push(f(der));
            b64buf = String::new();
            continue;
        }

        if take_base64 {
            b64buf.push_str(line.trim());
        }
    }
}

/// Extract all the certificates from rd, and return a vec of `key::Certificate`s
/// containing the der-format contents.
pub fn certs(rd: &mut dyn io::BufRead) -> Result<Vec<key::Certificate>, ()> {
    extract(
        rd,
        "-----BEGIN CERTIFICATE-----",
        "-----END CERTIFICATE-----",
        &|v| key::Certificate(v),
    )
}

/// Extract all RSA private keys from rd, and return a vec of `key::PrivateKey`s
/// containing the der-format contents.
pub fn rsa_private_keys(rd: &mut dyn io::BufRead) -> Result<Vec<key::PrivateKey>, ()> {
    extract(
        rd,
        "-----BEGIN RSA PRIVATE KEY-----",
        "-----END RSA PRIVATE KEY-----",
        &|v| key::PrivateKey(v),
    )
}

/// Extract all PKCS8-encoded private keys from rd, and return a vec of
/// `key::PrivateKey`s containing the der-format contents.
pub fn pkcs8_private_keys(rd: &mut dyn io::BufRead) -> Result<Vec<key::PrivateKey>, ()> {
    extract(
        rd,
        "-----BEGIN PRIVATE KEY-----",
        "-----END PRIVATE KEY-----",
        &|v| key::PrivateKey(v),
    )
}
