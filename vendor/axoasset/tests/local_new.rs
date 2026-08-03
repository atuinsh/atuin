#![allow(irrefutable_let_patterns)]

use std::collections::HashMap;
use std::path::Path;

#[tokio::test]
async fn it_creates_new_assets() {
    let dest = assert_fs::TempDir::new().unwrap();

    let mut files = HashMap::new();
    files.insert("README.md", "# axoasset");
    files.insert("styles.css", "@import");

    for (file, contents) in files {
        let origin_path = Path::new("./tests/assets").join(file).display().to_string();
        let dest_dir = Path::new(&dest.as_os_str())
            .join(file)
            .display()
            .to_string();
        axoasset::LocalAsset::new(&origin_path, contents.into())
            .unwrap()
            .write_to_dir(dest.to_str().unwrap())
            .unwrap();

        let loaded_asset = axoasset::LocalAsset::load_asset(&dest_dir).unwrap();

        assert!(std::str::from_utf8(loaded_asset.as_bytes())
            .unwrap()
            .contains(contents));
    }
}

#[test]
fn it_creates_parent_directories() {
    let dest = assert_fs::TempDir::new().unwrap();

    let dest_path = Path::new(&dest.as_os_str())
        .join("subdir")
        .join("test.md")
        .display()
        .to_string();
    axoasset::LocalAsset::write_new_all("file content", dest_path).unwrap();

    assert!(Path::new(&dest.as_os_str()).join("subdir").exists());
}

#[test]
fn it_creates_a_new_directory() {
    let dest = assert_fs::TempDir::new().unwrap();

    let dest_dir = Path::new(&dest.as_os_str())
        .join("subdir")
        .display()
        .to_string();
    axoasset::LocalAsset::create_dir(dest_dir).unwrap();

    assert!(Path::new(&dest.as_os_str()).join("subdir").exists());
}
