#![cfg(not(target_arch = "wasm32"))]
mod support;
use futures_util::stream::StreamExt;
use support::*;

use reqwest::Client;

#[tokio::test]
async fn auto_headers() {
    let server = server::http(move |req| async move {
        assert_eq!(req.method(), "GET");

        assert_eq!(req.headers()["accept"], "*/*");
        assert_eq!(req.headers().get("user-agent"), None);
        if cfg!(feature = "gzip") {
            assert!(req.headers()["accept-encoding"]
                .to_str()
                .unwrap()
                .contains("gzip"));
        }
        if cfg!(feature = "brotli") {
            assert!(req.headers()["accept-encoding"]
                .to_str()
                .unwrap()
                .contains("br"));
        }

        http::Response::default()
    });

    let url = format!("http://{}/1", server.addr());
    let res = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(&url)
        .send()
        .await
        .unwrap();

    assert_eq!(res.url().as_str(), &url);
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    assert_eq!(res.remote_addr(), Some(server.addr()));
}

#[tokio::test]
async fn user_agent() {
    let server = server::http(move |req| async move {
        assert_eq!(req.headers()["user-agent"], "reqwest-test-agent");
        http::Response::default()
    });

    let url = format!("http://{}/ua", server.addr());
    let res = reqwest::Client::builder()
        .user_agent("reqwest-test-agent")
        .build()
        .expect("client builder")
        .get(&url)
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn response_text() {
    let _ = env_logger::try_init();

    let server = server::http(move |_req| async { http::Response::new("Hello".into()) });

    let client = Client::new();

    let res = client
        .get(&format!("http://{}/text", server.addr()))
        .send()
        .await
        .expect("Failed to get");
    assert_eq!(res.content_length(), Some(5));
    let text = res.text().await.expect("Failed to get text");
    assert_eq!("Hello", text);
}

#[tokio::test]
async fn response_bytes() {
    let _ = env_logger::try_init();

    let server = server::http(move |_req| async { http::Response::new("Hello".into()) });

    let client = Client::new();

    let res = client
        .get(&format!("http://{}/bytes", server.addr()))
        .send()
        .await
        .expect("Failed to get");
    assert_eq!(res.content_length(), Some(5));
    let bytes = res.bytes().await.expect("res.bytes()");
    assert_eq!("Hello", bytes);
}

#[tokio::test]
#[cfg(feature = "json")]
async fn response_json() {
    let _ = env_logger::try_init();

    let server = server::http(move |_req| async { http::Response::new("\"Hello\"".into()) });

    let client = Client::new();

    let res = client
        .get(&format!("http://{}/json", server.addr()))
        .send()
        .await
        .expect("Failed to get");
    let text = res.json::<String>().await.expect("Failed to get json");
    assert_eq!("Hello", text);
}

#[tokio::test]
async fn body_pipe_response() {
    let _ = env_logger::try_init();

    let server = server::http(move |mut req| async move {
        if req.uri() == "/get" {
            http::Response::new("pipe me".into())
        } else {
            assert_eq!(req.uri(), "/pipe");
            assert_eq!(req.headers()["transfer-encoding"], "chunked");

            let mut full: Vec<u8> = Vec::new();
            while let Some(item) = req.body_mut().next().await {
                full.extend(&*item.unwrap());
            }

            assert_eq!(full, b"pipe me");

            http::Response::default()
        }
    });

    let client = Client::new();

    let res1 = client
        .get(&format!("http://{}/get", server.addr()))
        .send()
        .await
        .expect("get1");

    assert_eq!(res1.status(), reqwest::StatusCode::OK);
    assert_eq!(res1.content_length(), Some(7));

    // and now ensure we can "pipe" the response to another request
    let res2 = client
        .post(&format!("http://{}/pipe", server.addr()))
        .body(res1)
        .send()
        .await
        .expect("res2");

    assert_eq!(res2.status(), reqwest::StatusCode::OK);
}

#[cfg(any(feature = "native-tls", feature = "__rustls",))]
#[test]
fn use_preconfigured_tls_with_bogus_backend() {
    struct DefinitelyNotTls;

    reqwest::Client::builder()
        .use_preconfigured_tls(DefinitelyNotTls)
        .build()
        .expect_err("definitely is not TLS");
}

#[cfg(feature = "native-tls")]
#[test]
fn use_preconfigured_native_tls_default() {
    extern crate native_tls_crate;

    let tls = native_tls_crate::TlsConnector::builder()
        .build()
        .expect("tls builder");

    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .build()
        .expect("preconfigured default tls");
}

#[cfg(feature = "__rustls")]
#[test]
fn use_preconfigured_rustls_default() {
    extern crate rustls;

    let tls = rustls::ClientConfig::new();

    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .build()
        .expect("preconfigured rustls tls");
}

#[cfg(feature = "__rustls")]
#[tokio::test]
#[ignore = "Needs TLS support in the test server"]
async fn http2_upgrade() {
    let server = server::http(move |_| async move { http::Response::default() });

    let url = format!("https://localhost:{}", server.addr().port());
    let res = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .use_rustls_tls()
        .build()
        .expect("client builder")
        .get(&url)
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    assert_eq!(res.version(), reqwest::Version::HTTP_2);
}

#[cfg(feature = "default-tls")]
#[tokio::test]
async fn test_allowed_methods() {
    let resp = reqwest::Client::builder()
        .https_only(true)
        .build()
        .expect("client builder")
        .get("https://google.com")
        .send()
        .await;

    assert_eq!(resp.is_err(), false);

    let resp = reqwest::Client::builder()
        .https_only(true)
        .build()
        .expect("client builder")
        .get("http://google.com")
        .send()
        .await;

    assert_eq!(resp.is_err(), true);
}
