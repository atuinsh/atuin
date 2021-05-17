macro_rules! bench {
    ($name:ident($map:ident, $b:ident) $body:expr) => {
        mod $name {
            #[allow(unused_imports)]
            use super::custom_hdr;
            use fnv::FnvHasher;
            use http::header::*;
            use seahash::SeaHasher;
            use std::hash::BuildHasherDefault;
            #[allow(unused_imports)]
            use test::{self, Bencher};

            #[bench]
            fn header_map($b: &mut Bencher) {
                let $map = || HeaderMap::default();
                $body
            }

            #[bench]
            fn order_map_fnv($b: &mut Bencher) {
                use indexmap::IndexMap;
                let $map = || IndexMap::<_, _, BuildHasherDefault<FnvHasher>>::default();
                $body
            }

            #[bench]
            fn vec_map($b: &mut Bencher) {
                use crate::vec_map::VecMap;

                let $map = || VecMap::with_capacity(0);
                $body
            }

            #[bench]
            fn order_map_seahash($b: &mut Bencher) {
                use indexmap::IndexMap;
                let $map = || IndexMap::<_, _, BuildHasherDefault<SeaHasher>>::default();
                $body
            }

            /*
            #[bench]
            fn order_map_siphash($b: &mut Bencher) {
                use indexmap::IndexMap;
                let $map = || IndexMap::new();
                $body
            }

            #[bench]
            fn std_map_siphash($b: &mut Bencher) {
                use std::collections::HashMap;
                let $map = || HashMap::new();
                $body
            }
            */
        }
    };
}

bench!(new_insert_get_host(new_map, b) {
    b.iter(|| {
        let mut h = new_map();
        h.insert(HOST, "hyper.rs");
        test::black_box(h.get(&HOST));
    })
});

bench!(insert_4_std_get_30(new_map, b) {

    b.iter(|| {
        let mut h = new_map();

        for i in 0..4 {
            h.insert(super::STD[i].clone(), "foo");
        }

        for i in 0..30 {
            test::black_box(h.get(&super::STD[i % 4]));
        }
    })
});

bench!(insert_6_std_get_6(new_map, b) {

    b.iter(|| {
        let mut h = new_map();

        for i in 0..6 {
            h.insert(super::STD[i].clone(), "foo");
        }

        for i in 0..6 {
            test::black_box(h.get(&super::STD[i % 4]));
        }
    })
});

/*
bench!(insert_remove_host(new_map, b) {
    let mut h = new_map();

    b.iter(|| {
        test::black_box(h.insert(HOST, "hyper.rs"));
        test::black_box(h.remove(&HOST));
    })
});

bench!(insert_insert_host(new_map, b) {
    let mut h = new_map();

    b.iter(|| {
        test::black_box(h.insert(HOST, "hyper.rs"));
        test::black_box(h.insert(HOST, "hyper.rs"));
    })
});
*/

bench!(get_10_of_20_std(new_map, b) {
    let mut h = new_map();

    for hdr in super::STD[10..30].iter() {
        h.insert(hdr.clone(), hdr.as_str().to_string());
    }

    b.iter(|| {
        for hdr in &super::STD[10..20] {
            test::black_box(h.get(hdr));
        }
    })
});

bench!(get_100_std(new_map, b) {
    let mut h = new_map();

    for hdr in super::STD.iter() {
        h.insert(hdr.clone(), hdr.as_str().to_string());
    }

    b.iter(|| {
        for i in 0..100 {
            test::black_box(h.get(&super::STD[i % super::STD.len()]));
        }
    })
});

bench!(set_8_get_1_std(new_map, b) {
    b.iter(|| {
        let mut h = new_map();

        for hdr in &super::STD[0..8] {
            h.insert(hdr.clone(), "foo");
        }

        test::black_box(h.get(&super::STD[0]));
    })
});

bench!(set_10_get_1_std(new_map, b) {
    b.iter(|| {
        let mut h = new_map();

        for hdr in &super::STD[0..10] {
            h.insert(hdr.clone(), "foo");
        }

        test::black_box(h.get(&super::STD[0]));
    })
});

