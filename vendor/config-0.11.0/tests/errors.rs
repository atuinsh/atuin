#![cfg(feature = "toml")]

extern crate config;

#[macro_use]
extern crate serde_derive;

use config::*;
use std::path::PathBuf;

fn make() -> Config {
    let mut c = Config::default();
    c.merge(File::new("tests/Settings", FileFormat::Toml))
        .unwrap();

    c
}

#[test]
fn test_error_parse() {
    let mut c = Config::default();
    let res = c.merge(File::new("tests/Settings-invalid", FileFormat::Toml));

    let path: PathBuf = ["tests", "Settings-invalid.toml"].iter().collect();

    assert!(res.is_err());
    assert_eq!(
        res.unwrap_err().to_string(),
        format!(
            "invalid TOML value, did you mean to use a quoted string? at line 2 column 9 in {}",
            path.display()
        )
    );
}

#[test]
fn test_error_type() {
    let c = make();

    let res = c.get::<bool>("boolean_s_parse");

    let path: PathBuf = ["tests", "Settings.toml"].iter().collect();

    assert!(res.is_err());
    assert_eq!(
        res.unwrap_err().to_string(),
        format!(
            "invalid type: string \"fals\", expected a boolean for key `boolean_s_parse` in {}",
            path.display()
        )
    );
}

#[test]
fn test_error_type_detached() {
    let c = make();

    let value = c.get::<Value>("boolean_s_parse").unwrap();
    let res = value.try_into::<bool>();

    assert!(res.is_err());
    assert_eq!(
        res.unwrap_err().to_string(),
        "invalid type: string \"fals\", expected a boolean".to_string()
    );
}

#[test]
fn test_error_enum_de() {
    #[derive(Debug, Deserialize, PartialEq)]
    enum Diode {
        Off,
        Brightness(i32),
        Blinking(i32, i32),
        Pattern { name: String, inifinite: bool },
    }

    let on_v: Value = "on".into();
    let on_d = on_v.try_into::<Diode>();
    assert_eq!(
        on_d.unwrap_err().to_string(),
        "enum Diode does not have variant constructor on".to_string()
    );

    let array_v: Value = vec![100, 100].into();
    let array_d = array_v.try_into::<Diode>();
    assert_eq!(
        array_d.unwrap_err().to_string(),
        "value of enum Diode should be represented by either string or table with exactly one key"
    );

    let confused_v: Value = [
        ("Brightness".to_string(), 100.into()),
        ("Blinking".to_string(), vec![300, 700].into()),
    ]
    .iter()
    .cloned()
    .collect::<std::collections::HashMap<String, Value>>()
    .into();
    let confused_d = confused_v.try_into::<Diode>();
    assert_eq!(
        confused_d.unwrap_err().to_string(),
        "value of enum Diode should be represented by either string or table with exactly one key"
    );
}

#[test]
fn error_with_path() {
    #[derive(Debug, Deserialize)]
    struct Inner {
        test: i32,
    }

    #[derive(Debug, Deserialize)]
    struct Outer {
        inner: Inner,
    }
    const CFG: &str = r#"
inner:
    test: ABC
"#;

    let mut cfg = Config::new();
    cfg.merge(File::from_str(CFG, FileFormat::Yaml)).unwrap();
    let e = cfg.try_into::<Outer>().unwrap_err();
    if let ConfigError::Type {
        key: Some(path), ..
    } = e
    {
        assert_eq!(path, "inner.test");
    } else {
        panic!("Wrong error {:?}", e);
    }
}
