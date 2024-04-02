use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    pub height_strategy: PreviewHeightStrategy,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            height_strategy: PreviewHeightStrategy::AllResults,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Copy)]
pub enum PreviewHeightStrategy {
    #[serde(rename = "all_results")]
    AllResults,

    #[serde(rename = "selected_result")]
    SelectedResult,
}