bench!(set_20_get_1_std(new_map, b) {
    b.iter(|| {
        let mut h = new_map();

        for hdr in &super::STD[0..20] {
            h.insert(hdr.clone(), "foo");
        }

        test::black_box(h.get(&super::STD[0]));
    })
});

bench!(get_10_custom_short(new_map, b) {
    let hdrs = custom_hdr(20);
    let mut h = new_map();

    for hdr in &hdrs {
        h.insert(hdr.clone(), hdr.as_str().to_string());
    }

    b.iter(|| {
        for hdr in &hdrs[..10] {
            test::black_box(h.get(hdr));
        }
    })
});

bench!(set_10_get_1_custom_short(new_map, b) {
    let hdrs = custom_hdr(10);

    b.iter(|| {
        let mut h = new_map();

        for hdr in &hdrs {
            h.insert(hdr.clone(), "foo");
        }

        test::black_box(h.get(&hdrs[0]));
    })
});

bench!(set_10_get_1_custom_med(new_map, b) {
    let hdrs = super::med_custom_hdr(10);

    b.iter(|| {
        let mut h = new_map();

        for hdr in &hdrs {
            h.insert(hdr.clone(), "foo");
        }

        test::black_box(h.get(&hdrs[0]));
    })
});

bench!(set_10_get_1_custom_long(new_map, b) {
    let hdrs = super::long_custom_hdr(10);

    b.iter(|| {
        let mut h = new_map();

        for hdr in &hdrs {
            h.insert(hdr.clone(), "foo");
        }

        test::black_box(h.get(&hdrs[0]));
    })
});

bench!(set_10_get_1_custom_very_long(new_map, b) {
    let hdrs = super::very_long_custom_hdr(10);

    b.iter(|| {
        let mut h = new_map();

        for hdr in &hdrs {
            h.insert(hdr.clone(), "foo");
        }

        test::black_box(h.get(&hdrs[0]));
    })
});

bench!(set_20_get_1_custom_short(new_map, b) {
    let hdrs = custom_hdr(20);

    b.iter(|| {
        let mut h = new_map();

        for hdr in &hdrs {
            h.insert(hdr.clone(), "foo");
        }

        test::black_box(h.get(&hdrs[0]));
    })
});

bench!(set_20_get_1_custom_med(new_map, b) {
    let hdrs = super::med_custom_hdr(20);

    b.iter(|| {
        let mut h = new_map();

        for hdr in &hdrs {
            h.insert(hdr.clone(), "foo");
        }

        test::black_box(h.get(&hdrs[0]));
    })
});

bench!(set_20_get_1_custom_long(new_map, b) {
    let hdrs = super::long_custom_hdr(20);

    b.iter(|| {
        let mut h = new_map();

        for hdr in &hdrs {
            h.insert(hdr.clone(), "foo");
        }

        test::black_box(h.get(&hdrs[0]));
    })
});

bench!(set_20_get_1_custom_very_long(new_map, b) {
    let hdrs = super::very_long_custom_hdr(20);

    b.iter(|| {
        let mut h = new_map();

        for hdr in &hdrs {
            h.insert(hdr.clone(), "foo");
        }

        test::black_box(h.get(&hdrs[0]));
    })
});

bench!(insert_all_std_headers(new_map, b) {
    b.iter(|| {
        let mut h = new_map();

        for hdr in super::STD {
            test::black_box(h.insert(hdr.clone(), "foo"));
        }
    })
});

bench!(insert_79_custom_std_headers(new_map, b) {
    let hdrs = super::custom_std(79);

    b.iter(|| {
        let mut h = new_map();

        for hdr in &hdrs {
            h.insert(hdr.clone(), "foo");
        }
    })
});

bench!(insert_100_custom_headers(new_map, b) {
    let hdrs = custom_hdr(100);

    b.iter(|| {
        let mut h = new_map();

        for hdr in &hdrs {
            test::black_box(h.insert(hdr.clone(), "foo"));
        }
    })
});

