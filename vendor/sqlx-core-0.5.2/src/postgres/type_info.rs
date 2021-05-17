#![allow(dead_code)]

use std::fmt::{self, Display, Formatter};
use std::ops::Deref;
use std::sync::Arc;

use crate::ext::ustr::UStr;
use crate::type_info::TypeInfo;

/// Type information for a PostgreSQL type.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
pub struct PgTypeInfo(pub(crate) PgType);

impl Deref for PgTypeInfo {
    type Target = PgType;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
#[repr(u32)]
pub enum PgType {
    Bool,
    Bytea,
    Char,
    Name,
    Int8,
    Int2,
    Int4,
    Text,
    Oid,
    Json,
    JsonArray,
    Point,
    Lseg,
    Path,
    Box,
    Polygon,
    Line,
    LineArray,
    Cidr,
    CidrArray,
    Float4,
    Float8,
    Unknown,
    Circle,
    CircleArray,
    Macaddr8,
    Macaddr8Array,
    Macaddr,
    Inet,
    BoolArray,
    ByteaArray,
    CharArray,
    NameArray,
    Int2Array,
    Int4Array,
    TextArray,
    BpcharArray,
    VarcharArray,
    Int8Array,
    PointArray,
    LsegArray,
    PathArray,
    BoxArray,
    Float4Array,
    Float8Array,
    PolygonArray,
    OidArray,
    MacaddrArray,
    InetArray,
    Bpchar,
    Varchar,
    Date,
    Time,
    Timestamp,
    TimestampArray,
    DateArray,
    TimeArray,
    Timestamptz,
    TimestamptzArray,
    Interval,
    IntervalArray,
    NumericArray,
    Timetz,
    TimetzArray,
    Bit,
    BitArray,
    Varbit,
    VarbitArray,
    Numeric,
    Record,
    RecordArray,
    Uuid,
    UuidArray,
    Jsonb,
    JsonbArray,
    Int4Range,
    Int4RangeArray,
    NumRange,
    NumRangeArray,
    TsRange,
    TsRangeArray,
    TstzRange,
    TstzRangeArray,
    DateRange,
    DateRangeArray,
    Int8Range,
    Int8RangeArray,
    Jsonpath,
    JsonpathArray,
    Money,
    MoneyArray,

    // https://www.postgresql.org/docs/9.3/datatype-pseudo.html
    Void,

    // A realized user-defined type. When a connection sees a DeclareXX variant it resolves
    // into this one before passing it along to `accepts` or inside of `Value` objects.
    Custom(Arc<PgCustomType>),

    // From [`PgTypeInfo::with_name`]
    DeclareWithName(UStr),

    // NOTE: Do we want to bring back type declaration by ID? It's notoriously fragile but
    //       someone may have a user for it
    DeclareWithOid(u32),
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
pub struct PgCustomType {
    #[cfg_attr(feature = "offline", serde(skip))]
    pub(crate) oid: u32,
    pub(crate) name: UStr,
    pub(crate) kind: PgTypeKind,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
pub enum PgTypeKind {
    Simple,
    Pseudo,
    Domain(PgTypeInfo),
    Composite(Arc<[(String, PgTypeInfo)]>),
    Array(PgTypeInfo),
    Enum(Arc<[String]>),
    Range(PgTypeInfo),
}

impl PgTypeInfo {
    /// Returns the corresponding `PgTypeInfo` if the OID is a built-in type and recognized by SQLx.
    pub(crate) fn try_from_oid(oid: u32) -> Option<Self> {
        PgType::try_from_oid(oid).map(Self)
    }

    /// Returns the _kind_ (simple, array, enum, etc.) for this type.
    pub fn kind(&self) -> &PgTypeKind {
        self.0.kind()
    }

    #[doc(hidden)]
    pub fn __type_feature_gate(&self) -> Option<&'static str> {
        if [
            PgTypeInfo::DATE,
            PgTypeInfo::TIME,
            PgTypeInfo::TIMESTAMP,
            PgTypeInfo::TIMESTAMPTZ,
            PgTypeInfo::DATE_ARRAY,
            PgTypeInfo::TIME_ARRAY,
            PgTypeInfo::TIMESTAMP_ARRAY,
            PgTypeInfo::TIMESTAMPTZ_ARRAY,
        ]
        .contains(self)
        {
            Some("time")
        } else if [PgTypeInfo::UUID, PgTypeInfo::UUID_ARRAY].contains(self) {
            Some("uuid")
        } else if [
            PgTypeInfo::JSON,
            PgTypeInfo::JSONB,
            PgTypeInfo::JSON_ARRAY,
            PgTypeInfo::JSONB_ARRAY,
        ]
        .contains(self)
        {
            Some("json")
        } else if [
            PgTypeInfo::CIDR,
            PgTypeInfo::INET,
            PgTypeInfo::CIDR_ARRAY,
            PgTypeInfo::INET_ARRAY,
        ]
        .contains(self)
        {
            Some("ipnetwork")
        } else if [PgTypeInfo::NUMERIC, PgTypeInfo::NUMERIC_ARRAY].contains(self) {
            Some("bigdecimal")
        } else {
            None
        }
    }

