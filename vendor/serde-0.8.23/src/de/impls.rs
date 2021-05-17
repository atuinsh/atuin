//! This module contains `Deserialize` and `Visitor` implementations.

#[cfg(feature = "std")]
use std::borrow::Cow;
#[cfg(all(feature = "unstable", feature = "collections", not(feature = "std")))]
use collections::borrow::Cow;

#[cfg(all(feature = "collections", not(feature = "std")))]
use collections::{
    BinaryHeap,
    BTreeMap,
    BTreeSet,
    LinkedList,
    VecDeque,
    Vec,
    String,
};

#[cfg(feature = "std")]
use std::collections::{
    HashMap,
    HashSet,
    BinaryHeap,
    BTreeMap,
    BTreeSet,
    LinkedList,
    VecDeque,
};

#[cfg(all(feature = "unstable", feature = "collections"))]
use collections::enum_set::{CLike, EnumSet};
#[cfg(all(feature = "unstable", feature = "collections"))]
use collections::borrow::ToOwned;

use core::hash::{Hash, BuildHasher};
use core::marker::PhantomData;
#[cfg(feature = "std")]
use std::net;
#[cfg(feature = "std")]
use std::path;
use core::str;

#[cfg(feature = "std")]
use std::rc::Rc;
#[cfg(all(feature = "unstable", feature = "alloc", not(feature = "std")))]
use alloc::rc::Rc;

#[cfg(feature = "std")]
use std::sync::Arc;
#[cfg(all(feature = "unstable", feature = "alloc", not(feature = "std")))]
use alloc::arc::Arc;

#[cfg(all(feature = "unstable", feature = "alloc", not(feature = "std")))]
use alloc::boxed::Box;

#[cfg(feature = "std")]
use std::time::Duration;

#[cfg(feature = "unstable")]
use core::nonzero::{NonZero, Zeroable};

#[cfg(feature = "unstable")]
use core::num::Zero;

use de::{
    Deserialize,
    Deserializer,
    EnumVisitor,
    Error,
    MapVisitor,
    SeqVisitor,
    Type,
    VariantVisitor,
    Visitor,
};
use de::from_primitive::FromPrimitive;

///////////////////////////////////////////////////////////////////////////////

/// A visitor that produces a `()`.
pub struct UnitVisitor;

impl Visitor for UnitVisitor {
    type Value = ();

    fn visit_unit<E>(&mut self) -> Result<(), E>
        where E: Error,
    {
        Ok(())
    }

    fn visit_seq<V>(&mut self, mut visitor: V) -> Result<(), V::Error>
        where V: SeqVisitor,
    {
        visitor.end()
    }
}

impl Deserialize for () {
    fn deserialize<D>(deserializer: &mut D) -> Result<(), D::Error>
        where D: Deserializer,
    {
        deserializer.deserialize_unit(UnitVisitor)
    }
}

///////////////////////////////////////////////////////////////////////////////

/// A visitor that produces a `bool`.
pub struct BoolVisitor;

impl Visitor for BoolVisitor {
    type Value = bool;

    fn visit_bool<E>(&mut self, v: bool) -> Result<bool, E>
        where E: Error,
    {
        Ok(v)
    }

    fn visit_str<E>(&mut self, s: &str) -> Result<bool, E>
        where E: Error,
    {
        match s.trim_matches(::utils::Pattern_White_Space) {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(Error::invalid_type(Type::Bool)),
        }
    }
}

impl Deserialize for bool {
    fn deserialize<D>(deserializer: &mut D) -> Result<bool, D::Error>
        where D: Deserializer,
    {
        deserializer.deserialize_bool(BoolVisitor)
    }
}

///////////////////////////////////////////////////////////////////////////////

macro_rules! impl_deserialize_num_method {
    ($src_ty:ty, $method:ident, $from_method:ident, $ty:expr) => {
        #[inline]
        fn $method<E>(&mut self, v: $src_ty) -> Result<T, E>
            where E: Error,
        {
            match FromPrimitive::$from_method(v) {
                Some(v) => Ok(v),
                None => Err(Error::invalid_type($ty)),
            }
        }
    }
}

/// A visitor that produces a primitive type.
struct PrimitiveVisitor<T> {
    marker: PhantomData<T>,
}

impl<T> PrimitiveVisitor<T> {
    /// Construct a new `PrimitiveVisitor`.
    #[inline]
    fn new() -> Self {
        PrimitiveVisitor {
            marker: PhantomData,
        }
    }
}