bench!(insert_500_custom_headers(new_map, b) {
    let hdrs = custom_hdr(500);

    b.iter(|| {
        let mut h = new_map();

        for hdr in &hdrs {
            test::black_box(h.insert(hdr.clone(), "foo"));
        }
    })
});

bench!(insert_one_15_char_header(new_map, b) {
    let hdr: HeaderName = "abcd-abcd-abcde"
        .parse().unwrap();

    b.iter(|| {
        let mut h = new_map();
        h.insert(hdr.clone(), "hello");
        test::black_box(h);
    })
});

bench!(insert_one_25_char_header(new_map, b) {
    let hdr: HeaderName = "abcd-abcd-abcd-abcd-abcde"
        .parse().unwrap();

    b.iter(|| {
        let mut h = new_map();
        h.insert(hdr.clone(), "hello");
        test::black_box(h);
    })
});

bench!(insert_one_50_char_header(new_map, b) {
    let hdr: HeaderName = "abcd-abcd-abcd-abcd-abcd-abcd-abcd-abcd-abcd-abcde"
        .parse().unwrap();

    b.iter(|| {
        let mut h = new_map();
        h.insert(hdr.clone(), "hello");
        test::black_box(h);
    })
});

bench!(insert_one_100_char_header(new_map, b) {
    let hdr: HeaderName = "abcd-abcd-abcd-abcd-abcd-abcd-abcd-abcd-abcd-abcdeabcd-abcd-abcd-abcd-abcd-abcd-abcd-abcd-abcd-abcde"
        .parse().unwrap();

    b.iter(|| {
        let mut h = new_map();
        h.insert(hdr.clone(), "hello");
        test::black_box(h);
    })
});

const HN_HDRS: [(&'static str, &'static str); 11] = [
    ("Date", "Fri, 27 Jan 2017 23:00:00 GMT"),
    ("Content-Type", "text/html; charset=utf-8"),
    ("Transfer-Encoding", "chunked"),
    ("Connection", "keep-alive"),
    ("Set-Cookie", "__cfduid=dbdfbbe3822b61cb8750ba37d894022151485558000; expires=Sat, 27-Jan-18 23:00:00 GMT; path=/; domain=.ycombinator.com; HttpOnly"),
    ("Vary", "Accept-Encoding"),
    ("Cache-Control", "private"),
    ("X-Frame-Options", "DENY"),
    ("Strict-Transport-Security", "max-age=31556900; includeSubDomains"),
    ("Server", "cloudflare-nginx"),
    ("CF-RAY", "327fd1809f3c1baf-SEA"),
];