    /// Create a `PgTypeInfo` from a type name.
    ///
    /// The OID for the type will be fetched from Postgres on use of
    /// a value of this type. The fetched OID will be cached per-connection.
    pub const fn with_name(name: &'static str) -> Self {
        Self(PgType::DeclareWithName(UStr::Static(name)))
    }

    /// Create a `PgTypeInfo` from an OID.
    ///
    /// Note that the OID for a type is very dependent on the environment. If you only ever use
    /// one database or if this is an unhandled build-in type, you should be fine. Otherwise,
    /// you will be better served using [`with_name`](Self::with_name).
    pub const fn with_oid(oid: u32) -> Self {
        Self(PgType::DeclareWithOid(oid))
    }
}

// DEVELOPER PRO TIP: find builtin type OIDs easily by grepping this file
// https://github.com/postgres/postgres/blob/master/src/include/catalog/pg_type.dat
//
// If you have Postgres running locally you can also try
// SELECT oid, typarray FROM pg_type where typname = '<type name>'

impl PgType {
    /// Returns the corresponding `PgType` if the OID is a built-in type and recognized by SQLx.
    pub(crate) fn try_from_oid(oid: u32) -> Option<Self> {
        Some(match oid {
            16 => PgType::Bool,
            17 => PgType::Bytea,
            18 => PgType::Char,
            19 => PgType::Name,
            20 => PgType::Int8,
            21 => PgType::Int2,
            23 => PgType::Int4,
            25 => PgType::Text,
            26 => PgType::Oid,
            114 => PgType::Json,
            199 => PgType::JsonArray,
            600 => PgType::Point,
            601 => PgType::Lseg,
            602 => PgType::Path,
            603 => PgType::Box,
            604 => PgType::Polygon,
            628 => PgType::Line,
            629 => PgType::LineArray,
            650 => PgType::Cidr,
            651 => PgType::CidrArray,
            700 => PgType::Float4,
            701 => PgType::Float8,
            705 => PgType::Unknown,
            718 => PgType::Circle,
            719 => PgType::CircleArray,
            774 => PgType::Macaddr8,
            775 => PgType::Macaddr8Array,
            790 => PgType::Money,
            791 => PgType::MoneyArray,
            829 => PgType::Macaddr,
            869 => PgType::Inet,
            1000 => PgType::BoolArray,
            1001 => PgType::ByteaArray,
            1002 => PgType::CharArray,
            1003 => PgType::NameArray,
            1005 => PgType::Int2Array,
            1007 => PgType::Int4Array,
            1009 => PgType::TextArray,
            1014 => PgType::BpcharArray,
            1015 => PgType::VarcharArray,
            1016 => PgType::Int8Array,
            1017 => PgType::PointArray,
            1018 => PgType::LsegArray,
            1019 => PgType::PathArray,
            1020 => PgType::BoxArray,
            1021 => PgType::Float4Array,
            1022 => PgType::Float8Array,
            1027 => PgType::PolygonArray,
            1028 => PgType::OidArray,
            1040 => PgType::MacaddrArray,
            1041 => PgType::InetArray,
            1042 => PgType::Bpchar,
            1043 => PgType::Varchar,
            1082 => PgType::Date,
            1083 => PgType::Time,
            1114 => PgType::Timestamp,
            1115 => PgType::TimestampArray,
            1182 => PgType::DateArray,
            1183 => PgType::TimeArray,
            1184 => PgType::Timestamptz,
            1185 => PgType::TimestamptzArray,
            1186 => PgType::Interval,
            1187 => PgType::IntervalArray,
            1231 => PgType::NumericArray,
            1266 => PgType::Timetz,
            1270 => PgType::TimetzArray,
            1560 => PgType::Bit,
            1561 => PgType::BitArray,
            1562 => PgType::Varbit,
            1563 => PgType::VarbitArray,
            1700 => PgType::Numeric,
            2278 => PgType::Void,
            2249 => PgType::Record,
            2287 => PgType::RecordArray,
            2950 => PgType::Uuid,
            2951 => PgType::UuidArray,
            3802 => PgType::Jsonb,
            3807 => PgType::JsonbArray,
            3904 => PgType::Int4Range,
            3905 => PgType::Int4RangeArray,
            3906 => PgType::NumRange,
            3907 => PgType::NumRangeArray,
            3908 => PgType::TsRange,
            3909 => PgType::TsRangeArray,
            3910 => PgType::TstzRange,
            3911 => PgType::TstzRangeArray,
            3912 => PgType::DateRange,
            3913 => PgType::DateRangeArray,
            3926 => PgType::Int8Range,
            3927 => PgType::Int8RangeArray,
            4072 => PgType::Jsonpath,
            4073 => PgType::JsonpathArray,

            _ => {
                return None;
            }
        })
    }

    pub(crate) fn oid(&self) -> u32 {
        match self.try_oid() {
            Some(oid) => oid,
            None => unreachable!("(bug) use of unresolved type declaration [oid]"),
        }
    }

