use std::cmp::min;
use std::thread;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

fn main() {
    let mut downloaded = 69369369;
    let total_size = 231231231;

    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .progress_chars("#>-"),
    );
    pb.set_position(downloaded);
    pb.reset_eta();

    while downloaded < total_size {
        downloaded = min(downloaded + 123211, total_size);
        pb.set_position(downloaded);
        thread::sleep(Duration::from_millis(12));
    }

    pb.finish_with_message("downloaded");
}