bench!(hn_hdrs_set_8_get_many(new_map, b) {
    let hdrs: Vec<(HeaderName, &'static str)> = super::HN_HDRS[..8].iter()
        .map(|&(name, val)| (name.parse().unwrap(), val))
        .collect();

    b.iter(|| {
        let mut h = new_map();

        for &(ref name, val) in hdrs.iter() {
            h.insert(name.clone(), val);
        }

        for _ in 0..15 {
            test::black_box(h.get(&CONTENT_LENGTH));
            test::black_box(h.get(&VARY));
        }
    });
});

bench!(hn_hdrs_set_8_get_miss(new_map, b) {
    let hdrs: Vec<(HeaderName, &'static str)> = super::HN_HDRS[..8].iter()
        .map(|&(name, val)| (name.parse().unwrap(), val))
        .collect();

    let miss: HeaderName = "x-wat".parse().unwrap();

    b.iter(|| {
        let mut h = new_map();

        for &(ref name, val) in hdrs.iter() {
            h.insert(name.clone(), val);
        }

        test::black_box(h.get(&CONTENT_LENGTH));
        test::black_box(h.get(&miss));
    });
});

bench!(hn_hdrs_set_11_get_with_miss(new_map, b) {
    let hdrs: Vec<(HeaderName, &'static str)> = super::HN_HDRS.iter()
        .map(|&(name, val)| (name.parse().unwrap(), val))
        .collect();

    let miss: HeaderName = "x-wat".parse().unwrap();

    b.iter(|| {
        let mut h = new_map();

        for &(ref name, val) in hdrs.iter() {
            h.insert(name.clone(), val);
        }

        for _ in 0..10 {
            test::black_box(h.get(&CONTENT_LENGTH));
            test::black_box(h.get(&VARY));
            test::black_box(h.get(&miss));
        }
    });
});

use http::header::*;

fn custom_hdr(n: usize) -> Vec<HeaderName> {
    (0..n)
        .map(|i| {
            let s = format!("x-custom-{}", i);
            s.parse().unwrap()
        })
        .collect()
}

fn med_custom_hdr(n: usize) -> Vec<HeaderName> {
    (0..n)
        .map(|i| {
            let s = format!("content-length-{}", i);
            s.parse().unwrap()
        })
        .collect()
}

fn long_custom_hdr(n: usize) -> Vec<HeaderName> {
    (0..n)
        .map(|i| {
            let s = format!("access-control-allow-headers-{}", i);
            s.parse().unwrap()
        })
        .collect()
}

fn very_long_custom_hdr(n: usize) -> Vec<HeaderName> {
    (0..n)
        .map(|i| {
            let s = format!("access-control-allow-access-control-allow-headers-{}", i);
            s.parse().unwrap()
        })
        .collect()
}

fn custom_std(n: usize) -> Vec<HeaderName> {
    (0..n)
        .map(|i| {
            let s = format!("{}-{}", STD[i % STD.len()].as_str(), i);
            s.parse().unwrap()
        })
        .collect()
}

const STD: &'static [HeaderName] = &[
    ACCEPT,
    ACCEPT_CHARSET,
    ACCEPT_ENCODING,
    ACCEPT_LANGUAGE,
    ACCEPT_RANGES,
    ACCESS_CONTROL_ALLOW_CREDENTIALS,
    ACCESS_CONTROL_ALLOW_HEADERS,
    ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS,
    ACCESS_CONTROL_MAX_AGE,
    ACCESS_CONTROL_REQUEST_HEADERS,
    ACCESS_CONTROL_REQUEST_METHOD,
    AGE,
    ALLOW,
    ALT_SVC,
    AUTHORIZATION,
    CACHE_CONTROL,
    CONNECTION,
    CONTENT_DISPOSITION,
    CONTENT_ENCODING,
    CONTENT_LANGUAGE,
    CONTENT_LENGTH,
    CONTENT_LOCATION,
    CONTENT_RANGE,
    CONTENT_SECURITY_POLICY,
    CONTENT_SECURITY_POLICY_REPORT_ONLY,
    CONTENT_TYPE,
    COOKIE,
    DNT,
    DATE,
    ETAG,
    EXPECT,
    EXPIRES,
    FORWARDED,
    FROM,
    HOST,
    IF_MATCH,
    IF_MODIFIED_SINCE,
    IF_NONE_MATCH,
    IF_RANGE,
    IF_UNMODIFIED_SINCE,
    LAST_MODIFIED,
    LINK,
    LOCATION,
    MAX_FORWARDS,
    ORIGIN,
    PRAGMA,
    PROXY_AUTHENTICATE,
    PROXY_AUTHORIZATION,
    PUBLIC_KEY_PINS,
    PUBLIC_KEY_PINS_REPORT_ONLY,
    RANGE,
    REFERER,
    REFERRER_POLICY,
    REFRESH,
    RETRY_AFTER,
    SERVER,
    SET_COOKIE,
    STRICT_TRANSPORT_SECURITY,
    TE,
    TRAILER,
    TRANSFER_ENCODING,
    USER_AGENT,
    UPGRADE,
    UPGRADE_INSECURE_REQUESTS,
    VARY,
    VIA,
    WARNING,
    WWW_AUTHENTICATE,
    X_CONTENT_TYPE_OPTIONS,
    X_DNS_PREFETCH_CONTROL,
    X_FRAME_OPTIONS,
    X_XSS_PROTECTION,
];