    pub(crate) fn try_oid(&self) -> Option<u32> {
        Some(match self {
            PgType::Bool => 16,
            PgType::Bytea => 17,
            PgType::Char => 18,
            PgType::Name => 19,
            PgType::Int8 => 20,
            PgType::Int2 => 21,
            PgType::Int4 => 23,
            PgType::Text => 25,
            PgType::Oid => 26,
            PgType::Json => 114,
            PgType::JsonArray => 199,
            PgType::Point => 600,
            PgType::Lseg => 601,
            PgType::Path => 602,
            PgType::Box => 603,
            PgType::Polygon => 604,
            PgType::Line => 628,
            PgType::LineArray => 629,
            PgType::Cidr => 650,
            PgType::CidrArray => 651,
            PgType::Float4 => 700,
            PgType::Float8 => 701,
            PgType::Unknown => 705,
            PgType::Circle => 718,
            PgType::CircleArray => 719,
            PgType::Macaddr8 => 774,
            PgType::Macaddr8Array => 775,
            PgType::Money => 790,
            PgType::MoneyArray => 791,
            PgType::Macaddr => 829,
            PgType::Inet => 869,
            PgType::BoolArray => 1000,
            PgType::ByteaArray => 1001,
            PgType::CharArray => 1002,
            PgType::NameArray => 1003,
            PgType::Int2Array => 1005,
            PgType::Int4Array => 1007,
            PgType::TextArray => 1009,
            PgType::BpcharArray => 1014,
            PgType::VarcharArray => 1015,
            PgType::Int8Array => 1016,
            PgType::PointArray => 1017,
            PgType::LsegArray => 1018,
            PgType::PathArray => 1019,
            PgType::BoxArray => 1020,
            PgType::Float4Array => 1021,
            PgType::Float8Array => 1022,
            PgType::PolygonArray => 1027,
            PgType::OidArray => 1028,
            PgType::MacaddrArray => 1040,
            PgType::InetArray => 1041,
            PgType::Bpchar => 1042,
            PgType::Varchar => 1043,
            PgType::Date => 1082,
            PgType::Time => 1083,
            PgType::Timestamp => 1114,
            PgType::TimestampArray => 1115,
            PgType::DateArray => 1182,
            PgType::TimeArray => 1183,
            PgType::Timestamptz => 1184,
            PgType::TimestamptzArray => 1185,
            PgType::Interval => 1186,
            PgType::IntervalArray => 1187,
            PgType::NumericArray => 1231,
            PgType::Timetz => 1266,
            PgType::TimetzArray => 1270,
            PgType::Bit => 1560,
            PgType::BitArray => 1561,
            PgType::Varbit => 1562,
            PgType::VarbitArray => 1563,
            PgType::Numeric => 1700,
            PgType::Void => 2278,
            PgType::Record => 2249,
            PgType::RecordArray => 2287,
            PgType::Uuid => 2950,
            PgType::UuidArray => 2951,
            PgType::Jsonb => 3802,
            PgType::JsonbArray => 3807,
            PgType::Int4Range => 3904,
            PgType::Int4RangeArray => 3905,
            PgType::NumRange => 3906,
            PgType::NumRangeArray => 3907,
            PgType::TsRange => 3908,
            PgType::TsRangeArray => 3909,
            PgType::TstzRange => 3910,
            PgType::TstzRangeArray => 3911,
            PgType::DateRange => 3912,
            PgType::DateRangeArray => 3913,
            PgType::Int8Range => 3926,
            PgType::Int8RangeArray => 3927,
            PgType::Jsonpath => 4072,
            PgType::JsonpathArray => 4073,
            PgType::Custom(ty) => ty.oid,

            PgType::DeclareWithOid(oid) => *oid,
            PgType::DeclareWithName(_) => {
                return None;
            }
        })
    }

