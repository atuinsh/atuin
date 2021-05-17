use indicatif::ProgressBar;
use std::time::Duration;
use tokio::runtime;
use tokio::time::interval;

fn main() {
    // Plain progress bar, totaling 1024 steps.
    let steps = 1024;
    let pb = ProgressBar::new(steps);

    // Stream of events, triggering every 5ms.
    let rt = runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("failed to create runtime");

    // Future computation which runs for `steps` interval events,
    // incrementing one step of the progress bar each time.
    let future = async {
        let mut intv = interval(Duration::from_millis(5));

        for _ in 0..steps {
            intv.tick().await;
            pb.inc(1);
        }
    };

    // Drive the future to completion, blocking until done.
    rt.block_on(future);

    // Mark the progress bar as finished.
    pb.finish();
}
