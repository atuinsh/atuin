// This program does benchmarking of the functions in verify.rs,
// that do certificate chain validation and signature verification.
//
// Note: we don't use any of the standard 'cargo bench', 'test::Bencher',
// etc. because it's unstable at the time of writing.

use std::time::{Duration, Instant};

use crate::anchors;
use crate::error::TLSError;
use crate::key;
use crate::verify;
use crate::verify::ServerCertVerifier;
use webpki;

use webpki_roots;

fn duration_nanos(d: Duration) -> u64 {
    ((d.as_secs() as f64) * 1e9 + (d.subsec_nanos() as f64)) as u64
}

fn bench<Fsetup, Ftest, S>(count: usize, name: &'static str, f_setup: Fsetup, f_test: Ftest)
where
    Fsetup: Fn() -> S,
    Ftest: Fn(S),
{
    let mut times = Vec::new();

    for _ in 0..count {
        let state = f_setup();
        let start = Instant::now();
        f_test(state);
        times.push(duration_nanos(Instant::now().duration_since(start)));
    }

    println!("{}: min {:?}us", name, times.iter().min().unwrap() / 1000);
}

fn fixed_time() -> Result<webpki::Time, TLSError> {
    Ok(webpki::Time::from_seconds_since_unix_epoch(1500000000))
}

static V: &'static verify::WebPKIVerifier = &verify::WebPKIVerifier { time: fixed_time };

#[test]
fn test_reddit_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-reddit.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-reddit.1.der").to_vec());
    let chain = [cert0, cert1];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(reddit)",
        || (),
        |_| {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str("reddit.com").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}

#[test]
fn test_github_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-github.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-github.1.der").to_vec());
    let chain = [cert0, cert1];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(github)",
        || (),
        |_| {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str("github.com").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}

#[test]
fn test_arstechnica_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-arstechnica.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-arstechnica.1.der").to_vec());
    let cert2 = key::Certificate(include_bytes!("testdata/cert-arstechnica.2.der").to_vec());
    let chain = [cert0, cert1, cert2];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(arstechnica)",
        || (),
        |_| {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str("arstechnica.com").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}

#[test]
fn test_servo_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-servo.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-servo.1.der").to_vec());
    let cert2 = key::Certificate(include_bytes!("testdata/cert-servo.2.der").to_vec());
    let chain = [cert0, cert1, cert2];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(servo)",
        || (),
        |_| {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str("servo.org").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}

#[test]
fn test_twitter_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-twitter.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-twitter.1.der").to_vec());
    let chain = [cert0, cert1];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(twitter)",
        || (),
        |_| {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str("twitter.com").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}

#[test]
fn test_wikipedia_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-wikipedia.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-wikipedia.1.der").to_vec());
    let chain = [cert0, cert1];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(wikipedia)",
        || (),
        |_| {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str("wikipedia.org").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}

#[test]
fn test_google_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-google.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-google.1.der").to_vec());
    let cert2 = key::Certificate(include_bytes!("testdata/cert-google.2.der").to_vec());
    let chain = [cert0, cert1, cert2];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(google)",
        || (),
        |_| {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str("www.google.com").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}

#[test]
fn test_hn_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-hn.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-hn.1.der").to_vec());
    let cert2 = key::Certificate(include_bytes!("testdata/cert-hn.2.der").to_vec());
    let chain = [cert0, cert1, cert2];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(hn)",
        || (),
        |_| {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str("news.ycombinator.com").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}

#[test]
fn test_stackoverflow_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-stackoverflow.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-stackoverflow.1.der").to_vec());
    let chain = [cert0, cert1];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(stackoverflow)",
        || (),
        |_| {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str("stackoverflow.com").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}

#[test]
fn test_duckduckgo_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-duckduckgo.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-duckduckgo.1.der").to_vec());
    let chain = [cert0, cert1];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(duckduckgo)",
        || (),
        |_| {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str("duckduckgo.com").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}

#[test]
fn test_rustlang_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-rustlang.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-rustlang.1.der").to_vec());
    let cert2 = key::Certificate(include_bytes!("testdata/cert-rustlang.2.der").to_vec());
    let chain = [cert0, cert1, cert2];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(rustlang)",
        || (),
        |_| {
            let dns_name = webpki::DNSNameRef::try_from_ascii_str("www.rust-lang.org").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}

#[test]
fn test_wapo_cert() {
    let cert0 = key::Certificate(include_bytes!("testdata/cert-wapo.0.der").to_vec());
    let cert1 = key::Certificate(include_bytes!("testdata/cert-wapo.1.der").to_vec());
    let cert2 = key::Certificate(include_bytes!("testdata/cert-wapo.2.der").to_vec());
    let chain = [cert0, cert1, cert2];
    let mut anchors = anchors::RootCertStore::empty();
    anchors.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    bench(
        100,
        "verify_server_cert(wapo)",
        || (),
        |_| {
            let dns_name =
                webpki::DNSNameRef::try_from_ascii_str("www.washingtonpost.com").unwrap();
            V.verify_server_cert(&anchors, &chain[..], dns_name, &[])
                .unwrap();
        },
    );
}
