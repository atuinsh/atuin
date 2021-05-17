//! Tests for the `field` module.

use super::*;

#[test]
fn get_value() {
	let data = [5u32 << 3, 0x01234567, !5];
	let bits = data.view_bits::<Lsb0>();

	let (head, elem, tail) = bits[3 .. 6].domain().enclave().unwrap();
	let byte = get::<u32, u8>(elem, Lsb0::mask(head, tail), 3);
	assert_eq!(byte, 5u8);

	let (head, body, tail) = bits[32 .. 48].domain().region().unwrap();
	assert!(head.is_none());
	assert!(body.is_empty());
	let (elem, tail) = tail.unwrap();
	let short = get::<u32, u16>(elem, Lsb0::mask(None, tail), 0);
	assert_eq!(short, 0x4567u16);

	let (head, body, tail) = bits[48 .. 64].domain().region().unwrap();
	assert!(tail.is_none());
	assert!(body.is_empty());
	let (head, elem) = head.unwrap();
	let short = get::<u32, u16>(elem, Lsb0::mask(head, None), 16);
	assert_eq!(short, 0x0123u16);

	let (head, body, tail) = bits[64 .. 96].domain().region().unwrap();
	assert!(head.is_none());
	assert_eq!(body, &[!5]);
	assert!(tail.is_none());
}

#[test]
fn set_value() {
	let mut data = [0u32; 3];
	let bits = data.view_bits_mut::<Lsb0>();

	let (head, elem, tail) = bits[3 .. 6].domain_mut().enclave().unwrap();
	set::<u32, u16>(elem, 13u16, Lsb0::mask(head, tail), 3);

	let (head, body, tail) = bits[32 .. 48].domain_mut().region().unwrap();
	assert!(head.is_none());
	assert!(body.is_empty());
	let (elem, tail) = tail.unwrap();
	set::<u32, u16>(elem, 0x4567u16, Lsb0::mask(None, tail), 0);

	let (head, body, tail) = bits[48 .. 64].domain_mut().region().unwrap();
	assert!(tail.is_none());
	assert!(body.is_empty());
	let (head, elem) = head.unwrap();
	set::<u32, u16>(elem, 0x0123u16, Lsb0::mask(head, None), 16);

	assert_eq!(data[0], 5 << 3);
	assert_eq!(data[1], 0x01234567u32);
}

#[test]
fn byte_fields() {
	let mut data = [0u8; 3];

	data.view_bits_mut::<Msb0>()[4 .. 20].store_be(0xABCDu16);
	assert_eq!(data, [0x0A, 0xBC, 0xD0]);
	assert_eq!(data.view_bits::<Msb0>()[4 .. 20].load_be::<u16>(), 0xABCD);

	data.view_bits_mut::<Msb0>()[2 .. 6].store_be(9u8);
	assert_eq!(data, [0x26, 0xBC, 0xD0]);
	assert_eq!(data.view_bits::<Msb0>()[2 .. 6].load_be::<u8>(), 9);

	data = [0; 3];
	data.view_bits_mut::<Lsb0>()[4 .. 20].store_be(0xABCDu16);
	assert_eq!(data, [0xA0, 0xBC, 0x0D]);
	assert_eq!(data.view_bits::<Lsb0>()[4 .. 20].load_be::<u16>(), 0xABCD);

	data.view_bits_mut::<Lsb0>()[2 .. 6].store_be(9u8);
	//  0b1010_0000 | 0b00_1001_00
	assert_eq!(data, [0xA4, 0xBC, 0x0D]);
	assert_eq!(data.view_bits::<Lsb0>()[2 .. 6].load_be::<u8>(), 9);

	data = [0; 3];
	data.view_bits_mut::<Msb0>()[4 .. 20].store_le(0xABCDu16);
	assert_eq!(data, [0x0D, 0xBC, 0xA0]);
	assert_eq!(data.view_bits::<Msb0>()[4 .. 20].load_le::<u16>(), 0xABCD);

	data = [0; 3];
	data.view_bits_mut::<Lsb0>()[4 .. 20].store_le(0xABCDu16);
	assert_eq!(data, [0xD0, 0xBC, 0x0A]);
	assert_eq!(data.view_bits::<Lsb0>()[4 .. 20].load_le::<u16>(), 0xABCD);
}

#[test]
fn wide_load() {
	let mut data = bitarr![Lsb0, u16; 0; 256];
	assert_eq!(data[16 .. 144].load::<u128>(), 0u128);
	data[16 .. 144].store(!0u128);
	assert_eq!(data[16 .. 144].load::<u128>(), !0u128);
}

#[test]
#[should_panic]
fn check_panic() {
	check("fail", 10, 8);
}
