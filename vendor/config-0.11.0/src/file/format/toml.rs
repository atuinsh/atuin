use source::Source;
use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use toml;
use value::{Value, ValueKind};

pub fn parse(
    uri: Option<&String>,
    text: &str,
) -> Result<HashMap<String, Value>, Box<dyn Error + Send + Sync>> {
    // Parse a TOML value from the provided text
    // TODO: Have a proper error fire if the root of a file is ever not a Table
    let value = from_toml_value(uri, &toml::from_str(text)?);
    match value.kind {
        ValueKind::Table(map) => Ok(map),

        _ => Ok(HashMap::new()),
    }
}

fn from_toml_value(uri: Option<&String>, value: &toml::Value) -> Value {
    match *value {
        toml::Value::String(ref value) => Value::new(uri, value.to_string()),
        toml::Value::Float(value) => Value::new(uri, value),
        toml::Value::Integer(value) => Value::new(uri, value),
        toml::Value::Boolean(value) => Value::new(uri, value),

        toml::Value::Table(ref table) => {
            let mut m = HashMap::new();

            for (key, value) in table {
                m.insert(key.clone(), from_toml_value(uri, value));
            }

            Value::new(uri, m)
        }

        toml::Value::Array(ref array) => {
            let mut l = Vec::new();

            for value in array {
                l.push(from_toml_value(uri, value));
            }

            Value::new(uri, l)
        }

        toml::Value::Datetime(ref datetime) => Value::new(uri, datetime.to_string()),
    }
}