    pub(crate) fn display_name(&self) -> &str {
        match self {
            PgType::Bool => "BOOL",
            PgType::Bytea => "BYTEA",
            PgType::Char => "\"CHAR\"",
            PgType::Name => "NAME",
            PgType::Int8 => "INT8",
            PgType::Int2 => "INT2",
            PgType::Int4 => "INT4",
            PgType::Text => "TEXT",
            PgType::Oid => "OID",
            PgType::Json => "JSON",
            PgType::JsonArray => "JSON[]",
            PgType::Point => "POINT",
            PgType::Lseg => "LSEG",
            PgType::Path => "PATH",
            PgType::Box => "BOX",
            PgType::Polygon => "POLYGON",
            PgType::Line => "LINE",
            PgType::LineArray => "LINE[]",
            PgType::Cidr => "CIDR",
            PgType::CidrArray => "CIDR[]",
            PgType::Float4 => "FLOAT4",
            PgType::Float8 => "FLOAT8",
            PgType::Unknown => "UNKNOWN",
            PgType::Circle => "CIRCLE",
            PgType::CircleArray => "CIRCLE[]",
            PgType::Macaddr8 => "MACADDR8",
            PgType::Macaddr8Array => "MACADDR8[]",
            PgType::Macaddr => "MACADDR",
            PgType::Inet => "INET",
            PgType::BoolArray => "BOOL[]",
            PgType::ByteaArray => "BYTEA[]",
            PgType::CharArray => "\"CHAR\"[]",
            PgType::NameArray => "NAME[]",
            PgType::Int2Array => "INT2[]",
            PgType::Int4Array => "INT4[]",
            PgType::TextArray => "TEXT[]",
            PgType::BpcharArray => "CHAR[]",
            PgType::VarcharArray => "VARCHAR[]",
            PgType::Int8Array => "INT8[]",
            PgType::PointArray => "POINT[]",
            PgType::LsegArray => "LSEG[]",
            PgType::PathArray => "PATH[]",
            PgType::BoxArray => "BOX[]",
            PgType::Float4Array => "FLOAT4[]",
            PgType::Float8Array => "FLOAT8[]",
            PgType::PolygonArray => "POLYGON[]",
            PgType::OidArray => "OID[]",
            PgType::MacaddrArray => "MACADDR[]",
            PgType::InetArray => "INET[]",
            PgType::Bpchar => "CHAR",
            PgType::Varchar => "VARCHAR",
            PgType::Date => "DATE",
            PgType::Time => "TIME",
            PgType::Timestamp => "TIMESTAMP",
            PgType::TimestampArray => "TIMESTAMP[]",
            PgType::DateArray => "DATE[]",
            PgType::TimeArray => "TIME[]",
            PgType::Timestamptz => "TIMESTAMPTZ",
            PgType::TimestamptzArray => "TIMESTAMPTZ[]",
            PgType::Interval => "INTERVAL",
            PgType::IntervalArray => "INTERVAL[]",
            PgType::NumericArray => "NUMERIC[]",
            PgType::Timetz => "TIMETZ",
            PgType::TimetzArray => "TIMETZ[]",
            PgType::Bit => "BIT",
            PgType::BitArray => "BIT[]",
            PgType::Varbit => "VARBIT",
            PgType::VarbitArray => "VARBIT[]",
            PgType::Numeric => "NUMERIC",
            PgType::Record => "RECORD",
            PgType::RecordArray => "RECORD[]",
            PgType::Uuid => "UUID",
            PgType::UuidArray => "UUID[]",
            PgType::Jsonb => "JSONB",
            PgType::JsonbArray => "JSONB[]",
            PgType::Int4Range => "INT4RANGE",
            PgType::Int4RangeArray => "INT4RANGE[]",
            PgType::NumRange => "NUMRANGE",
            PgType::NumRangeArray => "NUMRANGE[]",
            PgType::TsRange => "TSRANGE",
            PgType::TsRangeArray => "TSRANGE[]",
            PgType::TstzRange => "TSTZRANGE",
            PgType::TstzRangeArray => "TSTZRANGE[]",
            PgType::DateRange => "DATERANGE",
            PgType::DateRangeArray => "DATERANGE[]",
            PgType::Int8Range => "INT8RANGE",
            PgType::Int8RangeArray => "INT8RANGE[]",
            PgType::Jsonpath => "JSONPATH",
            PgType::JsonpathArray => "JSONPATH[]",
            PgType::Money => "MONEY",
            PgType::MoneyArray => "MONEY[]",
            PgType::Void => "VOID",
            PgType::Custom(ty) => &*ty.name,
            PgType::DeclareWithOid(_) => "?",
            PgType::DeclareWithName(name) => name,
        }
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            PgType::Bool => "bool",
            PgType::Bytea => "bytea",
            PgType::Char => "char",
            PgType::Name => "name",
            PgType::Int8 => "int8",
            PgType::Int2 => "int2",
            PgType::Int4 => "int4",
            PgType::Text => "text",
            PgType::Oid => "oid",
            PgType::Json => "json",
            PgType::JsonArray => "_json",
            PgType::Point => "point",
            PgType::Lseg => "lseg",
            PgType::Path => "path",
            PgType::Box => "box",
            PgType::Polygon => "polygon",
            PgType::Line => "line",
            PgType::LineArray => "_line",
            PgType::Cidr => "cidr",
            PgType::CidrArray => "_cidr",
            PgType::Float4 => "float4",
            PgType::Float8 => "float8",
            PgType::Unknown => "unknown",
            PgType::Circle => "circle",
            PgType::CircleArray => "_circle",
            PgType::Macaddr8 => "macaddr8",
            PgType::Macaddr8Array => "_macaddr8",
            PgType::Macaddr => "macaddr",
            PgType::Inet => "inet",
            PgType::BoolArray => "_bool",
            PgType::ByteaArray => "_bytea",
            PgType::CharArray => "_char",
            PgType::NameArray => "_name",
            PgType::Int2Array => "_int2",
            PgType::Int4Array => "_int4",
            PgType::TextArray => "_text",
            PgType::BpcharArray => "_bpchar",
            PgType::VarcharArray => "_varchar",
            PgType::Int8Array => "_int8",
            PgType::PointArray => "_point",
            PgType::LsegArray => "_lseg",
            PgType::PathArray => "_path",
            PgType::BoxArray => "_box",
            PgType::Float4Array => "_float4",
            PgType::Float8Array => "_float8",
            PgType::PolygonArray => "_polygon",
            PgType::OidArray => "_oid",
            PgType::MacaddrArray => "_macaddr",
            PgType::InetArray => "_inet",
            PgType::Bpchar => "bpchar",
            PgType::Varchar => "varchar",
            PgType::Date => "date",
            PgType::Time => "time",
            PgType::Timestamp => "timestamp",
            PgType::TimestampArray => "_timestamp",
            PgType::DateArray => "_date",
            PgType::TimeArray => "_time",
            PgType::Timestamptz => "timestamptz",
            PgType::TimestamptzArray => "_timestamptz",
            PgType::Interval => "interval",
            PgType::IntervalArray => "_interval",
            PgType::NumericArray => "_numeric",
            PgType::Timetz => "timetz",
            PgType::TimetzArray => "_timetz",
            PgType::Bit => "bit",
            PgType::BitArray => "_bit",
            PgType::Varbit => "varbit",
            PgType::VarbitArray => "_varbit",
            PgType::Numeric => "numeric",
            PgType::Record => "record",
            PgType::RecordArray => "_record",
            PgType::Uuid => "uuid",
            PgType::UuidArray => "_uuid",
            PgType::Jsonb => "jsonb",
            PgType::JsonbArray => "_jsonb",
            PgType::Int4Range => "int4range",
            PgType::Int4RangeArray => "_int4range",
            PgType::NumRange => "numrange",
            PgType::NumRangeArray => "_numrange",
            PgType::TsRange => "tsrange",
            PgType::TsRangeArray => "_tsrange",
            PgType::TstzRange => "tstzrange",
            PgType::TstzRangeArray => "_tstzrange",
            PgType::DateRange => "daterange",
            PgType::DateRangeArray => "_daterange",
            PgType::Int8Range => "int8range",
            PgType::Int8RangeArray => "_int8range",
            PgType::Jsonpath => "jsonpath",
            PgType::JsonpathArray => "_jsonpath",
            PgType::Money => "money",
            PgType::MoneyArray => "_money",
            PgType::Void => "void",
            PgType::Custom(ty) => &*ty.name,
            PgType::DeclareWithOid(_) => "?",
            PgType::DeclareWithName(name) => name,
        }
    }

