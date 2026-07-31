use std::fs;
use std::path::Path;

#[test]
fn it_removes_both_file_and_directory() {
    let dest = assert_fs::TempDir::new().unwrap();
    let file_path = Path::new(&dest.as_os_str()).join("subdir").join("test.md");
    let dir_path = Path::new(&dest.as_os_str()).join("subdir");

    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(&file_path, "hello").unwrap();

    axoasset::LocalAsset::remove_file(file_path.display().to_string()).unwrap();
    assert!(!file_path.exists());

    axoasset::LocalAsset::remove_dir(dir_path.display().to_string()).unwrap();
    assert!(!dir_path.exists());
}
