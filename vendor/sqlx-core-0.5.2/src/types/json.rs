use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue as JsonRawValue;
use serde_json::Value as JsonValue;

use crate::database::{Database, HasArguments, HasValueRef};
use crate::decode::Decode;
use crate::encode::{Encode, IsNull};
use crate::error::BoxDynError;
use crate::types::Type;

#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Json<T: ?Sized>(pub T);

impl<T> Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Json<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> AsRef<T> for Json<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl<T> AsMut<T> for Json<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<DB> Type<DB> for JsonValue
where
    Json<Self>: Type<DB>,
    DB: Database,
{
    fn type_info() -> DB::TypeInfo {
        <Json<Self> as Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <Json<Self> as Type<DB>>::compatible(ty)
    }
}

impl<DB> Type<DB> for Vec<JsonValue>
where
    Vec<Json<JsonValue>>: Type<DB>,
    DB: Database,
{
    fn type_info() -> DB::TypeInfo {
        <Vec<Json<JsonValue>> as Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <Vec<Json<JsonValue>> as Type<DB>>::compatible(ty)
    }
}

impl<DB> Type<DB> for [JsonValue]
where
    [Json<JsonValue>]: Type<DB>,
    DB: Database,
{
    fn type_info() -> DB::TypeInfo {
        <[Json<JsonValue>] as Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <[Json<JsonValue>] as Type<DB>>::compatible(ty)
    }
}

impl<'q, DB> Encode<'q, DB> for JsonValue
where
    for<'a> Json<&'a Self>: Encode<'q, DB>,
    DB: Database,
{
    fn encode_by_ref(&self, buf: &mut <DB as HasArguments<'q>>::ArgumentBuffer) -> IsNull {
        <Json<&Self> as Encode<'q, DB>>::encode(Json(self), buf)
    }
}

impl<'r, DB> Decode<'r, DB> for JsonValue
where
    Json<Self>: Decode<'r, DB>,
    DB: Database,
{
    fn decode(value: <DB as HasValueRef<'r>>::ValueRef) -> Result<Self, BoxDynError> {
        <Json<Self> as Decode<DB>>::decode(value).map(|item| item.0)
    }
}

impl<DB> Type<DB> for JsonRawValue
where
    for<'a> Json<&'a Self>: Type<DB>,
    DB: Database,
{
    fn type_info() -> DB::TypeInfo {
        <Json<&Self> as Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <Json<&Self> as Type<DB>>::compatible(ty)
    }
}

// We don't have to implement Encode for JsonRawValue because that's covered by the default
// implementation for Encode
impl<'r, DB> Decode<'r, DB> for &'r JsonRawValue
where
    Json<Self>: Decode<'r, DB>,
    DB: Database,
{
    fn decode(value: <DB as HasValueRef<'r>>::ValueRef) -> Result<Self, BoxDynError> {
        <Json<Self> as Decode<DB>>::decode(value).map(|item| item.0)
    }
}
