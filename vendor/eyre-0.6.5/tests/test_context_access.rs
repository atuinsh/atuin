#[test]
fn test_context() {
    use eyre::{eyre, Report};

    let error: Report = eyre!("oh no!");
    let _ = error.context();
}
