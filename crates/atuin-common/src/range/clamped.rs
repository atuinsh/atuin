//! An integer held within a fixed range, with a default for when it is omitted.

use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;

use num_traits::PrimInt;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

/// The range a [`Clamped`] integer lives in, carried by a marker type so the bounds are part of
/// the field's type: `Clamped<PageSize>` rather than a bare `u32` that every reader must
/// re-validate.
pub trait Bounds {
    type Int: PrimInt + fmt::Debug + Serialize + DeserializeOwned;
    const MIN: Self::Int;
    const MAX: Self::Int;
    /// The value an omitted or `null` field takes. Clamped into `MIN..=MAX` like any other.
    const DEFAULT: Self::Int;
}

/// An integer that is always within `B::MIN..=B::MAX`.
///
/// Deserializing clamps out-of-range values instead of rejecting them, and `null` becomes
/// `B::DEFAULT` -- the lenient contract LLM tool parameters need, where a model sending
/// `limit: 0` or `limit: null` should not fail the whole call. Mark the field `#[serde(default)]`
/// so an omitted value becomes `B::DEFAULT` as well.
pub struct Clamped<B: Bounds>(B::Int, PhantomData<B>);

impl<B: Bounds> Clamped<B> {
    pub const MIN: B::Int = B::MIN;
    pub const MAX: B::Int = B::MAX;
    pub const DEFAULT: B::Int = B::DEFAULT;

    #[must_use]
    pub fn new(value: B::Int) -> Self {
        Self(value.clamp(B::MIN, B::MAX), PhantomData)
    }

    #[must_use]
    pub fn get(self) -> B::Int {
        self.0
    }
}

impl<B: Bounds> Default for Clamped<B> {
    fn default() -> Self {
        Self::new(B::DEFAULT)
    }
}

impl<'de, B: Bounds> Deserialize<'de> for Clamped<B> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Option::<B::Int>::deserialize(deserializer)?.map_or_else(Self::default, Self::new))
    }
}

/// The schema states the bounds and default, so a client (or a model reading a tool schema)
/// sees the same contract the deserializer enforces. Inlined: the name is not unique per `B`.
impl<B: Bounds> JsonSchema for Clamped<B> {
    fn schema_name() -> Cow<'static, str> {
        "Clamped".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "minimum": B::MIN,
            "maximum": B::MAX,
            "default": B::DEFAULT,
        })
    }
}

// Manual impls: deriving would demand the traits of the marker `B` too, which is only a name.
impl<B: Bounds> Clone for Clamped<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: Bounds> Copy for Clamped<B> {}

impl<B: Bounds> PartialEq for Clamped<B> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<B: Bounds> Eq for Clamped<B> {}

impl<B: Bounds> fmt::Debug for Clamped<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use schemars::JsonSchema;
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    struct PageSize;

    impl Bounds for PageSize {
        type Int = u32;
        const MIN: u32 = 1;
        const MAX: u32 = 20;
        const DEFAULT: u32 = 5;
    }

    struct Offset;

    impl Bounds for Offset {
        type Int = i8;
        const MIN: i8 = -10;
        const MAX: i8 = 10;
        const DEFAULT: i8 = 0;
    }

    #[derive(Deserialize, JsonSchema)]
    struct Params {
        #[serde(default)]
        limit: Clamped<PageSize>,
        #[serde(default)]
        offset: Clamped<Offset>,
    }

    #[rstest]
    #[case::missing(json!({}), 5)]
    #[case::null(json!({"limit": null}), 5)]
    #[case::in_range(json!({"limit": 7}), 7)]
    #[case::at_min(json!({"limit": 1}), 1)]
    #[case::below_min(json!({"limit": 0}), 1)]
    #[case::above_max(json!({"limit": 100}), 20)]
    fn deserializes_with_default_and_clamping(
        #[case] input: serde_json::Value,
        #[case] expected: u32,
    ) {
        let params: Params = serde_json::from_value(input).unwrap();
        assert_eq!(params.limit.get(), expected);
    }

    #[rstest]
    #[case::missing(json!({}), 0)]
    #[case::negative_in_range(json!({"offset": -3}), -3)]
    #[case::below_min(json!({"offset": -100}), -10)]
    fn works_for_signed_types(#[case] input: serde_json::Value, #[case] expected: i8) {
        let params: Params = serde_json::from_value(input).unwrap();
        assert_eq!(params.offset.get(), expected);
    }

    #[rstest]
    #[case::negative(json!({"limit": -1}))]
    #[case::string(json!({"limit": "5"}))]
    fn rejects_non_integers(#[case] input: serde_json::Value) {
        assert!(serde_json::from_value::<Params>(input).is_err());
    }

    #[rstest]
    fn exposes_its_bounds() {
        type Limit = Clamped<PageSize>;
        assert_eq!((Limit::MIN, Limit::MAX, Limit::DEFAULT), (1, 20, 5));
        assert_eq!(Limit::default().get(), 5);
        assert_eq!(Limit::new(0).get(), 1);
        assert_eq!(format!("{:?}", Limit::new(7)), "7");
    }

    #[rstest]
    fn json_schema_carries_the_bounds() {
        let schema = schemars::schema_for!(Params);
        let limit = &schema.as_value()["properties"]["limit"];
        assert_eq!(limit["type"], "integer");
        assert_eq!(limit["minimum"], 1);
        assert_eq!(limit["maximum"], 20);
        assert_eq!(limit["default"], 5);
        assert_eq!(schema.as_value()["properties"]["offset"]["minimum"], -10);
        // Both fields have defaults, so neither is required.
        assert_eq!(schema.as_value().get("required"), None);
    }
}