    pub(crate) fn kind(&self) -> &PgTypeKind {
        match self {
            PgType::Bool => &PgTypeKind::Simple,
            PgType::Bytea => &PgTypeKind::Simple,
            PgType::Char => &PgTypeKind::Simple,
            PgType::Name => &PgTypeKind::Simple,
            PgType::Int8 => &PgTypeKind::Simple,
            PgType::Int2 => &PgTypeKind::Simple,
            PgType::Int4 => &PgTypeKind::Simple,
            PgType::Text => &PgTypeKind::Simple,
            PgType::Oid => &PgTypeKind::Simple,
            PgType::Json => &PgTypeKind::Simple,
            PgType::JsonArray => &PgTypeKind::Array(PgTypeInfo(PgType::Json)),
            PgType::Point => &PgTypeKind::Simple,
            PgType::Lseg => &PgTypeKind::Simple,
            PgType::Path => &PgTypeKind::Simple,
            PgType::Box => &PgTypeKind::Simple,
            PgType::Polygon => &PgTypeKind::Simple,
            PgType::Line => &PgTypeKind::Simple,
            PgType::LineArray => &PgTypeKind::Array(PgTypeInfo(PgType::Line)),
            PgType::Cidr => &PgTypeKind::Simple,
            PgType::CidrArray => &PgTypeKind::Array(PgTypeInfo(PgType::Cidr)),
            PgType::Float4 => &PgTypeKind::Simple,
            PgType::Float8 => &PgTypeKind::Simple,
            PgType::Unknown => &PgTypeKind::Simple,
            PgType::Circle => &PgTypeKind::Simple,
            PgType::CircleArray => &PgTypeKind::Array(PgTypeInfo(PgType::Circle)),
            PgType::Macaddr8 => &PgTypeKind::Simple,
            PgType::Macaddr8Array => &PgTypeKind::Array(PgTypeInfo(PgType::Macaddr8)),
            PgType::Macaddr => &PgTypeKind::Simple,
            PgType::Inet => &PgTypeKind::Simple,
            PgType::BoolArray => &PgTypeKind::Array(PgTypeInfo(PgType::Bool)),
            PgType::ByteaArray => &PgTypeKind::Array(PgTypeInfo(PgType::Bytea)),
            PgType::CharArray => &PgTypeKind::Array(PgTypeInfo(PgType::Char)),
            PgType::NameArray => &PgTypeKind::Array(PgTypeInfo(PgType::Name)),
            PgType::Int2Array => &PgTypeKind::Array(PgTypeInfo(PgType::Int2)),
            PgType::Int4Array => &PgTypeKind::Array(PgTypeInfo(PgType::Int4)),
            PgType::TextArray => &PgTypeKind::Array(PgTypeInfo(PgType::Text)),
            PgType::BpcharArray => &PgTypeKind::Array(PgTypeInfo(PgType::Bpchar)),
            PgType::VarcharArray => &PgTypeKind::Array(PgTypeInfo(PgType::Varchar)),
            PgType::Int8Array => &PgTypeKind::Array(PgTypeInfo(PgType::Int8)),
            PgType::PointArray => &PgTypeKind::Array(PgTypeInfo(PgType::Point)),
            PgType::LsegArray => &PgTypeKind::Array(PgTypeInfo(PgType::Lseg)),
            PgType::PathArray => &PgTypeKind::Array(PgTypeInfo(PgType::Path)),
            PgType::BoxArray => &PgTypeKind::Array(PgTypeInfo(PgType::Box)),
            PgType::Float4Array => &PgTypeKind::Array(PgTypeInfo(PgType::Float4)),
            PgType::Float8Array => &PgTypeKind::Array(PgTypeInfo(PgType::Float8)),
            PgType::PolygonArray => &PgTypeKind::Array(PgTypeInfo(PgType::Polygon)),
            PgType::OidArray => &PgTypeKind::Array(PgTypeInfo(PgType::Oid)),
            PgType::MacaddrArray => &PgTypeKind::Array(PgTypeInfo(PgType::Macaddr)),
            PgType::InetArray => &PgTypeKind::Array(PgTypeInfo(PgType::Inet)),
            PgType::Bpchar => &PgTypeKind::Simple,
            PgType::Varchar => &PgTypeKind::Simple,
            PgType::Date => &PgTypeKind::Simple,
            PgType::Time => &PgTypeKind::Simple,
            PgType::Timestamp => &PgTypeKind::Simple,
            PgType::TimestampArray => &PgTypeKind::Array(PgTypeInfo(PgType::Timestamp)),
            PgType::DateArray => &PgTypeKind::Array(PgTypeInfo(PgType::Date)),
            PgType::TimeArray => &PgTypeKind::Array(PgTypeInfo(PgType::Time)),
            PgType::Timestamptz => &PgTypeKind::Simple,
            PgType::TimestamptzArray => &PgTypeKind::Array(PgTypeInfo(PgType::Timestamptz)),
            PgType::Interval => &PgTypeKind::Simple,
            PgType::IntervalArray => &PgTypeKind::Array(PgTypeInfo(PgType::Interval)),
            PgType::NumericArray => &PgTypeKind::Array(PgTypeInfo(PgType::Numeric)),
            PgType::Timetz => &PgTypeKind::Simple,
            PgType::TimetzArray => &PgTypeKind::Array(PgTypeInfo(PgType::Timetz)),
            PgType::Bit => &PgTypeKind::Simple,
            PgType::BitArray => &PgTypeKind::Array(PgTypeInfo(PgType::Bit)),
            PgType::Varbit => &PgTypeKind::Simple,
            PgType::VarbitArray => &PgTypeKind::Array(PgTypeInfo(PgType::Varbit)),
            PgType::Numeric => &PgTypeKind::Simple,
            PgType::Record => &PgTypeKind::Simple,
            PgType::RecordArray => &PgTypeKind::Array(PgTypeInfo(PgType::Record)),
            PgType::Uuid => &PgTypeKind::Simple,
            PgType::UuidArray => &PgTypeKind::Array(PgTypeInfo(PgType::Uuid)),
            PgType::Jsonb => &PgTypeKind::Simple,
            PgType::JsonbArray => &PgTypeKind::Array(PgTypeInfo(PgType::Jsonb)),
            PgType::Int4Range => &PgTypeKind::Range(PgTypeInfo::INT4),
            PgType::Int4RangeArray => &PgTypeKind::Array(PgTypeInfo(PgType::Int4Range)),
            PgType::NumRange => &PgTypeKind::Range(PgTypeInfo::NUMERIC),
            PgType::NumRangeArray => &PgTypeKind::Array(PgTypeInfo(PgType::NumRange)),
            PgType::TsRange => &PgTypeKind::Range(PgTypeInfo::TIMESTAMP),
            PgType::TsRangeArray => &PgTypeKind::Array(PgTypeInfo(PgType::TsRange)),
            PgType::TstzRange => &PgTypeKind::Range(PgTypeInfo::TIMESTAMPTZ),
            PgType::TstzRangeArray => &PgTypeKind::Array(PgTypeInfo(PgType::TstzRange)),
            PgType::DateRange => &PgTypeKind::Range(PgTypeInfo::DATE),
            PgType::DateRangeArray => &PgTypeKind::Array(PgTypeInfo(PgType::DateRange)),
            PgType::Int8Range => &PgTypeKind::Range(PgTypeInfo::INT8),
            PgType::Int8RangeArray => &PgTypeKind::Array(PgTypeInfo(PgType::Int8Range)),
            PgType::Jsonpath => &PgTypeKind::Simple,
            PgType::JsonpathArray => &PgTypeKind::Array(PgTypeInfo(PgType::Jsonpath)),
            PgType::Money => &PgTypeKind::Simple,
            PgType::MoneyArray => &PgTypeKind::Array(PgTypeInfo(PgType::Money)),

            PgType::Void => &PgTypeKind::Pseudo,

            PgType::Custom(ty) => &ty.kind,

            PgType::DeclareWithOid(_) | PgType::DeclareWithName(_) => {
                unreachable!("(bug) use of unresolved type declaration [kind]")
            }
        }
    }
}

impl TypeInfo for PgTypeInfo {
    fn name(&self) -> &str {
        self.0.display_name()
    }

