#![cfg(feature = "remote")]

use std::collections::HashMap;
use std::fs;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

#[tokio::test]
async fn it_loads_remote_assets() {
    let mock_server = MockServer::start().await;

    let mut assets = HashMap::new();
    assets.insert("/README.md", "# axoasset");
    assets.insert("/README", "# axoasset");
    assets.insert("/styles.css", "@import");
    assets.insert("/styles", "@import");

    for (route, contents) in assets {
        let response = if route.contains("README") {
            let readme_bytes = fs::read("./tests/assets/README.md").unwrap();
            ResponseTemplate::new(200)
                .set_body_bytes(readme_bytes)
                .insert_header("Content-Type", "text/plain+md")
        } else {
            let styles_bytes = fs::read("./tests/assets/styles.css").unwrap();
            ResponseTemplate::new(200)
                .set_body_bytes(styles_bytes)
                .insert_header("Content-Type", "text/css")
        };

        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(response)
            .mount(&mock_server)
            .await;

        let mut origin_path = format!("http://{}", mock_server.address());
        origin_path.push_str(route);
        let asset = common::client().load_asset(&origin_path).await.unwrap();

        assert!(std::str::from_utf8(asset.as_bytes())
            .unwrap()
            .contains(contents));
    }
}

#[tokio::test]
async fn it_loads_remote_assets_as_bytes() {
    let mock_server = MockServer::start().await;

    let mut assets = HashMap::new();
    assets.insert("/README.md", "# axoasset");
    assets.insert("/README", "# axoasset");
    assets.insert("/styles.css", "@import");
    assets.insert("/styles", "@import");

    for (route, contents) in assets {
        let response = if route.contains("README") {
            let readme_bytes = fs::read("./tests/assets/README.md").unwrap();
            ResponseTemplate::new(200)
                .set_body_bytes(readme_bytes)
                .insert_header("Content-Type", "text/plain+md")
        } else {
            let styles_bytes = fs::read("./tests/assets/styles.css").unwrap();
            ResponseTemplate::new(200)
                .set_body_bytes(styles_bytes)
                .insert_header("Content-Type", "text/css")
        };

        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(response)
            .mount(&mock_server)
            .await;

        let mut origin_path = format!("http://{}", mock_server.address());
        origin_path.push_str(route);
        let loaded_bytes = common::client().load_bytes(&origin_path).await.unwrap();

        assert!(std::str::from_utf8(&loaded_bytes)
            .unwrap()
            .contains(contents));
    }
}

#[tokio::test]
async fn it_loads_remote_assets_as_string() {
    let mock_server = MockServer::start().await;

    let mut assets = HashMap::new();
    assets.insert("/README.md", "# axoasset");
    assets.insert("/README", "# axoasset");
    assets.insert("/styles.css", "@import");
    assets.insert("/styles", "@import");

    for (route, contents) in assets {
        let response = if route.contains("README") {
            let readme_bytes = fs::read("./tests/assets/README.md").unwrap();
            ResponseTemplate::new(200)
                .set_body_bytes(readme_bytes)
                .insert_header("Content-Type", "text/plain+md")
        } else {
            let styles_bytes = fs::read("./tests/assets/styles.css").unwrap();
            ResponseTemplate::new(200)
                .set_body_bytes(styles_bytes)
                .insert_header("Content-Type", "text/css")
        };

        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(response)
            .mount(&mock_server)
            .await;

        let mut origin_path = format!("http://{}", mock_server.address());
        origin_path.push_str(route);
        let loaded_string = common::client().load_string(&origin_path).await.unwrap();

        assert!(loaded_string.contains(contents));
    }
}
