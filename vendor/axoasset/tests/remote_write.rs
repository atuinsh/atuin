#![cfg(feature = "remote")]

use std::collections::HashMap;
use std::fs;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

#[tokio::test]
async fn it_writes_remote_assets() {
    let mock_server = MockServer::start().await;

    let dest = assert_fs::TempDir::new().unwrap();

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

        let dest = asset.write_to_dir(dest.to_str().unwrap()).await.unwrap();
        assert!(dest.exists());
        fs::read_to_string(dest).unwrap().contains(contents);
    }
}