    fn is_null(&self) -> bool {
        false
    }

    fn is_void(&self) -> bool {
        matches!(self.0, PgType::Void)
    }
}

impl PartialEq<PgCustomType> for PgCustomType {
    fn eq(&self, other: &PgCustomType) -> bool {
        other.oid == self.oid
    }
}

impl PgTypeInfo {
    // boolean, state of true or false
    pub(crate) const BOOL: Self = Self(PgType::Bool);
    pub(crate) const BOOL_ARRAY: Self = Self(PgType::BoolArray);

    // binary data types, variable-length binary string
    pub(crate) const BYTEA: Self = Self(PgType::Bytea);
    pub(crate) const BYTEA_ARRAY: Self = Self(PgType::ByteaArray);

    // uuid
    pub(crate) const UUID: Self = Self(PgType::Uuid);
    pub(crate) const UUID_ARRAY: Self = Self(PgType::UuidArray);

    // record
    pub(crate) const RECORD: Self = Self(PgType::Record);
    pub(crate) const RECORD_ARRAY: Self = Self(PgType::RecordArray);

    //
    // JSON types
    // https://www.postgresql.org/docs/current/datatype-json.html
    //

    pub(crate) const JSON: Self = Self(PgType::Json);
    pub(crate) const JSON_ARRAY: Self = Self(PgType::JsonArray);

