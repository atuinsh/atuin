//! JSON Schemas of config files.

use std::collections::HashSet;

use serde_json::{Map, Value};

/// `schema`, with each property that `defaults` sets marked with that value as its `default`,
/// over any `default` the schema already had. A file may leave out a setting with a default, so
/// such a setting is no longer `required`.
///
/// `defaults` mirrors the config file: an object per table. A table whose schema is a `$ref`
/// into `$defs` marks that definition. A second table that refers to the same definition gets
/// its own copy of it, so each table keeps its own defaults.
#[must_use]
pub fn with_defaults(schema: schemars::Schema, defaults: &Value) -> Value {
    let mut schema = schema.to_value();
    let definitions = schema.get("$defs").cloned().unwrap_or_default();
    let mut marked = HashSet::new();
    let mut pending = vec![(String::new(), defaults.clone())];
    while let Some((pointer, defaults)) = pending.pop() {
        let (Some(node), Value::Object(defaults)) = (schema.pointer_mut(&pointer), defaults) else {
            continue;
        };
        unrequire(node, &defaults);
        let Some(properties) = node.get_mut("properties").and_then(Value::as_object_mut) else {
            continue;
        };
        pending.extend(mark(properties, &defaults, &pointer, &definitions, &mut marked));
    }
    schema
}

/// Drop each setting that `defaults` sets from the `required` of `node`, the table's schema.
fn unrequire(node: &mut Value, defaults: &Map<String, Value>) {
    let Value::Object(node) = node else {
        return;
    };
    let Some(Value::Array(required)) = node.get_mut("required") else {
        return;
    };
    required.retain(|name| name.as_str().is_none_or(|name| !defaults.contains_key(name)));
    if required.is_empty() {
        node.remove("required");
    }
}

/// Mark each of `properties`, those of the table at `pointer`, with its value in `defaults`, and
/// return the tables to mark next, by JSON pointer.
///
/// `definitions` are the schema's definitions before any marking, and `marked` names those a
/// table has marked already.
fn mark(
    properties: &mut Map<String, Value>,
    defaults: &Map<String, Value>,
    pointer: &str,
    definitions: &Value,
    marked: &mut HashSet<String>,
) -> Vec<(String, Value)> {
    let mut tables = Vec::new();
    for (key, property) in properties {
        let Some(default) = defaults.get(key) else {
            continue;
        };
        let definition = property
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix("#/$defs/"))
            .filter(|_| default.is_object())
            .map(str::to_owned);
        let Some(name) = definition else {
            if let Value::Object(property) = property {
                property.insert("default".to_owned(), default.clone());
            }
            continue;
        };
        let shared = !marked.insert(name.clone());
        if shared {
            inline(property, &definitions[name.as_str()]);
        }
        // The table's own `default` is its type's `Default`, which can contradict the built-in
        // defaults that its settings carry from here on.
        if let Value::Object(property) = property {
            property.remove("default");
        }
        let table = if shared {
            format!("{pointer}/properties/{}", escape(key))
        } else {
            format!("/$defs/{}", escape(&name))
        };
        tables.push((table, default.clone()));
    }
    tables
}

/// Replace `property`'s `$ref` with a copy of `definition`, keeping the property's own keywords,
/// such as its `description`.
fn inline(property: &mut Value, definition: &Value) {
    let (Value::Object(property), Value::Object(definition)) = (property, definition) else {
        return;
    };
    property.remove("$ref");
    for (key, value) in definition {
        property.entry(key.clone()).or_insert_with(|| value.clone());
    }
}

/// `token` as one segment of a JSON pointer.
fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use schemars::JsonSchema;
    use serde_json::json;

    use super::*;

    #[derive(Default, JsonSchema, serde::Serialize)]
    struct Inner {
        port: u16,
    }

    /// A table whose own `default` is the `Default` of its type, as `#[serde(default)]` gives it.
    #[derive(JsonSchema)]
    #[expect(dead_code, reason = "only its schema is read")]
    struct Defaulted {
        #[serde(default)]
        metrics: Inner,
    }

    #[derive(JsonSchema)]
    #[expect(dead_code, reason = "only its schema is read")]
    struct Outer {
        host: String,
        metrics: Inner,
        unset: bool,
    }

    #[derive(JsonSchema)]
    #[expect(dead_code, reason = "only its schema is read")]
    struct Shared {
        first: Inner,
        second: Inner,
    }

    /// The schema of the table `key`, followed through a `$ref` if it has one.
    fn table<'a>(schema: &'a Value, key: &str) -> &'a Value {
        let property = &schema["properties"][key];
        match property["$ref"].as_str().and_then(|r| r.strip_prefix("#/$defs/")) {
            Some(name) => &schema["$defs"][name],
            None => property,
        }
    }

    #[rstest]
    fn marks_top_level_and_nested_tables() {
        let schema = with_defaults(
            schemars::schema_for!(Outer),
            &json!({ "host": "127.0.0.1", "metrics": { "port": 9001 } }),
        );
        assert_eq!(schema["properties"]["host"]["default"], "127.0.0.1");
        assert_eq!(schema["$defs"]["Inner"]["properties"]["port"]["default"], 9001);
        assert_eq!(schema["properties"]["unset"].get("default"), None);
    }

    #[rstest]
    fn gives_tables_sharing_a_definition_their_own_defaults() {
        let schema = with_defaults(
            schemars::schema_for!(Shared),
            &json!({ "first": { "port": 1 }, "second": { "port": 2 } }),
        );
        assert_eq!(table(&schema, "first")["properties"]["port"]["default"], 1);
        assert_eq!(table(&schema, "second")["properties"]["port"]["default"], 2);
    }

    #[rstest]
    fn settings_with_defaults_are_no_longer_required() {
        let schema = with_defaults(
            schemars::schema_for!(Outer),
            &json!({ "host": "127.0.0.1", "metrics": { "port": 9001 } }),
        );
        assert_eq!(schema["required"], json!(["unset"]));
        assert_eq!(schema["$defs"]["Inner"].get("required"), None);
    }

    #[rstest]
    fn a_table_drops_the_default_of_its_type() {
        let before = schemars::schema_for!(Defaulted).to_value();
        assert_eq!(before["properties"]["metrics"]["default"], json!({ "port": 0 }));

        let schema = with_defaults(
            schemars::schema_for!(Defaulted),
            &json!({ "metrics": { "port": 9001 } }),
        );
        assert_eq!(schema["properties"]["metrics"].get("default"), None);
        assert_eq!(table(&schema, "metrics")["properties"]["port"]["default"], 9001);
    }
}
