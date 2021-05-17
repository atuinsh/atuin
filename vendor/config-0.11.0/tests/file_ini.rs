#![cfg(feature = "ini")]

extern crate config;
extern crate float_cmp;
extern crate serde;

#[macro_use]
extern crate serde_derive;

use config::*;
use std::path::PathBuf;

#[derive(Debug, Deserialize, PartialEq)]
struct Place {
    name: String,
    longitude: f64,
    latitude: f64,
    favorite: bool,
    reviews: u64,
    rating: Option<f32>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Settings {
    debug: f64,
    place: Place,
}

fn make() -> Config {
    let mut c = Config::default();
    c.merge(File::new("tests/Settings", FileFormat::Ini))
        .unwrap();
    c
}

#[test]
fn test_file() {
    let c = make();
    let s: Settings = c.try_into().unwrap();
    assert_eq!(
        s,
        Settings {
            debug: 1.0,
            place: Place {
                name: String::from("Torre di Pisa"),
                longitude: 43.7224985,
                latitude: 10.3970522,
                favorite: false,
                reviews: 3866,
                rating: Some(4.5),
            },
        }
    );
}

#[test]
fn test_error_parse() {
    let mut c = Config::default();
    let res = c.merge(File::new("tests/Settings-invalid", FileFormat::Ini));

    let path: PathBuf = ["tests", "Settings-invalid.ini"].iter().collect();

    assert!(res.is_err());
    assert_eq!(
        res.unwrap_err().to_string(),
        format!(
            r#"2:0 Expecting "[Some('='), Some(':')]" but found EOF. in {}"#,
            path.display()
        )
    );
}
