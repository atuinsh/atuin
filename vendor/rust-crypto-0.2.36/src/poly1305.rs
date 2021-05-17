// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// This is a port of Andrew Moons poly1305-donna
// https://github.com/floodyberry/poly1305-donna

use std::cmp::min;

use cryptoutil::{read_u32_le, write_u32_le};
use mac::{Mac, MacResult};

#[derive(Clone, Copy)]
pub struct Poly1305 {
    r         : [u32; 5],
    h         : [u32; 5],
    pad       : [u32; 4],
    leftover  : usize,
    buffer    : [u8; 16],
    finalized : bool,
}

impl Poly1305 {
    pub fn new(key: &[u8]) -> Poly1305 {
        assert!(key.len() == 32);
        let mut poly = Poly1305{ r: [0u32; 5], h: [0u32; 5], pad: [0u32; 4], leftover: 0, buffer: [0u8; 16], finalized: false };

        // r &= 0xffffffc0ffffffc0ffffffc0fffffff
        poly.r[0] = (read_u32_le(&key[0..4])     ) & 0x3ffffff;
        poly.r[1] = (read_u32_le(&key[3..7]) >> 2) & 0x3ffff03;
        poly.r[2] = (read_u32_le(&key[6..10]) >> 4) & 0x3ffc0ff;
        poly.r[3] = (read_u32_le(&key[9..13]) >> 6) & 0x3f03fff;
        poly.r[4] = (read_u32_le(&key[12..16]) >> 8) & 0x00fffff;

        poly.pad[0] = read_u32_le(&key[16..20]);
        poly.pad[1] = read_u32_le(&key[20..24]);
        poly.pad[2] = read_u32_le(&key[24..28]);
        poly.pad[3] = read_u32_le(&key[28..32]);

        poly
    }

    fn block(&mut self, m: &[u8]) {
        let hibit : u32 = if self.finalized { 0 } else { 1 << 24 };

        let r0 = self.r[0];
        let r1 = self.r[1];
        let r2 = self.r[2];
        let r3 = self.r[3];
        let r4 = self.r[4];

        let s1 = r1 * 5;
        let s2 = r2 * 5;
        let s3 = r3 * 5;
        let s4 = r4 * 5;

        let mut h0 = self.h[0];
        let mut h1 = self.h[1];
        let mut h2 = self.h[2];
        let mut h3 = self.h[3];
        let mut h4 = self.h[4];

        // h += m
        h0 += (read_u32_le(&m[0..4])     ) & 0x3ffffff;
        h1 += (read_u32_le(&m[3..7]) >> 2) & 0x3ffffff;
        h2 += (read_u32_le(&m[6..10]) >> 4) & 0x3ffffff;
        h3 += (read_u32_le(&m[9..13]) >> 6) & 0x3ffffff;
        h4 += (read_u32_le(&m[12..16]) >> 8) | hibit;

        // h *= r
        let     d0 = (h0 as u64 * r0 as u64) + (h1 as u64 * s4 as u64) + (h2 as u64 * s3 as u64) + (h3 as u64 * s2 as u64) + (h4 as u64 * s1 as u64);
        let mut d1 = (h0 as u64 * r1 as u64) + (h1 as u64 * r0 as u64) + (h2 as u64 * s4 as u64) + (h3 as u64 * s3 as u64) + (h4 as u64 * s2 as u64);
        let mut d2 = (h0 as u64 * r2 as u64) + (h1 as u64 * r1 as u64) + (h2 as u64 * r0 as u64) + (h3 as u64 * s4 as u64) + (h4 as u64 * s3 as u64);
        let mut d3 = (h0 as u64 * r3 as u64) + (h1 as u64 * r2 as u64) + (h2 as u64 * r1 as u64) + (h3 as u64 * r0 as u64) + (h4 as u64 * s4 as u64);
        let mut d4 = (h0 as u64 * r4 as u64) + (h1 as u64 * r3 as u64) + (h2 as u64 * r2 as u64) + (h3 as u64 * r1 as u64) + (h4 as u64 * r0 as u64);

        // (partial) h %= p
        let mut c : u32;
                        c = (d0 >> 26) as u32; h0 = d0 as u32 & 0x3ffffff;
        d1 += c as u64; c = (d1 >> 26) as u32; h1 = d1 as u32 & 0x3ffffff;
        d2 += c as u64; c = (d2 >> 26) as u32; h2 = d2 as u32 & 0x3ffffff;
        d3 += c as u64; c = (d3 >> 26) as u32; h3 = d3 as u32 & 0x3ffffff;
        d4 += c as u64; c = (d4 >> 26) as u32; h4 = d4 as u32 & 0x3ffffff;
        h0 += c * 5;    c = h0 >> 26; h0 = h0 & 0x3ffffff;
        h1 += c;

        self.h[0] = h0;
        self.h[1] = h1;
        self.h[2] = h2;
        self.h[3] = h3;
        self.h[4] = h4;
    }