impl<T> Visitor for PrimitiveVisitor<T>
    where T: Deserialize + FromPrimitive + str::FromStr
{
    type Value = T;

    impl_deserialize_num_method!(isize, visit_isize, from_isize, Type::Isize);
    impl_deserialize_num_method!(i8, visit_i8, from_i8, Type::I8);
    impl_deserialize_num_method!(i16, visit_i16, from_i16, Type::I16);
    impl_deserialize_num_method!(i32, visit_i32, from_i32, Type::I32);
    impl_deserialize_num_method!(i64, visit_i64, from_i64, Type::I64);
    impl_deserialize_num_method!(usize, visit_usize, from_usize, Type::Usize);
    impl_deserialize_num_method!(u8, visit_u8, from_u8, Type::U8);
    impl_deserialize_num_method!(u16, visit_u16, from_u16, Type::U16);
    impl_deserialize_num_method!(u32, visit_u32, from_u32, Type::U32);
    impl_deserialize_num_method!(u64, visit_u64, from_u64, Type::U64);
    impl_deserialize_num_method!(f32, visit_f32, from_f32, Type::F32);
    impl_deserialize_num_method!(f64, visit_f64, from_f64, Type::F64);

    #[inline]
    fn visit_str<E>(&mut self, s: &str) -> Result<T, E>
        where E: Error,
    {
        str::FromStr::from_str(s.trim_matches(::utils::Pattern_White_Space)).or_else(|_| {
            Err(Error::invalid_type(Type::Str))
        })
    }
}

macro_rules! impl_deserialize_num {
    ($ty:ty, $method:ident) => {
        impl Deserialize for $ty {
            #[inline]
            fn deserialize<D>(deserializer: &mut D) -> Result<$ty, D::Error>
                where D: Deserializer,
            {
                deserializer.$method(PrimitiveVisitor::new())
            }
        }
    }
}

impl_deserialize_num!(isize, deserialize_isize);
impl_deserialize_num!(i8, deserialize_i8);
impl_deserialize_num!(i16, deserialize_i16);
impl_deserialize_num!(i32, deserialize_i32);
impl_deserialize_num!(i64, deserialize_i64);
impl_deserialize_num!(usize, deserialize_usize);
impl_deserialize_num!(u8, deserialize_u8);
impl_deserialize_num!(u16, deserialize_u16);
impl_deserialize_num!(u32, deserialize_u32);
impl_deserialize_num!(u64, deserialize_u64);
impl_deserialize_num!(f32, deserialize_f32);
impl_deserialize_num!(f64, deserialize_f64);

///////////////////////////////////////////////////////////////////////////////

struct CharVisitor;

impl Visitor for CharVisitor {
    type Value = char;

    #[inline]
    fn visit_char<E>(&mut self, v: char) -> Result<char, E>
        where E: Error,
    {
        Ok(v)
    }

    #[inline]
    fn visit_str<E>(&mut self, v: &str) -> Result<char, E>
        where E: Error,
    {
        let mut iter = v.chars();
        if let Some(v) = iter.next() {
            if iter.next().is_some() {
                Err(Error::invalid_type(Type::Char))
            } else {
                Ok(v)
            }
        } else {
            Err(Error::end_of_stream())
        }
    }
}

impl Deserialize for char {
    #[inline]
    fn deserialize<D>(deserializer: &mut D) -> Result<char, D::Error>
        where D: Deserializer,
    {
        deserializer.deserialize_char(CharVisitor)
    }
}

///////////////////////////////////////////////////////////////////////////////

#[cfg(any(feature = "std", feature = "collections"))]
struct StringVisitor;

#[cfg(any(feature = "std", feature = "collections"))]
impl Visitor for StringVisitor {
    type Value = String;

    fn visit_str<E>(&mut self, v: &str) -> Result<String, E>
        where E: Error,
    {
        Ok(v.to_owned())
    }

    fn visit_string<E>(&mut self, v: String) -> Result<String, E>
        where E: Error,
    {
        Ok(v)
    }

    fn visit_unit<E>(&mut self) -> Result<String, E>
        where E: Error,
    {
        Ok(String::new())
    }

    fn visit_bytes<E>(&mut self, v: &[u8]) -> Result<String, E>
        where E: Error,
    {
        match str::from_utf8(v) {
            Ok(s) => Ok(s.to_owned()),
            Err(_) => Err(Error::invalid_type(Type::String)),
        }
    }

    fn visit_byte_buf<E>(&mut self, v: Vec<u8>) -> Result<String, E>
        where E: Error,
    {
        match String::from_utf8(v) {
            Ok(s) => Ok(s),
            Err(_) => Err(Error::invalid_type(Type::String)),
        }
    }
}

#[cfg(any(feature = "std", feature = "collections"))]
impl Deserialize for String {
    fn deserialize<D>(deserializer: &mut D) -> Result<String, D::Error>
        where D: Deserializer,
    {
        deserializer.deserialize_string(StringVisitor)
    }
}

///////////////////////////////////////////////////////////////////////////////

struct OptionVisitor<T> {
    marker: PhantomData<T>,
}

impl<
    T: Deserialize,
> Visitor for OptionVisitor<T> {
    type Value = Option<T>;

    #[inline]
    fn visit_unit<E>(&mut self) -> Result<Option<T>, E>
        where E: Error,
    {
        Ok(None)
    }

    #[inline]
    fn visit_none<E>(&mut self) -> Result<Option<T>, E>
        where E: Error,
    {
        Ok(None)
    }

    #[inline]
    fn visit_some<D>(&mut self, deserializer: &mut D) -> Result<Option<T>, D::Error>
        where D: Deserializer,
    {
        Ok(Some(try!(Deserialize::deserialize(deserializer))))
    }
}

