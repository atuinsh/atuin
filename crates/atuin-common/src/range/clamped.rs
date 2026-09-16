//! An integer held within a fixed range, with a default for when it is omitted.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

mod sealed {
    pub trait Sealed {}
}

/// The integer types [`Clamped`] can hold.
///
/// A const generic cannot be typed by another generic parameter on stable Rust, so the bounds
/// are written as `i128` literals and converted to the wrapped type at runtime; `MIN_VALUE` and
/// `MAX_VALUE` let [`Clamped`] check at compile time that they fit. Sealed to the primitive
/// integers (all but `u128`, whose upper half does not fit an `i128`).
pub trait ClampInt:
    Copy + Ord + fmt::Debug + Serialize + DeserializeOwned + sealed::Sealed
{
    const MIN_VALUE: i128;
    const MAX_VALUE: i128;
    /// Lossless for any `value` within `MIN_VALUE..=MAX_VALUE`.
    fn from_i128(value: i128) -> Self;
}

macro_rules! clamp_int {
    ($($t:ty),*) => {$(
        impl sealed::Sealed for $t {}

        #[allow(
            clippy::cast_lossless,
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            clippy::cast_sign_loss,
            clippy::unnecessary_cast,
            reason = "widening to i128 in a const context has no From, and narrowing is \
                      guarded by the compile-time bounds check"
        )]
        impl ClampInt for $t {
            const MIN_VALUE: i128 = <$t>::MIN as i128;
            const MAX_VALUE: i128 = <$t>::MAX as i128;

            fn from_i128(value: i128) -> Self {
                value as Self
            }
        }
    )*};
}

clamp_int!(u8, u16, u32, u64, usize, i8, i16, i32, i64, i128, isize);

/// An integer that is always within `MIN..=MAX`, e.g. `Clamped<u32, 1, 20, 5>`.
///
/// Deserializing clamps out-of-range values instead of rejecting them, and `null` becomes
/// `DEFAULT` -- the lenient contract LLM tool parameters need, where a model sending `limit: 0`
/// or `limit: null` should not fail the whole call. Mark the field `#[serde(default)]` so an
/// omitted value becomes `DEFAULT` as well.
///
/// The bounds are checked when the type is used: `MIN <= DEFAULT <= MAX`, all within `T`.
///
/// ```compile_fail
/// # use atuin_common::range::Clamped;
/// let _ = Clamped::<u8, 0, 300, 5>::default();
/// ```
/// ```compile_fail
/// # use atuin_common::range::Clamped;
/// let _ = Clamped::<u32, 1, 20, 50>::default();
/// ```
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Clamped<T: ClampInt, const MIN: i128, const MAX: i128, const DEFAULT: i128>(T);

impl<T: ClampInt, const MIN: i128, const MAX: i128, const DEFAULT: i128> fmt::Debug
    for Clamped<T, MIN, MAX, DEFAULT>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<T: ClampInt, const MIN: i128, const MAX: i128, const DEFAULT: i128>
    Clamped<T, MIN, MAX, DEFAULT>
{
    // Evaluated per instantiation, when `new` first forces it: an invalid range is a build
    // error at the use site rather than a silently clamped default.
    const VALID: () = assert!(
        T::MIN_VALUE <= MIN && MIN <= DEFAULT && DEFAULT <= MAX && MAX <= T::MAX_VALUE,
        "Clamped bounds must satisfy T::MIN <= MIN <= DEFAULT <= MAX <= T::MAX"
    );

    #[must_use]
    pub fn new(value: T) -> Self {
        let () = Self::VALID;
        Self(value.clamp(Self::min(), Self::max()))
    }

    #[must_use]
    pub fn get(self) -> T {
        self.0
    }

    #[must_use]
    pub fn min() -> T {
        T::from_i128(MIN)
    }

    #[must_use]
    pub fn max() -> T {
        T::from_i128(MAX)
    }

    #[must_use]
    pub fn default_value() -> T {
        T::from_i128(DEFAULT)
    }
}

impl<T: ClampInt, const MIN: i128, const MAX: i128, const DEFAULT: i128> Default
    for Clamped<T, MIN, MAX, DEFAULT>
{
    fn default() -> Self {
        Self::new(Self::default_value())
    }
}

impl<'de, T: ClampInt, const MIN: i128, const MAX: i128, const DEFAULT: i128> Deserialize<'de>
    for Clamped<T, MIN, MAX, DEFAULT>
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Option::<T>::deserialize(deserializer)?.map_or_else(Self::default, Self::new))
    }
}

/// The schema states the bounds and default, so a client (or a model reading a tool schema)
/// sees the same contract the deserializer enforces. Inlined: the name is not unique per
/// instantiation.
impl<T: ClampInt, const MIN: i128, const MAX: i128, const DEFAULT: i128> JsonSchema
    for Clamped<T, MIN, MAX, DEFAULT>
{
    fn schema_name() -> Cow<'static, str> {
        "Clamped".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "minimum": Self::min(),
            "maximum": Self::max(),
            "default": Self::default_value(),
        })
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use schemars::JsonSchema;
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Deserialize, JsonSchema)]
    struct Params {
        #[serde(default)]
        limit: Clamped<u32, 1, 20, 5>,
        #[serde(default)]
        offset: Clamped<i8, -10, 10, 0>,
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
        type Limit = Clamped<u32, 1, 20, 5>;
        assert_eq!((Limit::min(), Limit::max(), Limit::default_value()), (1, 20, 5));
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
