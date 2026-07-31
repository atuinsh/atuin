#![allow(irrefutable_let_patterns)]

use std::collections::HashMap;
use std::path::Path;

use assert_fs::prelude::*;

#[tokio::test]
async fn it_loads_local_assets() {
    let origin = assert_fs::TempDir::new().unwrap();

    let mut files = HashMap::new();
    files.insert("README.md", "# axoasset");
    files.insert("styles.css", "@import");

    for (file, contents) in files {
        let asset = origin.child(file);
        let content = Path::new("./tests/assets").join(file);
        asset.write_file(&content).unwrap();

        let origin_path = asset.to_str().unwrap();
        let loaded_asset = axoasset::LocalAsset::load_asset(origin_path).unwrap();
        assert!(std::str::from_utf8(loaded_asset.as_bytes())
            .unwrap()
            .contains(contents));
    }
}

#[tokio::test]
async fn it_loads_local_assets_as_bytes() {
    let origin = assert_fs::TempDir::new().unwrap();

    let mut files = HashMap::new();
    files.insert("README.md", "# axoasset");
    files.insert("styles.css", "@import");

    for (file, contents) in files {
        let asset = origin.child(file);
        let content = Path::new("./tests/assets").join(file);
        asset.write_file(&content).unwrap();

        let origin_path = asset.to_str().unwrap();
        let loaded_bytes = axoasset::LocalAsset::load_bytes(origin_path).unwrap();

        assert!(std::str::from_utf8(&loaded_bytes)
            .unwrap()
            .contains(contents));
    }
}

#[tokio::test]
async fn it_loads_local_assets_as_strings() {
    let origin = assert_fs::TempDir::new().unwrap();

    let mut files = HashMap::new();
    files.insert("README.md", "# axoasset");
    files.insert("styles.css", "@import");

    for (file, contents) in files {
        let asset = origin.child(file);
        let content = Path::new("./tests/assets").join(file);
        asset.write_file(&content).unwrap();

        let origin_path = asset.to_str().unwrap();
        let loaded_string = axoasset::LocalAsset::load_string(origin_path).unwrap();

        assert!(loaded_string.contains(contents))
    }
}