impl<T> Deserialize for Option<T> where T: Deserialize {
    fn deserialize<D>(deserializer: &mut D) -> Result<Option<T>, D::Error>
        where D: Deserializer,
    {
        deserializer.deserialize_option(OptionVisitor { marker: PhantomData })
    }
}

///////////////////////////////////////////////////////////////////////////////

/// A visitor that produces a `PhantomData`.
pub struct PhantomDataVisitor<T> {
    marker: PhantomData<T>,
}

impl<T> Visitor for PhantomDataVisitor<T> {
    type Value = PhantomData<T>;

    #[inline]
    fn visit_unit<E>(&mut self) -> Result<PhantomData<T>, E>
        where E: Error,
    {
        Ok(PhantomData)
    }
}

impl<T> Deserialize for PhantomData<T> {
    fn deserialize<D>(deserializer: &mut D) -> Result<PhantomData<T>, D::Error>
        where D: Deserializer,
    {
        let visitor = PhantomDataVisitor { marker: PhantomData };
        deserializer.deserialize_unit_struct("PhantomData", visitor)
    }
}

///////////////////////////////////////////////////////////////////////////////

macro_rules! seq_impl {
    (
        $ty:ty,
        $visitor_ty:ident < $($typaram:ident : $bound1:ident $(+ $bound2:ident)*),* >,
        $visitor:ident,
        $ctor:expr,
        $with_capacity:expr,
        $insert:expr
    ) => {
        /// A visitor that produces a sequence.
        pub struct $visitor_ty<$($typaram),*> {
            marker: PhantomData<$ty>,
        }

        impl<$($typaram),*> $visitor_ty<$($typaram),*>
            where $($typaram: $bound1 $(+ $bound2)*),*
        {
            /// Construct a new sequence visitor.
            pub fn new() -> Self {
                $visitor_ty {
                    marker: PhantomData,
                }
            }
        }

        impl<$($typaram),*> Visitor for $visitor_ty<$($typaram),*>
            where $($typaram: $bound1 $(+ $bound2)*),*
        {
            type Value = $ty;

            #[inline]
            fn visit_unit<E>(&mut self) -> Result<$ty, E>
                where E: Error,
            {
                Ok($ctor)
            }

            #[inline]
            fn visit_seq<V>(&mut self, mut $visitor: V) -> Result<$ty, V::Error>
                where V: SeqVisitor,
            {
                let mut values = $with_capacity;

                while let Some(value) = try!($visitor.visit()) {
                    $insert(&mut values, value);
                }

                try!($visitor.end());

                Ok(values)
            }
        }

        impl<$($typaram),*> Deserialize for $ty
            where $($typaram: $bound1 $(+ $bound2)*),*
        {
            fn deserialize<D>(deserializer: &mut D) -> Result<$ty, D::Error>
                where D: Deserializer,
            {
                deserializer.deserialize_seq($visitor_ty::new())
            }
        }
    }
}

#[cfg(any(feature = "std", feature = "collections"))]
seq_impl!(
    BinaryHeap<T>,
    BinaryHeapVisitor<T: Deserialize + Ord>,
    visitor,
    BinaryHeap::new(),
    BinaryHeap::with_capacity(visitor.size_hint().0),
    BinaryHeap::push);

#[cfg(any(feature = "std", feature = "collections"))]
seq_impl!(
    BTreeSet<T>,
    BTreeSetVisitor<T: Deserialize + Eq + Ord>,
    visitor,
    BTreeSet::new(),
    BTreeSet::new(),
    BTreeSet::insert);

#[cfg(all(feature = "unstable", feature = "collections"))]
seq_impl!(
    EnumSet<T>,
    EnumSetVisitor<T: Deserialize + CLike>,
    visitor,
    EnumSet::new(),
    EnumSet::new(),
    EnumSet::insert);

#[cfg(any(feature = "std", feature = "collections"))]
seq_impl!(
    LinkedList<T>,
    LinkedListVisitor<T: Deserialize>,
    visitor,
    LinkedList::new(),
    LinkedList::new(),
    LinkedList::push_back);

#[cfg(feature = "std")]
seq_impl!(
    HashSet<T, S>,
    HashSetVisitor<T: Deserialize + Eq + Hash,
                   S: BuildHasher + Default>,
    visitor,
    HashSet::with_hasher(S::default()),
    HashSet::with_capacity_and_hasher(visitor.size_hint().0, S::default()),
    HashSet::insert);

#[cfg(any(feature = "std", feature = "collections"))]
seq_impl!(
    Vec<T>,
    VecVisitor<T: Deserialize>,
    visitor,
    Vec::new(),
    Vec::with_capacity(visitor.size_hint().0),
    Vec::push);

#[cfg(any(feature = "std", feature = "collections"))]
seq_impl!(
    VecDeque<T>,
    VecDequeVisitor<T: Deserialize>,
    visitor,
    VecDeque::new(),
    VecDeque::with_capacity(visitor.size_hint().0),
    VecDeque::push_back);

