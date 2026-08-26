use serde::Serialize;
use serde_json::Value;

use super::{Capability, CapsBundle, DuplicateCapability};

/// The result of comparing a client's echoed capability token against the server's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Negotiation {
    /// The client's token matches, or was absent -- serve the request.
    Current,
    /// The client presented a *differing* token -- its cached capabilities are stale.
    Stale,
}

/// The set of capabilities a server advertises. Build it with [`new`](Self::new) and
/// [`add`](Self::add)/[`add_many`](Self::add_many), then thread it as an [`Arc`].
///
/// [`Arc`]: std::sync::Arc
#[derive(Debug)]
pub struct CapServer {
    /// Opaque version token (xxh3 of the canonical capability set), rebaked on every mutation.
    token: String,
    /// Pre-serialized capabilities document (a `CapabilitiesResponse` as JSON), rebaked with it.
    body: String,
    /// The advertised capabilities, exposed for typed introspection via `caps`.
    caps: CapsBundle,
}

impl CapServer {
    /// Create an empty capability server. It advertises nothing until you [`add`](Self::add) to it,
    /// but still carries a stable token for the empty set.
    #[must_use]
    pub fn new() -> Self {
        let mut server = Self {
            token: String::new(),
            body: String::new(),
            caps: CapsBundle::default(),
        };
        server.bake();
        server
    }

    /// Advertise a capability, then re-bake.
    ///
    /// Errors with [`DuplicateCapability`] if a capability with the same name is already advertised.
    #[allow(clippy::should_implement_trait)]
    pub fn add<C: Capability>(mut self, cap: C) -> Result<Self, DuplicateCapability> {
        self.caps.add(cap)?;
        self.bake();
        Ok(self)
    }

    /// Advertise every capability the iterator yields, re-baking once for the whole batch.
    ///
    /// Errors with [`DuplicateCapability`] on the first name that is already advertised -- whether
    /// from an earlier call or earlier in the same batch.
    pub fn add_many(
        mut self,
        caps: impl IntoIterator<Item = Box<dyn Capability>>,
    ) -> Result<Self, DuplicateCapability> {
        for cap in caps {
            self.caps.add_dyn(cap)?;
        }
        self.bake();
        Ok(self)
    }

    /// Recompute the version token and pre-serialized document from the current capability set.
    fn bake(&mut self) {
        // `to_wire` emits keys in sorted order, so the canonical bytes -- and thus the token -- are
        // byte-identical on every node running the same capability set.
        let wire = self.caps.to_wire();
        let canonical = serde_json::to_vec(&wire).expect("capability map serializes");
        let token = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&canonical));

        #[derive(Serialize)]
        struct Wire<'a> {
            version: &'a str,
            capabilities: &'a Value,
        }
        let body = serde_json::to_string(&Wire {
            version: &token,
            capabilities: &wire,
        })
        .expect("capabilities document serializes");

        self.token = token;
        self.body = body;
    }

    /// The opaque version token this server advertises. Stable for a given capability set; the
    /// client echoes it back verbatim and never interprets it.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The pre-serialized capabilities document, served verbatim by the capabilities endpoint.
    /// Deserializes into a [`crate::api::CapabilitiesResponse`].
    pub fn body(&self) -> &str {
        &self.body
    }

    /// The capabilities this server advertises, for typed introspection.
    pub fn caps(&self) -> &CapsBundle {
        &self.caps
    }

    /// Decide whether a request whose client echoed `known` is current.
    ///
    /// Absent (`None`) or matching tokens are [`Negotiation::Current`]; only a present *differing*
    /// token is [`Negotiation::Stale`]. A client that sends no token is therefore never rejected.
    pub fn negotiate(&self, known: Option<&str>) -> Negotiation {
        match known {
            Some(known) if known != self.token => Negotiation::Stale,
            _ => Negotiation::Current,
        }
    }
}

