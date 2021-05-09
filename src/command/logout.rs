use std::fs::remove_file;

pub fn run() {
    let session_path = atuin_common::utils::data_dir().join("session");

    if session_path.exists() {
        remove_file(session_path.as_path()).expect("Failed to remove session file");
        println!("You have logged out!");
    } else {
        println!("You are not logged in");
    }
}