///////////////////////////////////////////////////////////////////////////////

struct ArrayVisitor<A> {
    marker: PhantomData<A>,
}

impl<A> ArrayVisitor<A> {
    pub fn new() -> Self {
        ArrayVisitor {
            marker: PhantomData,
        }
    }
}

impl<T> Visitor for ArrayVisitor<[T; 0]> where T: Deserialize {
    type Value = [T; 0];

    #[inline]
    fn visit_unit<E>(&mut self) -> Result<[T; 0], E>
        where E: Error,
    {
        Ok([])
    }

    #[inline]
    fn visit_seq<V>(&mut self, mut visitor: V) -> Result<[T; 0], V::Error>
        where V: SeqVisitor,
    {
        try!(visitor.end());
        Ok([])
    }
}

impl<T> Deserialize for [T; 0]
    where T: Deserialize
{
    fn deserialize<D>(deserializer: &mut D) -> Result<[T; 0], D::Error>
        where D: Deserializer,
    {
        deserializer.deserialize_seq_fixed_size(0, ArrayVisitor::<[T; 0]>::new())
    }
}

macro_rules! array_impls {
    ($($len:expr => ($($name:ident)+))+) => {
        $(
            impl<T> Visitor for ArrayVisitor<[T; $len]> where T: Deserialize {
                type Value = [T; $len];

                #[inline]
                fn visit_seq<V>(&mut self, mut visitor: V) -> Result<[T; $len], V::Error>
                    where V: SeqVisitor,
                {
                    $(
                        let $name = match try!(visitor.visit()) {
                            Some(val) => val,
                            None => return Err(Error::end_of_stream()),
                        };
                    )+

                    try!(visitor.end());

                    Ok([$($name),+])
                }
            }

            impl<T> Deserialize for [T; $len]
                where T: Deserialize,
            {
                fn deserialize<D>(deserializer: &mut D) -> Result<[T; $len], D::Error>
                    where D: Deserializer,
                {
                    deserializer.deserialize_seq_fixed_size($len, ArrayVisitor::<[T; $len]>::new())
                }
            }
        )+
    }
}

array_impls! {
    1 => (a)
    2 => (a b)
    3 => (a b c)
    4 => (a b c d)
    5 => (a b c d e)
    6 => (a b c d e f)
    7 => (a b c d e f g)
    8 => (a b c d e f g h)
    9 => (a b c d e f g h i)
    10 => (a b c d e f g h i j)
    11 => (a b c d e f g h i j k)
    12 => (a b c d e f g h i j k l)
    13 => (a b c d e f g h i j k l m)
    14 => (a b c d e f g h i j k l m n)
    15 => (a b c d e f g h i j k l m n o)
    16 => (a b c d e f g h i j k l m n o p)
    17 => (a b c d e f g h i j k l m n o p q)
    18 => (a b c d e f g h i j k l m n o p q r)
    19 => (a b c d e f g h i j k l m n o p q r s)
    20 => (a b c d e f g h i j k l m n o p q r s t)
    21 => (a b c d e f g h i j k l m n o p q r s t u)
    22 => (a b c d e f g h i j k l m n o p q r s t u v)
    23 => (a b c d e f g h i j k l m n o p q r s t u v w)
    24 => (a b c d e f g h i j k l m n o p q r s t u v w x)
    25 => (a b c d e f g h i j k l m n o p q r s t u v w x y)
    26 => (a b c d e f g h i j k l m n o p q r s t u v w x y z)
    27 => (a b c d e f g h i j k l m n o p q r s t u v w x y z aa)
    28 => (a b c d e f g h i j k l m n o p q r s t u v w x y z aa ab)
    29 => (a b c d e f g h i j k l m n o p q r s t u v w x y z aa ab ac)
    30 => (a b c d e f g h i j k l m n o p q r s t u v w x y z aa ab ac ad)
    31 => (a b c d e f g h i j k l m n o p q r s t u v w x y z aa ab ac ad ae)
    32 => (a b c d e f g h i j k l m n o p q r s t u v w x y z aa ab ac ad ae af)
}

///////////////////////////////////////////////////////////////////////////////

macro_rules! tuple_impls {
    ($($len:expr => $visitor:ident => ($($name:ident)+))+) => {
        $(
            /// Construct a tuple visitor.
            pub struct $visitor<$($name,)+> {
                marker: PhantomData<($($name,)+)>,
            }

            impl<$($name: Deserialize,)+> $visitor<$($name,)+> {
                /// Construct a `TupleVisitor*<T>`.
                pub fn new() -> Self {
                    $visitor { marker: PhantomData }
                }
            }

            impl<$($name: Deserialize),+> Visitor for $visitor<$($name,)+> {
                type Value = ($($name,)+);

                #[inline]
                #[allow(non_snake_case)]
                fn visit_seq<V>(&mut self, mut visitor: V) -> Result<($($name,)+), V::Error>
                    where V: SeqVisitor,
                {
                    $(
                        let $name = match try!(visitor.visit()) {
                            Some(value) => value,
                            None => return Err(Error::end_of_stream()),
                        };
                    )+

                    try!(visitor.end());

                    Ok(($($name,)+))
                }
            }

            impl<$($name: Deserialize),+> Deserialize for ($($name,)+) {
                #[inline]
                fn deserialize<D>(deserializer: &mut D) -> Result<($($name,)+), D::Error>
                    where D: Deserializer,
                {
                    deserializer.deserialize_tuple($len, $visitor::new())
                }
            }
        )+
    }
}

