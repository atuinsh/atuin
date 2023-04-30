use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Some context that is attached to a history entry,
/// such as environment variables.
///
/// Now it's just stored, so any user can query it himself
/// from SQLite DB for his own analytics.
///
/// ## Implementation notes
/// All fields _must_ be marked as `#[serde(default)]` to ensure
/// compatibility between different versions of the client.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Context {
    #[serde(default)]
    pub env_vars: Option<HashMap<String, String>>,
}

impl ToString for Context {
    fn to_string(&self) -> String {
        serde_json::to_string(self).expect(
            "JSON serialization failed, but it shouldn't, as all keys are guaranteed to be strings",
        )
    }
}
