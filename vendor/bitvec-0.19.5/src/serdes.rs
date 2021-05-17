/*! `serde`-powered de/serialization.

This module implements the Serde traits for the `bitvec` types.

`BitSlice` is able to implement `Serialize`, but `serde` does not provide a
mechanism for deserializing into data borrowed from the calling context. Thus,
`BitSlice` can only deserialize into `BitBox` or `BitVec`, when built with the
`"alloc"` feature.

`BitBox` and `BitVec` implement serialization through `BitSlice`, and
deserialize `BitSlice`s into themselves.

`BitArray` has different behavior: because it always spans the full memory
buffer, and has no partial edge elements, it de/serializes the underlying memory
array without any additional information. It is currently incapable of
deserializing the stream produced by serializing `BitSlice`, and can only
deserialize the streams produced by `BitArray`s and ordinary arrays.

If you require de/serialization compatibility between `BitArray` and the other
structures, please file an issue.
!*/

#![cfg(feature = "serde")]

use crate::{
	array::BitArray,
	devel as dvl,
	domain::Domain,
	index::BitRegister,
	mem::BitMemory,
	order::BitOrder,
	pointer::BitPtr,
	slice::BitSlice,
	store::BitStore,
	view::BitView,
};

use core::{
	cmp,
	convert::TryInto,
	fmt::{
		self,
		Formatter,
	},
	marker::PhantomData,
	mem::ManuallyDrop,
	ptr,
};

use serde::{
	de::{
		self,
		Deserialize,
		Deserializer,
		MapAccess,
		SeqAccess,
		Unexpected,
		Visitor,
	},
	ser::{
		Serialize,
		SerializeSeq,
		SerializeStruct,
		Serializer,
	},
};

use tap::pipe::Pipe;

#[cfg(feature = "alloc")]
use crate::{
	boxed::BitBox,
	vec::BitVec,
};

impl<O, T> Serialize for BitSlice<O, T>
where
	O: BitOrder,
	T: BitStore,
	T::Mem: Serialize,
{
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where S: Serializer {
		let head = self.bitptr().head();
		let mut state = serializer.serialize_struct("BitSet", 3)?;

		state.serialize_field("head", &head.value())?;
		state.serialize_field("bits", &(self.len() as u64))?;
		state.serialize_field("data", &self.domain())?;

		state.end()
	}
}

impl<T> Serialize for Domain<'_, T>
where
	T: BitStore,
	T::Mem: Serialize,
{
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where S: Serializer {
		let mut state = serializer.serialize_seq(Some(self.len()))?;
		for elem in *self {
			state.serialize_element(&elem)?;
		}
		state.end()
	}
}

impl<O, V> Serialize for BitArray<O, V>
where
	O: BitOrder,
	V: BitView,
	V::Mem: Serialize,
{
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where S: Serializer {
		self.as_raw_slice().serialize(serializer)
	}
}

#[cfg(feature = "alloc")]
#[cfg(not(tarpaulin_include))]
impl<O, T> Serialize for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
	T::Mem: Serialize,
{
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where S: Serializer {
		self.as_bitslice().serialize(serializer)
	}
}

#[cfg(feature = "alloc")]
#[cfg(not(tarpaulin_include))]
impl<O, T> Serialize for BitVec<O, T>
where
	O: BitOrder,
	T: BitStore,
	T::Mem: Serialize,
{
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where S: Serializer {
		self.as_bitslice().serialize(serializer)
	}
}

impl<'de, O, T> Deserialize<'de> for BitArray<O, T>
where
	O: BitOrder,
	T: BitStore + BitRegister,
	T::Mem: Deserialize<'de>,
{
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where D: Deserializer<'de> {
		deserializer
			.pipe(<T::Mem as Deserialize<'de>>::deserialize)?
			.pipe(dvl::remove_mem::<T>)
			.pipe(Self::new)
			.pipe(Ok)
	}
}

macro_rules! deser_array {
	($($n:expr),+ $(,)?) => { $(
		impl<'de, O, T> Deserialize<'de> for BitArray<O, [T; $n]>
		where
			O: BitOrder,
			T: BitStore,
			T::Mem: Deserialize<'de>
		{
			fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
			where D: Deserializer<'de> {
				deserializer
					.pipe(<[T::Mem; $n] as Deserialize<'de>>::deserialize)?
					.pipe(|arr| unsafe { ptr::read(&arr as *const [T::Mem; $n] as *const [T; $n]) })
					.pipe(Self::new)
					.pipe(Ok)
			}
		}
	)+ };
}

deser_array!(
	0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
	21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32
);

