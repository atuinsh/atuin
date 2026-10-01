//! Turns the bytes of `openapi.json` into the client's Rust source.

use progenitor_impl::Generator;
use serde_json::Value;

use crate::mapping::{self, SpecError};
use crate::prepare;

/// Errors returned by [`generate`].
#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("openapi.json is not an OpenAPI 3.0 document: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Spec(#[from] SpecError),
    #[error("progenitor rejected the spec: {0}")]
    Progenitor(#[from] progenitor_impl::Error),
}

/// Generate the client's Rust source from the bytes of `openapi.json`.
pub fn generate(spec: &[u8]) -> Result<String, GenerateError> {
    let mut spec: Value = serde_json::from_slice(spec)?;
    prepare::strip(&mut spec)?;
    // The workspace's serde_json keeps keys in file order (`preserve_order`); sorting them keeps
    // the generated items in key order whatever order the hub writes them in.
    spec.sort_all_objects();
    mapping::check(&spec)?;

    let spec: openapiv3::OpenAPI = serde_json::from_value(spec)?;
    let tokens = Generator::new(&mapping::settings()).generate_tokens(&spec)?;
    Ok(tokens.to_string())
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};
    use serde_json::{Value, json};

    use super::generate;

    /// The hub's spec, as checked in next to the generated client.
    #[fixture]
    fn spec() -> Value {
        serde_json::from_str(include_str!("../openapi.json")).expect("openapi.json is JSON")
    }

    /// The code generated from `spec`, without whitespace, so a snippet matches however the
    /// tokens are spaced.
    fn generate_from(spec: &Value) -> String {
        generate(&serde_json::to_vec(spec).expect("a JSON value serializes"))
            .expect("the spec generates")
            .split_whitespace()
            .collect()
    }

    fn squeezed(snippet: &str) -> String {
        snippet.split_whitespace().collect()
    }

    fn set(spec: &mut Value, pointer: &str, value: Value) {
        *spec.pointer_mut(pointer).expect("the pointer names an existing value") = value;
    }

    #[rstest]
    fn a_nullable_secret_stays_a_secret(mut spec: Value) {
        set(
            &mut spec,
            "/components/schemas/LoginRequest/properties/password",
            json!({"type": "string", "format": "password", "nullable": true}),
        );
        let code = generate_from(&spec);
        assert!(code.contains(&squeezed("pub password: ::std::option::Option<crate::Secret>,")));
    }
}