tuple_impls! {
    1 => TupleVisitor1 => (T0)
    2 => TupleVisitor2 => (T0 T1)
    3 => TupleVisitor3 => (T0 T1 T2)
    4 => TupleVisitor4 => (T0 T1 T2 T3)
    5 => TupleVisitor5 => (T0 T1 T2 T3 T4)
    6 => TupleVisitor6 => (T0 T1 T2 T3 T4 T5)
    7 => TupleVisitor7 => (T0 T1 T2 T3 T4 T5 T6)
    8 => TupleVisitor8 => (T0 T1 T2 T3 T4 T5 T6 T7)
    9 => TupleVisitor9 => (T0 T1 T2 T3 T4 T5 T6 T7 T8)
    10 => TupleVisitor10 => (T0 T1 T2 T3 T4 T5 T6 T7 T8 T9)
    11 => TupleVisitor11 => (T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10)
    12 => TupleVisitor12 => (T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11)
    13 => TupleVisitor13 => (T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11 T12)
    14 => TupleVisitor14 => (T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11 T12 T13)
    15 => TupleVisitor15 => (T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11 T12 T13 T14)
    16 => TupleVisitor16 => (T0 T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11 T12 T13 T14 T15)
}

///////////////////////////////////////////////////////////////////////////////

macro_rules! map_impl {
    (
        $ty:ty,
        $visitor_ty:ident < $($typaram:ident : $bound1:ident $(+ $bound2:ident)*),* >,
        $visitor:ident,
        $ctor:expr,
        $with_capacity:expr
    ) => {
        /// A visitor that produces a map.
        pub struct $visitor_ty<$($typaram),*> {
            marker: PhantomData<$ty>,
        }

        impl<$($typaram),*> $visitor_ty<$($typaram),*>
            where $($typaram: $bound1 $(+ $bound2)*),*
        {
            /// Construct a `MapVisitor*<T>`.
            pub fn new() -> Self {
                $visitor_ty {
                    marker: PhantomData,
                }
            }
        }

        impl<$($typaram),*> Visitor for $visitor_ty<$($typaram),*>
            where $($typaram: $bound1 $(+ $bound2)*),*
        {
            type Value = $ty;

            #[inline]
            fn visit_unit<E>(&mut self) -> Result<$ty, E>
                where E: Error,
            {
                Ok($ctor)
            }

            #[inline]
            fn visit_map<Visitor>(&mut self, mut $visitor: Visitor) -> Result<$ty, Visitor::Error>
                where Visitor: MapVisitor,
            {
                let mut values = $with_capacity;

                while let Some((key, value)) = try!($visitor.visit()) {
                    values.insert(key, value);
                }

                try!($visitor.end());

                Ok(values)
            }
        }

        impl<$($typaram),*> Deserialize for $ty
            where $($typaram: $bound1 $(+ $bound2)*),*
        {
            fn deserialize<D>(deserializer: &mut D) -> Result<$ty, D::Error>
                where D: Deserializer,
            {
                deserializer.deserialize_map($visitor_ty::new())
            }
        }
    }
}

#[cfg(any(feature = "std", feature = "collections"))]
map_impl!(
    BTreeMap<K, V>,
    BTreeMapVisitor<K: Deserialize + Ord,
                    V: Deserialize>,
    visitor,
    BTreeMap::new(),
    BTreeMap::new());

#[cfg(feature = "std")]
map_impl!(
    HashMap<K, V, S>,
    HashMapVisitor<K: Deserialize + Eq + Hash,
                   V: Deserialize,
                   S: BuildHasher + Default>,
    visitor,
    HashMap::with_hasher(S::default()),
    HashMap::with_capacity_and_hasher(visitor.size_hint().0, S::default()));

///////////////////////////////////////////////////////////////////////////////

#[cfg(feature = "std")]
impl Deserialize for net::IpAddr {
    fn deserialize<D>(deserializer: &mut D) -> Result<Self, D::Error>
        where D: Deserializer,
    {
        let s = try!(String::deserialize(deserializer));
        match s.parse() {
            Ok(s) => Ok(s),
            Err(err) => Err(D::Error::invalid_value(&err.to_string())),
        }
    }
}

#[cfg(feature = "std")]
impl Deserialize for net::Ipv4Addr {
    fn deserialize<D>(deserializer: &mut D) -> Result<Self, D::Error>
        where D: Deserializer,
    {
        let s = try!(String::deserialize(deserializer));
        match s.parse() {
            Ok(s) => Ok(s),
            Err(err) => Err(D::Error::invalid_value(&err.to_string())),
        }
    }
}