#[cfg(feature = "alloc")]
#[derive(Clone, Copy, Debug, Default)]
struct BitVecVisitor<'de, O, T>
where
	O: BitOrder,
	T: BitStore,
	T::Mem: Deserialize<'de>,
{
	_lt: PhantomData<&'de ()>,
	_bs: PhantomData<BitVec<O, T>>,
}

#[cfg(feature = "alloc")]
impl<'de, O, T> BitVecVisitor<'de, O, T>
where
	O: BitOrder,
	T: BitStore,
	T::Mem: Deserialize<'de>,
{
	const THIS: Self = Self {
		_lt: PhantomData,
		_bs: PhantomData,
	};

	/// Constructs a `BitVec` from deserialized components.
	///
	/// # Parameters
	///
	/// - `&self`: A visitor, only needed for access to an error message.
	/// - `head`: The deserialized head-bit index.
	/// - `bits`: The deserialized length counter.
	/// - `data`: A vector of memory containing the bitslice. Its dest
	///
	/// # Returns
	///
	/// The result of assembling the deserialized components into a `BitVec`.
	/// This can fail if the `head` is invalid, or if the deserialized data
	/// cannot be encoded into a `BitPtr`.
	fn assemble<E>(
		&self,
		head: u8,
		bits: usize,
		data: Vec<T::Mem>,
	) -> Result<<Self as Visitor<'de>>::Value, E>
	where
		E: de::Error,
	{
		//  Disable the destructor on the deserialized buffer
		let data = ManuallyDrop::new(data);
		//  Assemble a region pointer
		BitPtr::new(
			data.as_ptr() as *mut T,
			//  Attempt to read the `head` index as a `BitIdx` bounded by the
			//  destination type.
			head.try_into().map_err(|_| {
				de::Error::invalid_value(
					Unexpected::Unsigned(head as u64),
					&"a head-bit index less than the deserialized element \
					  type’s bit width",
				)
			})?,
			//  Ensure that the `bits` counter is not lying about the data size.
			cmp::min(bits, data.len().saturating_mul(T::Mem::BITS as usize)),
		)
		//  Fail if the source cannot be encoded into a bit pointer.
		.ok_or_else(|| {
			de::Error::invalid_value(
				Unexpected::Other("invalid bit-region source data"),
				self,
			)
		})?
		//  No more branches remain, only typesystem manipulation,
		.pipe(BitPtr::to_bitslice_ptr_mut)
		.pipe(|bp| unsafe { BitVec::from_raw_parts(bp, data.capacity()) })
		.pipe(Ok)
	}
}

#[cfg(feature = "alloc")]
impl<'de, O, T> Visitor<'de> for BitVecVisitor<'de, O, T>
where
	O: BitOrder,
	T: BitStore,
	T::Mem: Deserialize<'de>,
{
	type Value = BitVec<O, T>;

	fn expecting(&self, fmt: &mut Formatter) -> fmt::Result {
		fmt.write_str("a BitSet data series")
	}

	/// Visit a sequence of anonymous data elements. These must be in the order
	/// `u8` (head-bit index), `u64` (length counter), `[T::Mem]` (data
	/// contents).
	fn visit_seq<V>(self, mut seq: V) -> Result<Self::Value, V::Error>
	where V: SeqAccess<'de> {
		let head = seq
			.next_element::<u8>()?
			.ok_or_else(|| de::Error::invalid_length(0, &self))?;
		let bits = seq
			.next_element::<u64>()?
			.ok_or_else(|| de::Error::invalid_length(1, &self))?;
		let data = seq
			.next_element::<Vec<T::Mem>>()?
			.ok_or_else(|| de::Error::invalid_length(2, &self))?;

		self.assemble(head, bits as usize, data)
	}

	/// Visit a map of named data elements. These may be in any order, and must
	/// be the pairs `head: u8`, `bits: u64`, and `data: [T::Mem]`.
	fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
	where V: MapAccess<'de> {
		let mut head: Option<u8> = None;
		let mut bits: Option<u64> = None;
		let mut data: Option<Vec<T::Mem>> = None;

		while let Some(key) = map.next_key()? {
			match key {
				"head" => {
					if head.replace(map.next_value()?).is_some() {
						return Err(de::Error::duplicate_field("head"));
					}
				},
				"bits" => {
					if bits.replace(map.next_value()?).is_some() {
						return Err(de::Error::duplicate_field("bits"));
					}
				},
				"data" => {
					if data.replace(map.next_value()?).is_some() {
						return Err(de::Error::duplicate_field("data"));
					}
				},
				f => {
					return Err(de::Error::unknown_field(f, &[
						"head", "bits", "data",
					]));
				},
			}
		}
		let head = head.ok_or_else(|| de::Error::missing_field("head"))?;
		let bits = bits.ok_or_else(|| de::Error::missing_field("bits"))?;
		let data = data.ok_or_else(|| de::Error::missing_field("data"))?;

		self.assemble(head, bits as usize, data)
	}
}

