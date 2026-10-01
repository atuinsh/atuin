//! Rewrites the hub's spec into the shape the client is generated from.
//!
//! Every non-2xx response goes. Self-hosted servers, proxies and older hubs send error bodies that
//! do not match the hub's schemas, and progenitor turns a documented error whose body fails to
//! parse into an error without its status. Undocumented, every failure reaches the hand-written
//! `ApiError` as the raw response, status included.
//!
//! The `x-atuin-capabilities-known` header parameter goes too: the client's hooks stamp it on
//! every negotiated call, so no caller passes it.
//!
//! A non-negative `int64` loses its other bounds (`minimum: 1`, `maximum`): the server enforces
//! them, and the client's `u64` holds every value it accepts.

use serde_json::{Map, Value};

/// Errors returned by [`strip`].
#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error("the spec has no {0} object")]
    Missing(&'static str),
    #[error("{0} is not an object")]
    NotAnObject(String),
}

const METHODS: [&str; 8] = ["get", "put", "post", "delete", "options", "head", "patch", "trace"];
const CAPABILITIES_KNOWN: &str = "x-atuin-capabilities-known";
const PARAMETER_REF_PREFIX: &str = "#/components/parameters/";

/// Strip what the generated client must not see; see the module docs.
pub fn strip(spec: &mut Value) -> Result<(), SpecError> {
    loosen_unsigned_bounds(spec);
    let shared = spec.pointer("/components/parameters").cloned().unwrap_or_default();
    let paths =
        spec.get_mut("paths").and_then(Value::as_object_mut).ok_or(SpecError::Missing(".paths"))?;

    for (path, item) in paths {
        let item = item
            .as_object_mut()
            .ok_or_else(|| SpecError::NotAnObject(format!(".paths[{path:?}]")))?;
        strip_capabilities_known(item, &shared);
        for method in METHODS {
            let Some(operation) = item.get_mut(method).and_then(Value::as_object_mut) else {
                continue;
            };
            strip_capabilities_known(operation, &shared);
            if let Some(responses) = operation.get_mut("responses").and_then(Value::as_object_mut) {
                responses.retain(|status, _| status.starts_with('2'));
            }
        }
    }
    Ok(())
}

fn loosen_unsigned_bounds(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if is_unsigned_int64(object) {
                object.insert("minimum".to_owned(), Value::from(0));
                object.remove("maximum");
            }
            object.values_mut().for_each(loosen_unsigned_bounds);
        }
        Value::Array(items) => items.iter_mut().for_each(loosen_unsigned_bounds),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn is_unsigned_int64(schema: &Map<String, Value>) -> bool {
    schema.get("format").and_then(Value::as_str) == Some("int64")
        && schema.get("minimum").and_then(Value::as_i64).is_some_and(|minimum| minimum >= 0)
}

fn strip_capabilities_known(owner: &mut Map<String, Value>, shared: &Value) {
    let Some(parameters) = owner.get_mut("parameters").and_then(Value::as_array_mut) else {
        return;
    };
    parameters.retain(|parameter| !is_capabilities_known(resolve(parameter, shared)));
}

/// Follow a `$ref` into `components.parameters`, or return an inline parameter as is.
fn resolve<'a>(parameter: &'a Value, shared: &'a Value) -> &'a Value {
    parameter
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix(PARAMETER_REF_PREFIX))
        .and_then(|name| shared.get(name))
        .unwrap_or(parameter)
}

fn is_capabilities_known(parameter: &Value) -> bool {
    parameter.get("in").and_then(Value::as_str) == Some("header")
        && parameter
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.eq_ignore_ascii_case(CAPABILITIES_KNOWN))
}
