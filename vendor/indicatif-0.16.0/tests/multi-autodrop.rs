use indicatif::{MultiProgress, ProgressBar};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[test]
fn main() {
    let m = MultiProgress::new();
    let pb = m.add(ProgressBar::new(10));
    let (tx, rx) = mpsc::channel();

    // start a thread to drive MultiProgress
    let h = thread::spawn(move || {
        m.join().unwrap();
        tx.send(()).unwrap();
        println!("Done in thread, droping MultiProgress");
    });

    {
        let pb2 = pb.clone();
        for _ in 0..10 {
            pb2.inc(1);
            thread::sleep(Duration::from_millis(50));
        }
    }

    // make sure anything is done in driver thread
    thread::sleep(Duration::from_millis(50));

    // the driver thread shouldn't finish
    rx.try_recv()
        .expect_err("The driver thread shouldn't finish");

    pb.set_message("Done");
    pb.finish();

    // make sure anything is done in driver thread
    thread::sleep(Duration::from_millis(50));

    // the driver thread should finish here
    rx.try_recv().expect("The driver thread should finish");

    h.join().unwrap();

    println!("Done in main");
}
