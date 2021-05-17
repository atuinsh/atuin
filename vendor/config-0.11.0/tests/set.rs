extern crate config;

use config::*;

#[test]
fn test_set_scalar() {
    let mut c = Config::default();

    c.set("value", true).unwrap();

    assert_eq!(c.get("value").ok(), Some(true));
}

#[cfg(feature = "toml")]
#[test]
fn test_set_scalar_default() {
    let mut c = Config::default();

    c.merge(File::new("tests/Settings", FileFormat::Toml))
        .unwrap();

    c.set_default("debug", false).unwrap();
    c.set_default("staging", false).unwrap();

    assert_eq!(c.get("debug").ok(), Some(true));
    assert_eq!(c.get("staging").ok(), Some(false));
}

#[cfg(feature = "toml")]
#[test]
fn test_set_scalar_path() {
    let mut c = Config::default();

    c.set("first.second.third", true).unwrap();

    assert_eq!(c.get("first.second.third").ok(), Some(true));

    c.merge(File::new("tests/Settings", FileFormat::Toml))
        .unwrap();

    c.set_default("place.favorite", true).unwrap();
    c.set_default("place.blocked", true).unwrap();

    assert_eq!(c.get("place.favorite").ok(), Some(false));
    assert_eq!(c.get("place.blocked").ok(), Some(true));
}

#[cfg(feature = "toml")]
#[test]
fn test_set_arr_path() {
    let mut c = Config::default();

    c.set("items[0].name", "Ivan").unwrap();

    assert_eq!(c.get("items[0].name").ok(), Some("Ivan".to_string()));

    c.set("data[0].things[1].name", "foo").unwrap();
    c.set("data[0].things[1].value", 42).unwrap();
    c.set("data[1]", 0).unwrap();

    assert_eq!(
        c.get("data[0].things[1].name").ok(),
        Some("foo".to_string())
    );
    assert_eq!(c.get("data[0].things[1].value").ok(), Some(42));
    assert_eq!(c.get("data[1]").ok(), Some(0));

    c.merge(File::new("tests/Settings", FileFormat::Toml))
        .unwrap();

    c.set("items[0].name", "John").unwrap();

    assert_eq!(c.get("items[0].name").ok(), Some("John".to_string()));

    c.set("items[2]", "George").unwrap();

    assert_eq!(c.get("items[2]").ok(), Some("George".to_string()));
}

#[cfg(feature = "toml")]
#[test]
fn test_set_capital() {
    let mut c = Config::default();

    c.set_default("this", false).unwrap();
    c.set("ThAt", true).unwrap();
    c.merge(File::from_str("{\"logLevel\": 5}", FileFormat::Json))
        .unwrap();

    assert_eq!(c.get("this").ok(), Some(false));
    assert_eq!(c.get("ThAt").ok(), Some(true));
    assert_eq!(c.get("logLevel").ok(), Some(5));
}