    fn finish(&mut self) {
        if self.leftover > 0 {
            self.buffer[self.leftover] = 1;
            for i in self.leftover+1..16 {
                self.buffer[i] = 0;
            }
            self.finalized = true;
            let tmp = self.buffer;
            self.block(&tmp);
        }

        // fully carry h
        let mut h0 = self.h[0];
        let mut h1 = self.h[1];
        let mut h2 = self.h[2];
        let mut h3 = self.h[3];
        let mut h4 = self.h[4];

        let mut c : u32;
                     c = h1 >> 26; h1 = h1 & 0x3ffffff;
        h2 +=     c; c = h2 >> 26; h2 = h2 & 0x3ffffff;
        h3 +=     c; c = h3 >> 26; h3 = h3 & 0x3ffffff;
        h4 +=     c; c = h4 >> 26; h4 = h4 & 0x3ffffff;
        h0 += c * 5; c = h0 >> 26; h0 = h0 & 0x3ffffff;
        h1 +=     c;

        // compute h + -p
        let mut g0 = h0.wrapping_add(5); c = g0 >> 26; g0 &= 0x3ffffff;
        let mut g1 = h1.wrapping_add(c); c = g1 >> 26; g1 &= 0x3ffffff;
        let mut g2 = h2.wrapping_add(c); c = g2 >> 26; g2 &= 0x3ffffff;
        let mut g3 = h3.wrapping_add(c); c = g3 >> 26; g3 &= 0x3ffffff;
        let mut g4 = h4.wrapping_add(c).wrapping_sub(1 << 26);

        // select h if h < p, or h + -p if h >= p
        let mut mask = (g4 >> (32 - 1)).wrapping_sub(1);
        g0 &= mask;
        g1 &= mask;
        g2 &= mask;
        g3 &= mask;
        g4 &= mask;
        mask = !mask;
        h0 = (h0 & mask) | g0;
        h1 = (h1 & mask) | g1;
        h2 = (h2 & mask) | g2;
        h3 = (h3 & mask) | g3;
        h4 = (h4 & mask) | g4;

        // h = h % (2^128)
        h0 = ((h0      ) | (h1 << 26)) & 0xffffffff;
        h1 = ((h1 >>  6) | (h2 << 20)) & 0xffffffff;
        h2 = ((h2 >> 12) | (h3 << 14)) & 0xffffffff;
        h3 = ((h3 >> 18) | (h4 <<  8)) & 0xffffffff;

        // h = mac = (h + pad) % (2^128)
        let mut f : u64;
        f = h0 as u64 + self.pad[0] as u64            ; h0 = f as u32;
        f = h1 as u64 + self.pad[1] as u64 + (f >> 32); h1 = f as u32;
        f = h2 as u64 + self.pad[2] as u64 + (f >> 32); h2 = f as u32;
        f = h3 as u64 + self.pad[3] as u64 + (f >> 32); h3 = f as u32;

        self.h[0] = h0;
        self.h[1] = h1;
        self.h[2] = h2;
        self.h[3] = h3;
    }
}

