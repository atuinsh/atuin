use serde_hjson;
use source::Source;
use std::collections::HashMap;
use std::error::Error;
use value::{Value, ValueKind};

pub fn parse(
    uri: Option<&String>,
    text: &str,
) -> Result<HashMap<String, Value>, Box<dyn Error + Send + Sync>> {
    // Parse a JSON object value from the text
    // TODO: Have a proper error fire if the root of a file is ever not a Table
    let value = from_hjson_value(uri, &serde_hjson::from_str(text)?);
    match value.kind {
        ValueKind::Table(map) => Ok(map),

        _ => Ok(HashMap::new()),
    }
}

fn from_hjson_value(uri: Option<&String>, value: &serde_hjson::Value) -> Value {
    match *value {
        serde_hjson::Value::String(ref value) => Value::new(uri, ValueKind::String(value.clone())),

        serde_hjson::Value::I64(value) => Value::new(uri, ValueKind::Integer(value)),

        serde_hjson::Value::U64(value) => Value::new(uri, ValueKind::Integer(value as i64)),

        serde_hjson::Value::F64(value) => Value::new(uri, ValueKind::Float(value)),

        serde_hjson::Value::Bool(value) => Value::new(uri, ValueKind::Boolean(value)),

        serde_hjson::Value::Object(ref table) => {
            let mut m = HashMap::new();

            for (key, value) in table {
                m.insert(key.clone(), from_hjson_value(uri, value));
            }

            Value::new(uri, ValueKind::Table(m))
        }

        serde_hjson::Value::Array(ref array) => {
            let mut l = Vec::new();

            for value in array {
                l.push(from_hjson_value(uri, value));
            }

            Value::new(uri, ValueKind::Array(l))
        }

        serde_hjson::Value::Null => Value::new(uri, ValueKind::Nil),
    }
}
