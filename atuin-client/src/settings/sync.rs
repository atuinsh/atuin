use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, Default)]
pub struct Sync {
    pub records: bool,
}