    pub(crate) const JSONB: Self = Self(PgType::Jsonb);
    pub(crate) const JSONB_ARRAY: Self = Self(PgType::JsonbArray);

    pub(crate) const JSONPATH: Self = Self(PgType::Jsonpath);
    pub(crate) const JSONPATH_ARRAY: Self = Self(PgType::JsonpathArray);

    //
    // network address types
    // https://www.postgresql.org/docs/current/datatype-net-types.html
    //

    pub(crate) const CIDR: Self = Self(PgType::Cidr);
    pub(crate) const CIDR_ARRAY: Self = Self(PgType::CidrArray);

    pub(crate) const INET: Self = Self(PgType::Inet);
    pub(crate) const INET_ARRAY: Self = Self(PgType::InetArray);

    pub(crate) const MACADDR: Self = Self(PgType::Macaddr);
    pub(crate) const MACADDR_ARRAY: Self = Self(PgType::MacaddrArray);

    pub(crate) const MACADDR8: Self = Self(PgType::Macaddr8);
    pub(crate) const MACADDR8_ARRAY: Self = Self(PgType::Macaddr8Array);

    //
    // character types
    // https://www.postgresql.org/docs/current/datatype-character.html
    //

    // internal type for object names
    pub(crate) const NAME: Self = Self(PgType::Name);
    pub(crate) const NAME_ARRAY: Self = Self(PgType::NameArray);

    // character type, fixed-length, blank-padded
    pub(crate) const BPCHAR: Self = Self(PgType::Bpchar);
    pub(crate) const BPCHAR_ARRAY: Self = Self(PgType::BpcharArray);

    // character type, variable-length with limit
    pub(crate) const VARCHAR: Self = Self(PgType::Varchar);
    pub(crate) const VARCHAR_ARRAY: Self = Self(PgType::VarcharArray);

    // character type, variable-length
    pub(crate) const TEXT: Self = Self(PgType::Text);
    pub(crate) const TEXT_ARRAY: Self = Self(PgType::TextArray);

    // unknown type, transmitted as text
    pub(crate) const UNKNOWN: Self = Self(PgType::Unknown);

    //
    // numeric types
    // https://www.postgresql.org/docs/current/datatype-numeric.html
    //

    // single-byte internal type
    pub(crate) const CHAR: Self = Self(PgType::Char);
    pub(crate) const CHAR_ARRAY: Self = Self(PgType::CharArray);

    // internal type for type ids
    pub(crate) const OID: Self = Self(PgType::Oid);
    pub(crate) const OID_ARRAY: Self = Self(PgType::OidArray);

    // small-range integer; -32768 to +32767
    pub(crate) const INT2: Self = Self(PgType::Int2);
    pub(crate) const INT2_ARRAY: Self = Self(PgType::Int2Array);

    // typical choice for integer; -2147483648 to +2147483647
    pub(crate) const INT4: Self = Self(PgType::Int4);
    pub(crate) const INT4_ARRAY: Self = Self(PgType::Int4Array);

    // large-range integer; -9223372036854775808 to +9223372036854775807
    pub(crate) const INT8: Self = Self(PgType::Int8);
    pub(crate) const INT8_ARRAY: Self = Self(PgType::Int8Array);

    // variable-precision, inexact, 6 decimal digits precision
    pub(crate) const FLOAT4: Self = Self(PgType::Float4);
    pub(crate) const FLOAT4_ARRAY: Self = Self(PgType::Float4Array);

    // variable-precision, inexact, 15 decimal digits precision
    pub(crate) const FLOAT8: Self = Self(PgType::Float8);
    pub(crate) const FLOAT8_ARRAY: Self = Self(PgType::Float8Array);

    // user-specified precision, exact
    pub(crate) const NUMERIC: Self = Self(PgType::Numeric);
    pub(crate) const NUMERIC_ARRAY: Self = Self(PgType::NumericArray);

    // user-specified precision, exact
    pub(crate) const MONEY: Self = Self(PgType::Money);
    pub(crate) const MONEY_ARRAY: Self = Self(PgType::MoneyArray);

    //
    // date/time types
    // https://www.postgresql.org/docs/current/datatype-datetime.html
    //

    // both date and time (no time zone)
    pub(crate) const TIMESTAMP: Self = Self(PgType::Timestamp);
    pub(crate) const TIMESTAMP_ARRAY: Self = Self(PgType::TimestampArray);

