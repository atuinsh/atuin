#![cfg(feature = "toml")]

extern crate config;

use config::*;

fn make() -> Config {
    let mut c = Config::default();
    c.merge(File::new("tests/Settings", FileFormat::Toml))
        .unwrap();

    c.merge(File::new("tests/Settings-production", FileFormat::Toml))
        .unwrap();

    c
}

#[test]
fn test_merge() {
    let c = make();

    assert_eq!(c.get("debug").ok(), Some(false));
    assert_eq!(c.get("production").ok(), Some(true));
    assert_eq!(
        c.get("place.creator.name").ok(),
        Some("Somebody New".to_string())
    );
    assert_eq!(c.get("place.rating").ok(), Some(4.9));
}

#[test]
fn test_merge_whole_config() {
    let mut c1 = Config::default();
    let mut c2 = Config::default();

    c1.set("x", 10).unwrap();
    c2.set("y", 25).unwrap();

    assert_eq!(c1.get("x").ok(), Some(10));
    assert_eq!(c2.get::<()>("x").ok(), None);

    assert_eq!(c2.get("y").ok(), Some(25));
    assert_eq!(c1.get::<()>("y").ok(), None);

    c1.merge(c2).unwrap();

    assert_eq!(c1.get("x").ok(), Some(10));
    assert_eq!(c1.get("y").ok(), Some(25));
}