#[cfg(feature = "alloc")]
impl<'de, O, T> Deserialize<'de> for BitBox<O, T>
where
	O: BitOrder,
	T: BitStore,
	T::Mem: Deserialize<'de>,
{
	#[inline]
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where D: Deserializer<'de> {
		deserializer
			.pipe(<BitVec<O, T> as Deserialize<'de>>::deserialize)
			.map(BitVec::into_boxed_bitslice)
	}
}

#[cfg(feature = "alloc")]
impl<'de, O, T> Deserialize<'de> for BitVec<O, T>
where
	O: BitOrder,
	T: BitStore,
	T::Mem: Deserialize<'de>,
{
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where D: Deserializer<'de> {
		deserializer.deserialize_struct(
			"BitSet",
			&["head", "bits", "data"],
			BitVecVisitor::THIS,
		)
	}
}

#[cfg(test)]
mod tests {
	use crate::prelude::*;

	use serde_test::{
		assert_ser_tokens,
		Token,
	};

	#[cfg(feature = "alloc")]
	use serde_test::{
		assert_de_tokens,
		assert_de_tokens_error,
	};

	macro_rules! bvtok {
		( s $elts:expr, $head:expr, $bits:expr, $ty:ident $( , $data:expr )* ) => {
			&[
				Token::Struct { name: "BitSet", len: 3, },
				Token::Str("head"), Token::U8( $head ),
				Token::Str("bits"), Token::U64( $bits ),
				Token::Str("data"), Token::Seq { len: Some( $elts ) },
				$( Token:: $ty ( $data ), )*
				Token::SeqEnd,
				Token::StructEnd,
			]
		};
		( d $elts:expr, $head:expr, $bits:expr, $ty:ident $( , $data:expr )* ) => {
			&[
				Token::Struct { name: "BitSet", len: 3, },
				Token::BorrowedStr("head"), Token::U8( $head ),
				Token::BorrowedStr("bits"), Token::U64( $bits ),
				Token::BorrowedStr("data"), Token::Seq { len: Some( $elts ) },
				$( Token:: $ty ( $data ), )*
				Token::SeqEnd,
				Token::StructEnd,
			]
		};
	}

	#[test]
	fn empty() {
		let slice = BitSlice::<Msb0, u8>::empty();

		assert_ser_tokens(&slice, bvtok![s 0, 0, 0, U8]);

		#[cfg(feature = "alloc")]
		assert_de_tokens(&bitvec![], bvtok![ d 0, 0, 0, U8 ]);
	}

	#[test]
	fn small() {
		let bits = 0b1111_1000u8.view_bits::<Msb0>();
		let bits = &bits[1 .. 5];
		assert_ser_tokens(&bits, bvtok![s 1, 1, 4, U8, 0b1111_1000]);

		let bits = 0b00001111_11111111u16.view_bits::<Lsb0>();
		let bits = &bits[.. 12];
		assert_ser_tokens(&bits, bvtok![s 1, 0, 12, U16, 0b00001111_11111111]);

		let bits = 0b11_11111111u32.view_bits::<LocalBits>();
		let bits = &bits[.. 10];
		assert_ser_tokens(&bits, bvtok![s 1, 0, 10, U32, 0x00_00_03_FF]);
	}

	#[test]
	#[cfg(feature = "alloc")]
	fn wide() {
		let src: &[u8] = &[0, !0];
		let bs = src.view_bits::<LocalBits>();
		assert_ser_tokens(&(&bs[1 .. 15]), bvtok![s 2, 1, 14, U8, 0, !0]);
	}

	#[test]
	#[cfg(feature = "alloc")]
	fn deser() {
		let bv = bitvec![Msb0, u8; 0, 1, 1, 0, 1, 0];
		assert_de_tokens(&bv, bvtok![d 1, 0, 6, U8, 0b0110_1000]);
		//  test that the bits outside the bits domain don't matter in deser
		assert_de_tokens(&bv, bvtok![d 1, 0, 6, U8, 0b0110_1001]);
		assert_de_tokens(&bv, bvtok![d 1, 0, 6, U8, 0b0110_1010]);
		assert_de_tokens(&bv, bvtok![d 1, 0, 6, U8, 0b0110_1011]);
	}

	#[test]
	#[cfg(feature = "alloc")]
	fn error_paths() {
		assert_de_tokens_error::<BitVec<Msb0, u8>>(
			bvtok!(d 0, 9, 0, U8),
			"invalid value: integer `9`, expected a head-bit index less than \
			 the deserialized element type’s bit width",
		);
	}
}
