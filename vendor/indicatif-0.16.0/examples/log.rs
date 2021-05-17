use std::thread;
use std::time::Duration;

use indicatif::ProgressBar;

fn main() {
    let pb = ProgressBar::new(100);
    for i in 0..100 {
        thread::sleep(Duration::from_millis(25));
        pb.println(format!("[+] finished #{}", i));
        pb.inc(1);
    }
    pb.finish_with_message("done");
}