impl Mac for Poly1305 {
    fn input(&mut self, data: &[u8]) {
        assert!(!self.finalized);
        let mut m = data;

        if self.leftover > 0 {
            let want = min(16 - self.leftover, m.len());
            for i in 0..want {
                self.buffer[self.leftover+i] = m[i];
            }
            m = &m[want..];
            self.leftover += want;

            if self.leftover < 16 {
                return;
            }

            // self.block(self.buffer[..]);
            let tmp = self.buffer;
            self.block(&tmp);

            self.leftover = 0;
        }

        while m.len() >= 16 {
            self.block(&m[0..16]);
            m = &m[16..];
        }

        for i in 0..m.len() {
            self.buffer[i] = m[i];
        }
        self.leftover = m.len();
    }

    fn reset(&mut self) {
        self.h = [0u32; 5];
        self.leftover = 0;
        self.finalized = false;
    }

    fn result(&mut self) -> MacResult {
        let mut mac = [0u8; 16];
        self.raw_result(&mut mac);
        MacResult::new(&mac[..])
    }

    fn raw_result(&mut self, output: &mut [u8]) {
        assert!(output.len() >= 16);
        if !self.finalized{
            self.finish();
        }
        write_u32_le(&mut output[0..4], self.h[0]);
        write_u32_le(&mut output[4..8], self.h[1]);
        write_u32_le(&mut output[8..12], self.h[2]);
        write_u32_le(&mut output[12..16], self.h[3]);
    }

    fn output_bytes(&self) -> usize { 16 }
}

#[cfg(test)]
mod test {
    use std::iter::repeat;

    use poly1305::Poly1305;
    use mac::Mac;

    fn poly1305(key: &[u8], msg: &[u8], mac: &mut [u8]) {
        let mut poly = Poly1305::new(key);
        poly.input(msg);
        poly.raw_result(mac);
    }

    #[test]
    fn test_nacl_vector() {
        let key = [
            0xee,0xa6,0xa7,0x25,0x1c,0x1e,0x72,0x91,
            0x6d,0x11,0xc2,0xcb,0x21,0x4d,0x3c,0x25,
            0x25,0x39,0x12,0x1d,0x8e,0x23,0x4e,0x65,
            0x2d,0x65,0x1f,0xa4,0xc8,0xcf,0xf8,0x80,
        ];

        let msg = [
            0x8e,0x99,0x3b,0x9f,0x48,0x68,0x12,0x73,
            0xc2,0x96,0x50,0xba,0x32,0xfc,0x76,0xce,
            0x48,0x33,0x2e,0xa7,0x16,0x4d,0x96,0xa4,
            0x47,0x6f,0xb8,0xc5,0x31,0xa1,0x18,0x6a,
            0xc0,0xdf,0xc1,0x7c,0x98,0xdc,0xe8,0x7b,
            0x4d,0xa7,0xf0,0x11,0xec,0x48,0xc9,0x72,
            0x71,0xd2,0xc2,0x0f,0x9b,0x92,0x8f,0xe2,
            0x27,0x0d,0x6f,0xb8,0x63,0xd5,0x17,0x38,
            0xb4,0x8e,0xee,0xe3,0x14,0xa7,0xcc,0x8a,
            0xb9,0x32,0x16,0x45,0x48,0xe5,0x26,0xae,
            0x90,0x22,0x43,0x68,0x51,0x7a,0xcf,0xea,
            0xbd,0x6b,0xb3,0x73,0x2b,0xc0,0xe9,0xda,
            0x99,0x83,0x2b,0x61,0xca,0x01,0xb6,0xde,
            0x56,0x24,0x4a,0x9e,0x88,0xd5,0xf9,0xb3,
            0x79,0x73,0xf6,0x22,0xa4,0x3d,0x14,0xa6,
            0x59,0x9b,0x1f,0x65,0x4c,0xb4,0x5a,0x74,
            0xe3,0x55,0xa5,
        ];

        let expected = [
            0xf3,0xff,0xc7,0x70,0x3f,0x94,0x00,0xe5,
            0x2a,0x7d,0xfb,0x4b,0x3d,0x33,0x05,0xd9,
        ];

        let mut mac = [0u8; 16];
        poly1305(&key, &msg, &mut mac);
        assert_eq!(&mac[..], &expected[..]);

        let mut poly = Poly1305::new(&key);
        poly.input(&msg[0..32]);
        poly.input(&msg[32..96]);
        poly.input(&msg[96..112]);
        poly.input(&msg[112..120]);
        poly.input(&msg[120..124]);
        poly.input(&msg[124..126]);
        poly.input(&msg[126..127]);
        poly.input(&msg[127..128]);
        poly.input(&msg[128..129]);
        poly.input(&msg[129..130]);
        poly.input(&msg[130..131]);
        poly.raw_result(&mut mac);
        assert_eq!(&mac[..], &expected[..]);
    }