impl Default for CapServer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::api::CapabilitiesResponse;
    use crate::caps::{CapabilitiesCap, Capability};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestCap {
        n: u32,
    }
    impl Capability for TestCap {
        fn static_name() -> &'static str {
            "test/cap"
        }

        fn name(&self) -> &'static str {
            Self::static_name()
        }

        fn json(&self) -> Result<serde_json::Value, serde_json::Error> {
            serde_json::to_value(self)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct OtherCap {
        m: u32,
    }
    impl Capability for OtherCap {
        fn static_name() -> &'static str {
            "test/other"
        }

        fn name(&self) -> &'static str {
            Self::static_name()
        }

        fn json(&self) -> Result<serde_json::Value, serde_json::Error> {
            serde_json::to_value(self)
        }
    }

    /// An empty capability server -- advertises nothing, but still carries a stable token.
    #[fixture]
    fn empty() -> CapServer {
        CapServer::new()
    }

    #[rstest]
    fn empty_server_advertises_nothing(empty: CapServer) {
        assert!(empty.caps().get::<TestCap>().is_none());
        assert!(empty.caps().get::<CapabilitiesCap>().is_none());
    }

    #[rstest]
    fn token_is_16_char_lowercase_hex(empty: CapServer) {
        let token = empty.token();
        assert_eq!(token.len(), 16);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[rstest]
    fn token_is_stable_for_the_same_set_and_changes_when_it_changes(empty: CapServer) {
        assert_eq!(empty.token(), CapServer::new().token());

        let with_cap = CapServer::new().add(TestCap { n: 1 }).unwrap();
        assert_ne!(empty.token(), with_cap.token());
        assert!(with_cap.caps().get::<TestCap>().is_some());
    }

    #[rstest]
    fn body_deserializes_into_the_client_response_shape(empty: CapServer) {
        let resp: CapabilitiesResponse = serde_json::from_str(empty.body()).unwrap();
        assert_eq!(resp.version, empty.token());
        assert!(resp.capabilities.is_empty());
    }

    #[rstest]
    #[case::zero(0)]
    #[case::small(7)]
    #[case::max(u32::MAX)]
    fn body_with_a_capability_round_trips_into_the_client_response_shape(#[case] n: u32) {
        let caps = CapServer::new().add(TestCap { n }).unwrap();
        let resp: CapabilitiesResponse = serde_json::from_str(caps.body()).unwrap();
        assert_eq!(resp.version, caps.token());
        assert_eq!(resp.capabilities.get("test/cap"), Some(&serde_json::json!({ "n": n })));
    }

    #[rstest]
    fn add_many_advertises_every_capability_and_matches_chained_adds() {
        let batch: Vec<Box<dyn Capability>> =
            vec![Box::new(TestCap { n: 1 }), Box::new(OtherCap { m: 2 })];
        let many = CapServer::new().add_many(batch).unwrap();

        // Every distinct capability in the batch is advertised.
        assert_eq!(many.caps().get::<TestCap>().unwrap().n, 1);
        assert_eq!(many.caps().get::<OtherCap>().unwrap().m, 2);

        // A batch bakes to the same set -- and therefore the same token -- as chained `add`s.
        let chained =
            CapServer::new().add(TestCap { n: 1 }).unwrap().add(OtherCap { m: 2 }).unwrap();
        assert_eq!(many.token(), chained.token());
        assert_eq!(many.body(), chained.body());

        // And it is not the empty token.
        assert_ne!(many.token(), CapServer::new().token());
    }

    #[rstest]
    fn add_many_rejects_a_repeated_name() {
        let batch: Vec<Box<dyn Capability>> =
            vec![Box::new(TestCap { n: 1 }), Box::new(TestCap { n: 9 })];
        let err = CapServer::new().add_many(batch).unwrap_err();
        assert_eq!(err.name, "test/cap");
    }

    #[rstest]
    fn add_rejects_a_duplicate_name() {
        let err =
            CapServer::new().add(TestCap { n: 1 }).unwrap().add(TestCap { n: 2 }).unwrap_err();
        assert_eq!(err.name, "test/cap");
    }

    /// How the client's echoed token relates to the server's. The matching token is only known
    /// once the server is built, so it is resolved against the fixture at run time.
    enum Known {
        Absent,
        Matching,
        Differing(&'static str),
    }

    #[rstest]
    #[case::absent(Known::Absent, Negotiation::Current)]
    #[case::matching(Known::Matching, Negotiation::Current)]
    #[case::differing(Known::Differing("deadbeefdeadbeef"), Negotiation::Stale)]
    fn negotiate_only_rejects_a_present_differing_token(
        empty: CapServer,
        #[case] known: Known,
        #[case] expected: Negotiation,
    ) {
        let known = match known {
            Known::Absent => None,
            Known::Matching => Some(empty.token().to_owned()),
            Known::Differing(token) => Some(token.to_owned()),
        };
        assert_eq!(empty.negotiate(known.as_deref()), expected);
    }
}
