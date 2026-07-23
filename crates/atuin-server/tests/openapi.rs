//! Guards `crates/atuin-client/openapi.json`, which is generated from this
//! crate and is the input to the progenitor-generated `atuin-api-client`.

use std::collections::BTreeSet;

const COMMITTED: &str = include_str!("../../atuin-client/openapi.json");
const ROUTER_SRC: &str = include_str!("../src/router.rs");

const REGENERATE: &str =
    "cargo run -p atuin-server --bin atuin-server -- openapi > crates/atuin-client/openapi.json";

/// The committed spec must be exactly what the server generates. Without this,
/// a wire type could change while the generated client kept the old shape.
#[test]
fn committed_spec_is_up_to_date() {
    let generated = atuin_server::openapi::spec();

    assert!(
        generated == COMMITTED,
        "crates/atuin-client/openapi.json is out of date.\n\
         Regenerate it, then regenerate the client per crates/atuin-api-client/README.md:\n  {REGENERATE}"
    );
}

/// `ApiDoc` lists its operations by hand, so a new route in `router.rs` can
/// silently miss the spec (and therefore the generated client). Catch that.
#[test]
fn every_route_is_documented() {
    let documented = documented_operations();
    let routed = routed_operations();

    assert!(
        !routed.is_empty(),
        "failed to parse any routes out of router.rs -- this test needs updating"
    );

    let undocumented: Vec<_> = routed.difference(&documented).collect();
    assert!(
        undocumented.is_empty(),
        "routes in router.rs are missing from the OpenAPI document: {undocumented:?}\n\
         Add a #[utoipa::path(..)] to the handler and list it in paths(..) in src/openapi.rs, then:\n  {REGENERATE}"
    );

    let unrouted: Vec<_> = documented.difference(&routed).collect();
    assert!(
        unrouted.is_empty(),
        "the OpenAPI document describes operations the server does not route: {unrouted:?}"
    );
}

/// (method, path) pairs described by the committed spec.
fn documented_operations() -> BTreeSet<(String, String)> {
    let spec: serde_json::Value = serde_json::from_str(COMMITTED).expect("spec is valid JSON");

    spec["paths"]
        .as_object()
        .expect("spec has paths")
        .iter()
        .flat_map(|(path, item)| {
            item.as_object()
                .expect("path item is an object")
                .keys()
                .map(move |method| (method.to_lowercase(), path.clone()))
        })
        .collect()
}

/// (method, path) pairs the router actually serves, scraped from its source.
/// Crude, but axum exposes no way to enumerate a `Router`'s routes.
fn routed_operations() -> BTreeSet<(String, String)> {
    const METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];

    let mut routes = BTreeSet::new();

    for line in ROUTER_SRC.lines() {
        let Some((_, after_call)) = line.split_once(".route(\"") else {
            continue;
        };
        let Some((path, handlers)) = after_call.split_once('"') else {
            continue;
        };

        for method in METHODS {
            if handlers.contains(&format!("{method}(")) {
                routes.insert((method.to_string(), path.to_string()));
            }
        }
    }

    routes
}
