#![cfg(feature = "remote")]

mod common;

use std::fs;
use std::path::Path;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn it_copies_remote_assets() {
    let mock_server = MockServer::start().await;

    let dest = assert_fs::TempDir::new().unwrap();
    let dest_dir = Path::new(dest.to_str().unwrap());

    let routes = vec!["README.md", "styles.css"];
    let readme_string = fs::read_to_string("./tests/assets/README.md").unwrap();
    let styles_string = fs::read_to_string("./tests/assets/styles.css").unwrap();

    for route in routes {
        let resp_string = if route.to_uppercase().contains("README") {
            &readme_string
        } else {
            &styles_string
        };

        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_string(resp_string))
            .mount(&mock_server)
            .await;

        let origin_path = format!("http://{}/{}", mock_server.address(), route);
        let copied_filename = common::client()
            .load_and_write_to_dir(&origin_path, dest.to_str().unwrap())
            .await
            .unwrap();
        let copied_file = dest_dir.join(copied_filename);
        assert!(copied_file.exists());
    }
}
