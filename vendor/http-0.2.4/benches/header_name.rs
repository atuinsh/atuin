#![feature(test)]

extern crate bytes;
extern crate http;
extern crate test;

use http::header::HeaderName;
use test::Bencher;

fn make_all_known_headers() -> Vec<Vec<u8>> {
    // Standard request headers
    vec![b"A-IM".to_vec(),
    b"Accept".to_vec(),
    b"Accept-Charset".to_vec(),
    b"Accept-Datetime".to_vec(),
    b"Accept-Encoding".to_vec(),
    b"Accept-Language".to_vec(),
    b"Access-Control-Request-Method".to_vec(),
    b"Authorization".to_vec(),
    b"Cache-Control".to_vec(),
    b"Connection".to_vec(),
    b"Permanent".to_vec(),
    b"Content-Length".to_vec(),
    b"Content-MD5".to_vec(),
    b"Content-Type".to_vec(),
    b"Cookie".to_vec(),
    b"Date".to_vec(),
    b"Expect".to_vec(),
    b"Forwarded".to_vec(),
    b"From".to_vec(),
    b"Host".to_vec(),
    b"Permanent".to_vec(),
    b"HTTP2-Settings".to_vec(),
    b"If-Match".to_vec(),
    b"If-Modified-Since".to_vec(),
    b"If-None-Match".to_vec(),
    b"If-Range".to_vec(),
    b"If-Unmodified-Since".to_vec(),
    b"Max-Forwards".to_vec(),
    b"Origin".to_vec(),
    b"Pragma".to_vec(),
    b"Proxy-Authorization".to_vec(),
    b"Range".to_vec(),
    b"Referer".to_vec(),
    b"TE".to_vec(),
    b"User-Agent".to_vec(),
    b"Upgrade".to_vec(),
    b"Via".to_vec(),
    b"Warning".to_vec(),
    // common_non_standard
    b"Upgrade-Insecure-Requests".to_vec(),
    b"Upgrade-Insecure-Requests".to_vec(),
    b"X-Requested-With".to_vec(),
    b"DNT".to_vec(),
    b"X-Forwarded-For".to_vec(),
    b"X-Forwarded-Host".to_vec(),
    b"X-Forwarded-Proto".to_vec(),
    b"Front-End-Https".to_vec(),
    b"X-Http-Method-Override".to_vec(),
    b"X-ATT-DeviceId".to_vec(),
    b"X-Wap-Profile".to_vec(),
    b"Proxy-Connection".to_vec(),
    b"X-UIDH".to_vec(),
    b"X-Csrf-Token".to_vec(),
    b"X-Request-ID".to_vec(),
    b"X-Correlation-ID".to_vec(),
    b"Save-Data".to_vec(),
    // standard_response_headers
    b"Accept-Patch".to_vec(),
    b"Accept-Ranges".to_vec(),
    b"Access-Control-Allow-Credentials".to_vec(),
    b"Access-Control-Allow-Headers".to_vec(),
    b"Access-Control-Allow-Methods".to_vec(),
    b"Access-Control-Allow-Origin".to_vec(),
    b"Access-Control-Expose-Headers".to_vec(),
    b"Access-Control-Max-Age".to_vec(),
    b"Age".to_vec(),
    b"Allow".to_vec(),
    b"Alt-Svc".to_vec(),
    b"Cache-Control".to_vec(),
    b"Connection".to_vec(),
    b"Content-Disposition".to_vec(),
    b"Content-Encoding".to_vec(),
    b"Content-Language".to_vec(),
    b"Content-Length".to_vec(),
    b"Content-Location".to_vec(),
    b"Content-MD5".to_vec(),
    b"Content-Range".to_vec(),
    b"Content-Type".to_vec(),
    b"Date".to_vec(),
    b"Delta-Base".to_vec(),
    b"ETag".to_vec(),
    b"Expires".to_vec(),
    b"IM".to_vec(),
    b"Last-Modified".to_vec(),
    b"Link".to_vec(),
    b"Location".to_vec(),
    b"P3P".to_vec(),
    b"Permanent".to_vec(),
    b"Pragma".to_vec(),
    b"Proxy-Authenticate".to_vec(),
    b"Public-Key-Pins".to_vec(),
    b"Retry-After".to_vec(),
    b"Server".to_vec(),
    b"Set-Cookie".to_vec(),
    b"Strict-Transport-Security".to_vec(),
    b"Tk".to_vec(),
    b"Trailer".to_vec(),
    b"Transfer-Encoding".to_vec(),
    b"Upgrade".to_vec(),
    b"Vary".to_vec(),
    b"Via".to_vec(),
    b"Warning".to_vec(),
    b"WWW-Authenticate".to_vec(),
    b"X-Frame-Options".to_vec(),
    // common_non_standard_response
    b"Content-Security-Policy".to_vec(),
    b"Refresh".to_vec(),
    b"Status".to_vec(),
    b"Timing-Allow-Origin".to_vec(),
    b"X-Content-Duration".to_vec(),
    b"X-Content-Security-Policy".to_vec(),
    b"X-Content-Type-Options".to_vec(),
    b"X-Correlation-ID".to_vec(),
    b"X-Powered-By".to_vec(),
    b"X-Request-ID".to_vec(),
    b"X-UA-Compatible".to_vec(),
    b"X-WebKit-CSP".to_vec(),
    b"X-XSS-Protection".to_vec(),
    ]
}

#[bench]
fn header_name_easy(b: &mut Bencher) {
    let name = b"Content-type";
    b.iter(|| {
        HeaderName::from_bytes(&name[..]).unwrap();
    });
}

#[bench]
fn header_name_bad(b: &mut Bencher) {
    let name = b"bad header name";
    b.iter(|| {
        HeaderName::from_bytes(&name[..]).expect_err("Bad header name");
    });
}

#[bench]
fn header_name_various(b: &mut Bencher) {
    let all_known_headers = make_all_known_headers();
    b.iter(|| {
        for name in &all_known_headers{
            HeaderName::from_bytes(name.as_slice()).unwrap();
        }
    });
}
