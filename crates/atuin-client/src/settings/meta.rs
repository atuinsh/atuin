use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    pub db_path: String,
}

impl Default for Settings {
    fn default() -> Self {
        let dir = atuin_common::utils::data_dir();
        let path = dir.join("meta.db");

        Self {
            db_path: path.to_string_lossy().to_string(),
        }
    }
}