    #[test]
    fn donna_self_test() {
        let wrap_key = [
            0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        let wrap_msg = [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        ];

        let wrap_mac = [
            0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        let mut mac = [0u8; 16];
        poly1305(&wrap_key, &wrap_msg, &mut mac);
        assert_eq!(&mac[..], &wrap_mac[..]);

        let total_key = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0xff,
            0xfe, 0xfd, 0xfc, 0xfb, 0xfa, 0xf9, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00,
        ];

        let total_mac = [
            0x64, 0xaf, 0xe2, 0xe8, 0xd6, 0xad, 0x7b, 0xbd,
            0xd2, 0x87, 0xf9, 0x7c, 0x44, 0x62, 0x3d, 0x39,
        ];

        let mut tpoly = Poly1305::new(&total_key);
        for i in 0..256 {
            let key: Vec<u8> = repeat(i as u8).take(32).collect();
            let msg: Vec<u8> = repeat(i as u8).take(256).collect();
            let mut mac = [0u8; 16];
            poly1305(&key[..], &msg[0..i], &mut mac);
            tpoly.input(&mac);
        }
        tpoly.raw_result(&mut mac);
        assert_eq!(&mac[..], &total_mac[..]);
    }

    #[test]
    fn test_tls_vectors() {
        // from http://tools.ietf.org/html/draft-agl-tls-chacha20poly1305-04
        let key = b"this is 32-byte key for Poly1305";
        let msg = [0u8; 32];
        let expected = [
            0x49, 0xec, 0x78, 0x09, 0x0e, 0x48, 0x1e, 0xc6,
            0xc2, 0x6b, 0x33, 0xb9, 0x1c, 0xcc, 0x03, 0x07,
        ];
        let mut mac = [0u8; 16];
        poly1305(key, &msg, &mut mac);
        assert_eq!(&mac[..], &expected[..]);

        let msg = b"Hello world!";
        let expected= [
            0xa6, 0xf7, 0x45, 0x00, 0x8f, 0x81, 0xc9, 0x16,
            0xa2, 0x0d, 0xcc, 0x74, 0xee, 0xf2, 0xb2, 0xf0,
        ];
        poly1305(key, msg, &mut mac);
        assert_eq!(&mac[..], &expected[..]);
    }
}

#[cfg(all(test, feature = "with-bench"))]
mod bench {
    use test::Bencher;
    use mac::Mac;
    use poly1305::Poly1305;

    #[bench]
    pub fn poly1305_10(bh: & mut Bencher) {
        let mut mac = [0u8; 16];
        let key     = [0u8; 32];
        let bytes   = [1u8; 10];
        bh.iter( || {
            let mut poly = Poly1305::new(&key);
            poly.input(&bytes);
            poly.raw_result(&mut mac);
        });
        bh.bytes = bytes.len() as u64;
    }

    #[bench]
    pub fn poly1305_1k(bh: & mut Bencher) {
        let mut mac = [0u8; 16];
        let key     = [0u8; 32];
        let bytes   = [1u8; 1024];
        bh.iter( || {
            let mut poly = Poly1305::new(&key);
            poly.input(&bytes);
            poly.raw_result(&mut mac);
        });
        bh.bytes = bytes.len() as u64;
    }

    #[bench]
    pub fn poly1305_64k(bh: & mut Bencher) {
        let mut mac = [0u8; 16];
        let key     = [0u8; 32];
        let bytes   = [1u8; 65536];
        bh.iter( || {
            let mut poly = Poly1305::new(&key);
            poly.input(&bytes);
            poly.raw_result(&mut mac);
        });
        bh.bytes = bytes.len() as u64;
    }
}
