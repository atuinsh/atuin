extern crate config;

#[macro_use]
extern crate serde_derive;

use config::*;

#[derive(Debug, Serialize, Deserialize)]
struct Settings {
    #[serde(skip)]
    foo: isize,
    #[serde(skip)]
    bar: u8,
}

#[test]
fn empty_deserializes() {
    let s: Settings = Config::new().try_into().expect("Deserialization failed");
    assert_eq!(s.foo, 0);
    assert_eq!(s.bar, 0);
}
