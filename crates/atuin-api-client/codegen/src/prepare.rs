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

use crate::mapping::SpecError;

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

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::{Value, json};

    use super::strip;

    fn spec(parameters: &Value) -> Value {
        json!({
            "components": {
                "parameters": {
                    "Known": {"in": "header", "name": "x-atuin-capabilities-known", "schema": {"type": "string"}},
                    "Other": {"in": "header", "name": "x-other", "schema": {"type": "string"}},
                },
            },
            "paths": {
                "/api/v0/me": {
                    "get": {
                        "operationId": "getMe",
                        "parameters": parameters,
                        "responses": {"200": {"description": "ok"}, "401": {"description": "no"}, "412": {"description": "stale"}},
                    },
                },
            },
        })
    }

    #[rstest]
    #[case::by_ref(json!([{"$ref": "#/components/parameters/Known"}]), json!([]))]
    #[case::inline(json!([{"in": "header", "name": "X-Atuin-Capabilities-Known"}]), json!([]))]
    #[case::other_header_kept(
        json!([{"$ref": "#/components/parameters/Other"}]),
        json!([{"$ref": "#/components/parameters/Other"}])
    )]
    #[case::query_of_the_same_name_kept(
        json!([{"in": "query", "name": "x-atuin-capabilities-known"}]),
        json!([{"in": "query", "name": "x-atuin-capabilities-known"}])
    )]
    fn strips_only_the_capabilities_known_header(#[case] parameters: Value, #[case] kept: Value) {
        let mut spec = spec(&parameters);
        strip(&mut spec).expect("the spec has paths");
        assert_eq!(spec.pointer("/paths/~1api~1v0~1me/get/parameters"), Some(&kept));
    }

    #[rstest]
    #[case::bounded(
        json!({"type": "integer", "format": "int64", "minimum": 1, "maximum": 536_870_912}),
        json!({"type": "integer", "format": "int64", "minimum": 0})
    )]
    #[case::from_zero_with_maximum(
        json!({"type": "integer", "format": "int64", "minimum": 0, "maximum": 10}),
        json!({"type": "integer", "format": "int64", "minimum": 0})
    )]
    #[case::signed_kept(
        json!({"type": "integer", "format": "int64", "minimum": -1, "maximum": 10}),
        json!({"type": "integer", "format": "int64", "minimum": -1, "maximum": 10})
    )]
    #[case::unbounded_kept(
        json!({"type": "integer", "format": "int64"}),
        json!({"type": "integer", "format": "int64"})
    )]
    fn a_non_negative_int64_keeps_only_its_sign(#[case] schema: Value, #[case] loosened: Value) {
        let mut spec = spec(&json!([]));
        spec["components"]["schemas"] = json!({"Size": {"properties": {"bytes": schema}}});
        strip(&mut spec).expect("the spec has paths");
        assert_eq!(spec.pointer("/components/schemas/Size/properties/bytes"), Some(&loosened));
    }

    #[rstest]
    fn keeps_only_2xx_responses() {
        let mut spec = spec(&json!([]));
        strip(&mut spec).expect("the spec has paths");
        assert_eq!(
            spec.pointer("/paths/~1api~1v0~1me/get/responses"),
            Some(&json!({"200": {"description": "ok"}}))
        );
    }
}
