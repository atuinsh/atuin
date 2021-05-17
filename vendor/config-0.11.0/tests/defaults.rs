extern crate config;

#[macro_use]
extern crate serde_derive;

use config::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub db_host: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            db_host: String::from("default"),
        }
    }
}

#[test]
fn set_defaults() {
    let c = Config::new();
    let s: Settings = c.try_into().expect("Deserialization failed");

    assert_eq!(s.db_host, "default");
}

#[test]
fn try_from_defaults() {
    let c = Config::try_from(&Settings::default()).expect("Serialization failed");
    let s: Settings = c.try_into().expect("Deserialization failed");
    assert_eq!(s.db_host, "default");
}