#[cfg(feature = "std")]
impl Deserialize for net::Ipv6Addr {
    fn deserialize<D>(deserializer: &mut D) -> Result<Self, D::Error>
        where D: Deserializer,
    {
        let s = try!(String::deserialize(deserializer));
        match s.parse() {
            Ok(s) => Ok(s),
            Err(err) => Err(D::Error::invalid_value(&err.to_string())),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////

#[cfg(feature = "std")]
impl Deserialize for net::SocketAddr {
    fn deserialize<D>(deserializer: &mut D) -> Result<Self, D::Error>
        where D: Deserializer,
    {
        let s = try!(String::deserialize(deserializer));
        match s.parse() {
            Ok(s) => Ok(s),
            Err(err) => Err(D::Error::invalid_value(&err.to_string())),
        }
    }
}

#[cfg(feature = "std")]
impl Deserialize for net::SocketAddrV4 {
    fn deserialize<D>(deserializer: &mut D) -> Result<Self, D::Error>
        where D: Deserializer,
    {
        let s = try!(String::deserialize(deserializer));
        match s.parse() {
            Ok(s) => Ok(s),
            Err(err) => Err(D::Error::invalid_value(&err.to_string())),
        }
    }
}

#[cfg(feature = "std")]
impl Deserialize for net::SocketAddrV6 {
    fn deserialize<D>(deserializer: &mut D) -> Result<Self, D::Error>
        where D: Deserializer,
    {
        let s = try!(String::deserialize(deserializer));
        match s.parse() {
            Ok(s) => Ok(s),
            Err(err) => Err(D::Error::invalid_value(&err.to_string())),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////

#[cfg(feature = "std")]
struct PathBufVisitor;

#[cfg(feature = "std")]
impl Visitor for PathBufVisitor {
    type Value = path::PathBuf;

    fn visit_str<E>(&mut self, v: &str) -> Result<path::PathBuf, E>
        where E: Error,
    {
        Ok(From::from(v))
    }

    fn visit_string<E>(&mut self, v: String) -> Result<path::PathBuf, E>
        where E: Error,
    {
        self.visit_str(&v)
    }
}

#[cfg(feature = "std")]
impl Deserialize for path::PathBuf {
    fn deserialize<D>(deserializer: &mut D) -> Result<path::PathBuf, D::Error>
        where D: Deserializer,
    {
        deserializer.deserialize_string(PathBufVisitor)
    }
}

///////////////////////////////////////////////////////////////////////////////

#[cfg(any(feature = "std", feature = "alloc"))]
impl<T: Deserialize> Deserialize for Box<T> {
    fn deserialize<D>(deserializer: &mut D) -> Result<Box<T>, D::Error>
        where D: Deserializer,
    {
        let val = try!(Deserialize::deserialize(deserializer));
        Ok(Box::new(val))
    }
}

#[cfg(any(feature = "std", feature = "collections"))]
impl<T: Deserialize> Deserialize for Box<[T]> {
    fn deserialize<D>(deserializer: &mut D) -> Result<Box<[T]>, D::Error>
        where D: Deserializer,
    {
        let v: Vec<T> = try!(Deserialize::deserialize(deserializer));
        Ok(v.into_boxed_slice())
    }
}

#[cfg(any(feature = "std", feature = "collections"))]
impl Deserialize for Box<str> {
    fn deserialize<D>(deserializer: &mut D) -> Result<Self, D::Error>
        where D: Deserializer
    {
        let s = try!(String::deserialize(deserializer));
        Ok(s.into_boxed_str())
    }
}

#[cfg(any(feature = "std", feature = "alloc"))]
impl<T: Deserialize> Deserialize for Arc<T> {
    fn deserialize<D>(deserializer: &mut D) -> Result<Arc<T>, D::Error>
        where D: Deserializer,
    {
        let val = try!(Deserialize::deserialize(deserializer));
        Ok(Arc::new(val))
    }
}

#[cfg(any(feature = "std", feature = "alloc"))]
impl<T: Deserialize> Deserialize for Rc<T> {
    fn deserialize<D>(deserializer: &mut D) -> Result<Rc<T>, D::Error>
        where D: Deserializer,
    {
        let val = try!(Deserialize::deserialize(deserializer));
        Ok(Rc::new(val))
    }
}

#[cfg(any(feature = "std", feature = "collections"))]
impl<'a, T: ?Sized> Deserialize for Cow<'a, T> where T: ToOwned, T::Owned: Deserialize, {
    #[inline]
    fn deserialize<D>(deserializer: &mut D) -> Result<Cow<'a, T>, D::Error>
        where D: Deserializer,
    {
        let val = try!(Deserialize::deserialize(deserializer));
        Ok(Cow::Owned(val))
    }
}

///////////////////////////////////////////////////////////////////////////////

// This is a cleaned-up version of the impl generated by:
//
//     #[derive(Deserialize)]
//     #[serde(deny_unknown_fields)]
//     struct Duration {
//         secs: u64,
//         nanos: u32,
//     }
#[cfg(feature = "std")]
impl Deserialize for Duration {
    fn deserialize<D>(deserializer: &mut D) -> Result<Self, D::Error>
        where D: Deserializer,
    {
        enum Field { Secs, Nanos };

        impl Deserialize for Field {
            fn deserialize<D>(deserializer: &mut D) -> Result<Field, D::Error>
                where D: Deserializer,
            {
                struct FieldVisitor;

                impl Visitor for FieldVisitor {
                    type Value = Field;

                    fn visit_usize<E>(&mut self, value: usize) -> Result<Field, E>
                        where E: Error,
                    {
                        match value {
                            0usize => Ok(Field::Secs),
                            1usize => Ok(Field::Nanos),
                            _ => Err(Error::invalid_value("expected a field")),
                        }
                    }

                    fn visit_str<E>(&mut self, value: &str) -> Result<Field, E>
                        where E: Error,
                    {
                        match value {
                            "secs" => Ok(Field::Secs),
                            "nanos" => Ok(Field::Nanos),
                            _ => Err(Error::unknown_field(value)),
                        }
                    }

                    fn visit_bytes<E>(&mut self, value: &[u8]) -> Result<Field, E>
                        where E: Error,
                    {
                        match value {
                            b"secs" => Ok(Field::Secs),
                            b"nanos" => Ok(Field::Nanos),
                            _ => {
                                let value = String::from_utf8_lossy(value);
                                Err(Error::unknown_field(&value))
                            }
                        }
                    }
                }

                deserializer.deserialize_struct_field(FieldVisitor)
            }
        }

        struct DurationVisitor;

        impl Visitor for DurationVisitor {
            type Value = Duration;

            fn visit_seq<V>(&mut self, mut visitor: V) -> Result<Duration, V::Error>
                where V: SeqVisitor,
            {
                let secs: u64 = match try!(visitor.visit()) {
                    Some(value) => value,
                    None => {
                        try!(visitor.end());
                        return Err(Error::invalid_length(0));
                    }
                };
                let nanos: u32 = match try!(visitor.visit()) {
                    Some(value) => value,
                    None => {
                        try!(visitor.end());
                        return Err(Error::invalid_length(1));
                    }
                };
                try!(visitor.end());
                Ok(Duration::new(secs, nanos))
            }

            fn visit_map<V>(&mut self, mut visitor: V) -> Result<Duration, V::Error>
                where V: MapVisitor,
            {
                let mut secs: Option<u64> = None;
                let mut nanos: Option<u32> = None;
                while let Some(key) = try!(visitor.visit_key::<Field>()) {
                    match key {
                        Field::Secs => {
                            if secs.is_some() {
                                return Err(<V::Error as Error>::duplicate_field("secs"));
                            }
                            secs = Some(try!(visitor.visit_value()));
                        }
                        Field::Nanos => {
                            if nanos.is_some() {
                                return Err(<V::Error as Error>::duplicate_field("nanos"));
                            }
                            nanos = Some(try!(visitor.visit_value()));
                        }
                    }
                }
                try!(visitor.end());
                let secs = match secs {
                    Some(secs) => secs,
                    None => try!(visitor.missing_field("secs")),
                };
                let nanos = match nanos {
                    Some(nanos) => nanos,
                    None => try!(visitor.missing_field("nanos")),
                };
                Ok(Duration::new(secs, nanos))
            }
        }

        const FIELDS: &'static [&'static str] = &["secs", "nanos"];
        deserializer.deserialize_struct("Duration", FIELDS, DurationVisitor)
    }
}

///////////////////////////////////////////////////////////////////////////////

#[cfg(feature = "unstable")]
impl<T> Deserialize for NonZero<T> where T: Deserialize + PartialEq + Zeroable + Zero {
    fn deserialize<D>(deserializer: &mut D) -> Result<NonZero<T>, D::Error> where D: Deserializer {
        let value = try!(Deserialize::deserialize(deserializer));
        if value == Zero::zero() {
            return Err(Error::invalid_value("expected a non-zero value"))
        }
        unsafe {
            Ok(NonZero::new(value))
        }
    }
}

///////////////////////////////////////////////////////////////////////////////


impl<T, E> Deserialize for Result<T, E> where T: Deserialize, E: Deserialize {
    fn deserialize<D>(deserializer: &mut D) -> Result<Result<T, E>, D::Error>
                      where D: Deserializer {
        enum Field {
            Ok,
            Err,
        }

        impl Deserialize for Field {
            #[inline]
            fn deserialize<D>(deserializer: &mut D) -> Result<Field, D::Error>
                where D: Deserializer
            {
                struct FieldVisitor;

                impl ::de::Visitor for FieldVisitor {
                    type Value = Field;

                    #[cfg(any(feature = "std", feature = "collections"))]
                    fn visit_usize<E>(&mut self, value: usize) -> Result<Field, E> where E: Error {
                        #[cfg(feature = "collections")]
                        use collections::string::ToString;
                        match value {
                            0 => Ok(Field::Ok),
                            1 => Ok(Field::Err),
                            _ => Err(Error::unknown_field(&value.to_string())),
                        }
                    }

                    #[cfg(all(not(feature = "std"), not(feature = "collections")))]
                    fn visit_usize<E>(&mut self, value: usize) -> Result<Field, E> where E: Error {
                        match value {
                            0 => Ok(Field::Ok),
                            1 => Ok(Field::Err),
                            _ => Err(Error::unknown_field("some number")),
                        }
                    }

                    fn visit_str<E>(&mut self, value: &str) -> Result<Field, E> where E: Error {
                        match value {
                            "Ok" => Ok(Field::Ok),
                            "Err" => Ok(Field::Err),
                            _ => Err(Error::unknown_field(value)),
                        }
                    }

                    fn visit_bytes<E>(&mut self, value: &[u8]) -> Result<Field, E> where E: Error {
                        match value {
                            b"Ok" => Ok(Field::Ok),
                            b"Err" => Ok(Field::Err),
                            _ => {
                                match str::from_utf8(value) {
                                    Ok(value) => Err(Error::unknown_field(value)),
                                    Err(_) => Err(Error::invalid_type(Type::String)),
                                }
                            }
                        }
                    }
                }

                deserializer.deserialize(FieldVisitor)
            }
        }

        struct Visitor<T, E>(PhantomData<Result<T, E>>);

        impl<T, E> EnumVisitor for Visitor<T, E>
            where T: Deserialize,
                  E: Deserialize
        {
            type Value = Result<T, E>;

            fn visit<V>(&mut self, mut visitor: V) -> Result<Result<T, E>, V::Error>
                where V: VariantVisitor
            {
                match try!(visitor.visit_variant()) {
                    Field::Ok => {
                        let value = try!(visitor.visit_newtype());
                        Ok(Ok(value))
                    }
                    Field::Err => {
                        let value = try!(visitor.visit_newtype());
                        Ok(Err(value))
                    }
                }
            }
        }

        const VARIANTS: &'static [&'static str] = &["Ok", "Err"];

        deserializer.deserialize_enum("Result", VARIANTS, Visitor(PhantomData))
    }
}

///////////////////////////////////////////////////////////////////////////////

/// A target for deserializers that want to ignore data. Implements
/// Deserialize and silently eats data given to it.
pub struct IgnoredAny;

impl Deserialize for IgnoredAny {
    #[inline]
    fn deserialize<D>(deserializer: &mut D) -> Result<IgnoredAny, D::Error>
        where D: Deserializer,
    {
        struct IgnoredAnyVisitor;

        impl Visitor for IgnoredAnyVisitor {
            type Value = IgnoredAny;

            #[inline]
            fn visit_bool<E>(&mut self, _: bool) -> Result<IgnoredAny, E> {
                Ok(IgnoredAny)
            }

            #[inline]
            fn visit_i64<E>(&mut self, _: i64) -> Result<IgnoredAny, E> {
                Ok(IgnoredAny)
            }

            #[inline]
            fn visit_u64<E>(&mut self, _: u64) -> Result<IgnoredAny, E> {
                Ok(IgnoredAny)
            }

            #[inline]
            fn visit_f64<E>(&mut self, _: f64) -> Result<IgnoredAny, E> {
                Ok(IgnoredAny)
            }

            #[inline]
            fn visit_str<E>(&mut self, _: &str) -> Result<IgnoredAny, E>
                where E: Error,
            {
                Ok(IgnoredAny)
            }

            #[inline]
            fn visit_none<E>(&mut self) -> Result<IgnoredAny, E> {
                Ok(IgnoredAny)
            }

            #[inline]
            fn visit_some<D>(&mut self, _: &mut D) -> Result<IgnoredAny, D::Error>
                where D: Deserializer,
            {
                Ok(IgnoredAny)
            }

            #[inline]
            fn visit_newtype_struct<D>(&mut self, _: &mut D) -> Result<IgnoredAny, D::Error>
                where D: Deserializer,
            {
                Ok(IgnoredAny)
            }

            #[inline]
            fn visit_unit<E>(&mut self) -> Result<IgnoredAny, E> {
                Ok(IgnoredAny)
            }

            #[inline]
            fn visit_seq<V>(&mut self, mut visitor: V) -> Result<IgnoredAny, V::Error>
                where V: SeqVisitor,
            {
                while let Some(_) = try!(visitor.visit::<IgnoredAny>()) {
                    // Gobble
                }

                try!(visitor.end());
                Ok(IgnoredAny)
            }

            #[inline]
            fn visit_map<V>(&mut self, mut visitor: V) -> Result<IgnoredAny, V::Error>
                where V: MapVisitor,
            {
                while let Some((_, _)) = try!(visitor.visit::<IgnoredAny, IgnoredAny>()) {
                    // Gobble
                }

                try!(visitor.end());
                Ok(IgnoredAny)
            }

            #[inline]
            fn visit_bytes<E>(&mut self, _: &[u8]) -> Result<IgnoredAny, E>
                where E: Error,
            {
                Ok(IgnoredAny)
            }
        }

        // TODO maybe not necessary with impl specialization
        deserializer.deserialize_ignored_any(IgnoredAnyVisitor)
    }
}
