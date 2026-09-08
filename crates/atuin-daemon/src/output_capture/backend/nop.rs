use atuin_client::history::{CommandCapture, HistoryId};

use super::{Backend, CaptureError, GetOutputError};

#[derive(Debug, Clone, Copy)]
pub struct NopBackend;

impl Backend for NopBackend {
    async fn capture(&self, _id: HistoryId, _capture: CommandCapture) -> Result<(), CaptureError> {
        Ok(())
    }

    async fn get(&self, _id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use easy_cast::Conv;
    use uuid::Uuid;

    use super::*;

    fn hid(n: u128) -> HistoryId {
        HistoryId::from_bytes(*Uuid::from_u128(n).as_bytes())
    }

    fn cap(output: &str) -> CommandCapture {
        CommandCapture {
            output: output.to_string(),
            output_observed_bytes: u64::conv(output.len()),
            output_truncated: false,
            terminal_width: 80,
            terminal_height: 24,
        }
    }

    #[tokio::test]
    async fn capture_is_discarded() {
        let backend = NopBackend;
        backend.capture(hid(1), cap("hello")).await.expect("capture");
        assert!(backend.get(hid(1)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn repeated_capture_for_same_id_is_accepted() {
        let backend = NopBackend;
        backend.capture(hid(1), cap("first")).await.expect("first");
        backend.capture(hid(1), cap("second")).await.expect("second");
    }

    #[tokio::test]
    async fn get_is_always_none() {
        let backend = NopBackend;
        assert!(backend.get(hid(42)).await.expect("get").is_none());
    }
}