    // both date and time (with time zone)
    pub(crate) const TIMESTAMPTZ: Self = Self(PgType::Timestamptz);
    pub(crate) const TIMESTAMPTZ_ARRAY: Self = Self(PgType::TimestamptzArray);

    // date (no time of day)
    pub(crate) const DATE: Self = Self(PgType::Date);
    pub(crate) const DATE_ARRAY: Self = Self(PgType::DateArray);

    // time of day (no date)
    pub(crate) const TIME: Self = Self(PgType::Time);
    pub(crate) const TIME_ARRAY: Self = Self(PgType::TimeArray);

    // time of day (no date), with time zone
    pub(crate) const TIMETZ: Self = Self(PgType::Timetz);
    pub(crate) const TIMETZ_ARRAY: Self = Self(PgType::TimetzArray);

    // time interval
    pub(crate) const INTERVAL: Self = Self(PgType::Interval);
    pub(crate) const INTERVAL_ARRAY: Self = Self(PgType::IntervalArray);

    //
    // geometric types
    // https://www.postgresql.org/docs/current/datatype-geometric.html
    //

    // point on a plane
    pub(crate) const POINT: Self = Self(PgType::Point);
    pub(crate) const POINT_ARRAY: Self = Self(PgType::PointArray);

    // infinite line
    pub(crate) const LINE: Self = Self(PgType::Line);
    pub(crate) const LINE_ARRAY: Self = Self(PgType::LineArray);

    // finite line segment
    pub(crate) const LSEG: Self = Self(PgType::Lseg);
    pub(crate) const LSEG_ARRAY: Self = Self(PgType::LsegArray);

    // rectangular box
    pub(crate) const BOX: Self = Self(PgType::Box);
    pub(crate) const BOX_ARRAY: Self = Self(PgType::BoxArray);

    // open or closed path
    pub(crate) const PATH: Self = Self(PgType::Path);
    pub(crate) const PATH_ARRAY: Self = Self(PgType::PathArray);

    // polygon
    pub(crate) const POLYGON: Self = Self(PgType::Polygon);
    pub(crate) const POLYGON_ARRAY: Self = Self(PgType::PolygonArray);

    // circle
    pub(crate) const CIRCLE: Self = Self(PgType::Circle);
    pub(crate) const CIRCLE_ARRAY: Self = Self(PgType::CircleArray);

    //
    // bit string types
    // https://www.postgresql.org/docs/current/datatype-bit.html
    //

    pub(crate) const BIT: Self = Self(PgType::Bit);
    pub(crate) const BIT_ARRAY: Self = Self(PgType::BitArray);

    pub(crate) const VARBIT: Self = Self(PgType::Varbit);
    pub(crate) const VARBIT_ARRAY: Self = Self(PgType::VarbitArray);

    //
    // range types
    // https://www.postgresql.org/docs/current/rangetypes.html
    //

    pub(crate) const INT4_RANGE: Self = Self(PgType::Int4Range);
    pub(crate) const INT4_RANGE_ARRAY: Self = Self(PgType::Int4RangeArray);

    pub(crate) const NUM_RANGE: Self = Self(PgType::NumRange);
    pub(crate) const NUM_RANGE_ARRAY: Self = Self(PgType::NumRangeArray);

    pub(crate) const TS_RANGE: Self = Self(PgType::TsRange);
    pub(crate) const TS_RANGE_ARRAY: Self = Self(PgType::TsRangeArray);

    pub(crate) const TSTZ_RANGE: Self = Self(PgType::TstzRange);
    pub(crate) const TSTZ_RANGE_ARRAY: Self = Self(PgType::TstzRangeArray);

    pub(crate) const DATE_RANGE: Self = Self(PgType::DateRange);
    pub(crate) const DATE_RANGE_ARRAY: Self = Self(PgType::DateRangeArray);

    pub(crate) const INT8_RANGE: Self = Self(PgType::Int8Range);
    pub(crate) const INT8_RANGE_ARRAY: Self = Self(PgType::Int8RangeArray);

    //
    // pseudo types
    // https://www.postgresql.org/docs/9.3/datatype-pseudo.html
    //

    pub(crate) const VOID: Self = Self(PgType::Void);
}

impl Display for PgTypeInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(self.name())
    }
}

impl PartialEq<PgType> for PgType {
    fn eq(&self, other: &PgType) -> bool {
        if let (Some(a), Some(b)) = (self.try_oid(), other.try_oid()) {
            // If there are OIDs available, use OIDs to perform a direct match
            a == b
        } else if matches!(
            (self, other),
            (PgType::DeclareWithName(_), PgType::DeclareWithOid(_))
                | (PgType::DeclareWithOid(_), PgType::DeclareWithName(_))
        ) {
            // One is a declare-with-name and the other is a declare-with-id
            // This only occurs in the TEXT protocol with custom types
            // Just opt-out of type checking here
            true
        } else {
            // Otherwise, perform a match on the name
            self.name().eq_ignore_ascii_case(other.name())
        }
    }
}

#[cfg(feature = "any")]
impl From<PgTypeInfo> for crate::any::AnyTypeInfo {
    #[inline]
    fn from(ty: PgTypeInfo) -> Self {
        crate::any::AnyTypeInfo(crate::any::type_info::AnyTypeInfoKind::Postgres(ty))
    }
}
