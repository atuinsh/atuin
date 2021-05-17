// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.


use buffer::{BufferResult, RefReadBuffer, RefWriteBuffer};
use symmetriccipher::{Encryptor, Decryptor, SynchronousStreamCipher, SymmetricCipherError};
use cryptoutil::{read_u32_le, symm_enc_or_dec, write_u32v_le};

use cryptoutil::copy_memory;


const ALPHA_MUL_TABLE : [u32; 256] =
[
    0x00000000, 0xE19FCF13, 0x6B973726, 0x8A08F835,
    0xD6876E4C, 0x3718A15F, 0xBD10596A, 0x5C8F9679,
    0x05A7DC98, 0xE438138B, 0x6E30EBBE, 0x8FAF24AD,
    0xD320B2D4, 0x32BF7DC7, 0xB8B785F2, 0x59284AE1,
    0x0AE71199, 0xEB78DE8A, 0x617026BF, 0x80EFE9AC,
    0xDC607FD5, 0x3DFFB0C6, 0xB7F748F3, 0x566887E0,
    0x0F40CD01, 0xEEDF0212, 0x64D7FA27, 0x85483534,
    0xD9C7A34D, 0x38586C5E, 0xB250946B, 0x53CF5B78,
    0x1467229B, 0xF5F8ED88, 0x7FF015BD, 0x9E6FDAAE,
    0xC2E04CD7, 0x237F83C4, 0xA9777BF1, 0x48E8B4E2,
    0x11C0FE03, 0xF05F3110, 0x7A57C925, 0x9BC80636,
    0xC747904F, 0x26D85F5C, 0xACD0A769, 0x4D4F687A,
    0x1E803302, 0xFF1FFC11, 0x75170424, 0x9488CB37,
    0xC8075D4E, 0x2998925D, 0xA3906A68, 0x420FA57B,
    0x1B27EF9A, 0xFAB82089, 0x70B0D8BC, 0x912F17AF,
    0xCDA081D6, 0x2C3F4EC5, 0xA637B6F0, 0x47A879E3,
    0x28CE449F, 0xC9518B8C, 0x435973B9, 0xA2C6BCAA,
    0xFE492AD3, 0x1FD6E5C0, 0x95DE1DF5, 0x7441D2E6,
    0x2D699807, 0xCCF65714, 0x46FEAF21, 0xA7616032,
    0xFBEEF64B, 0x1A713958, 0x9079C16D, 0x71E60E7E,
    0x22295506, 0xC3B69A15, 0x49BE6220, 0xA821AD33,
    0xF4AE3B4A, 0x1531F459, 0x9F390C6C, 0x7EA6C37F,
    0x278E899E, 0xC611468D, 0x4C19BEB8, 0xAD8671AB,
    0xF109E7D2, 0x109628C1, 0x9A9ED0F4, 0x7B011FE7,
    0x3CA96604, 0xDD36A917, 0x573E5122, 0xB6A19E31,
    0xEA2E0848, 0x0BB1C75B, 0x81B93F6E, 0x6026F07D,
    0x390EBA9C, 0xD891758F, 0x52998DBA, 0xB30642A9,
    0xEF89D4D0, 0x0E161BC3, 0x841EE3F6, 0x65812CE5,
    0x364E779D, 0xD7D1B88E, 0x5DD940BB, 0xBC468FA8,
    0xE0C919D1, 0x0156D6C2, 0x8B5E2EF7, 0x6AC1E1E4,
    0x33E9AB05, 0xD2766416, 0x587E9C23, 0xB9E15330,
    0xE56EC549, 0x04F10A5A, 0x8EF9F26F, 0x6F663D7C,
    0x50358897, 0xB1AA4784, 0x3BA2BFB1, 0xDA3D70A2,
    0x86B2E6DB, 0x672D29C8, 0xED25D1FD, 0x0CBA1EEE,
    0x5592540F, 0xB40D9B1C, 0x3E056329, 0xDF9AAC3A,
    0x83153A43, 0x628AF550, 0xE8820D65, 0x091DC276,
    0x5AD2990E, 0xBB4D561D, 0x3145AE28, 0xD0DA613B,
    0x8C55F742, 0x6DCA3851, 0xE7C2C064, 0x065D0F77,
    0x5F754596, 0xBEEA8A85, 0x34E272B0, 0xD57DBDA3,
    0x89F22BDA, 0x686DE4C9, 0xE2651CFC, 0x03FAD3EF,
    0x4452AA0C, 0xA5CD651F, 0x2FC59D2A, 0xCE5A5239,
    0x92D5C440, 0x734A0B53, 0xF942F366, 0x18DD3C75,
    0x41F57694, 0xA06AB987, 0x2A6241B2, 0xCBFD8EA1,
    0x977218D8, 0x76EDD7CB, 0xFCE52FFE, 0x1D7AE0ED,
    0x4EB5BB95, 0xAF2A7486, 0x25228CB3, 0xC4BD43A0,
    0x9832D5D9, 0x79AD1ACA, 0xF3A5E2FF, 0x123A2DEC,
    0x4B12670D, 0xAA8DA81E, 0x2085502B, 0xC11A9F38,
    0x9D950941, 0x7C0AC652, 0xF6023E67, 0x179DF174,
    0x78FBCC08, 0x9964031B, 0x136CFB2E, 0xF2F3343D,
    0xAE7CA244, 0x4FE36D57, 0xC5EB9562, 0x24745A71,
    0x7D5C1090, 0x9CC3DF83, 0x16CB27B6, 0xF754E8A5,
    0xABDB7EDC, 0x4A44B1CF, 0xC04C49FA, 0x21D386E9,
    0x721CDD91, 0x93831282, 0x198BEAB7, 0xF81425A4,
    0xA49BB3DD, 0x45047CCE, 0xCF0C84FB, 0x2E934BE8,
    0x77BB0109, 0x9624CE1A, 0x1C2C362F, 0xFDB3F93C,
    0xA13C6F45, 0x40A3A056, 0xCAAB5863, 0x2B349770,
    0x6C9CEE93, 0x8D032180, 0x070BD9B5, 0xE69416A6,
    0xBA1B80DF, 0x5B844FCC, 0xD18CB7F9, 0x301378EA,
    0x693B320B, 0x88A4FD18, 0x02AC052D, 0xE333CA3E,
    0xBFBC5C47, 0x5E239354, 0xD42B6B61, 0x35B4A472,
    0x667BFF0A, 0x87E43019, 0x0DECC82C, 0xEC73073F,
    0xB0FC9146, 0x51635E55, 0xDB6BA660, 0x3AF46973,
    0x63DC2392, 0x8243EC81, 0x084B14B4, 0xE9D4DBA7,
    0xB55B4DDE, 0x54C482CD, 0xDECC7AF8, 0x3F53B5EB
];

const ALPHA_DIV_TABLE : [u32; 256] =
[
    0x00000000, 0x180F40CD, 0x301E8033, 0x2811C0FE,
    0x603CA966, 0x7833E9AB, 0x50222955, 0x482D6998,
    0xC078FBCC, 0xD877BB01, 0xF0667BFF, 0xE8693B32,
    0xA04452AA, 0xB84B1267, 0x905AD299, 0x88559254,
    0x29F05F31, 0x31FF1FFC, 0x19EEDF02, 0x01E19FCF,
    0x49CCF657, 0x51C3B69A, 0x79D27664, 0x61DD36A9,
    0xE988A4FD, 0xF187E430, 0xD99624CE, 0xC1996403,
    0x89B40D9B, 0x91BB4D56, 0xB9AA8DA8, 0xA1A5CD65,
    0x5249BE62, 0x4A46FEAF, 0x62573E51, 0x7A587E9C,
    0x32751704, 0x2A7A57C9, 0x026B9737, 0x1A64D7FA,
    0x923145AE, 0x8A3E0563, 0xA22FC59D, 0xBA208550,
    0xF20DECC8, 0xEA02AC05, 0xC2136CFB, 0xDA1C2C36,
    0x7BB9E153, 0x63B6A19E, 0x4BA76160, 0x53A821AD,
    0x1B854835, 0x038A08F8, 0x2B9BC806, 0x339488CB,
    0xBBC11A9F, 0xA3CE5A52, 0x8BDF9AAC, 0x93D0DA61,
    0xDBFDB3F9, 0xC3F2F334, 0xEBE333CA, 0xF3EC7307,
    0xA492D5C4, 0xBC9D9509, 0x948C55F7, 0x8C83153A,
    0xC4AE7CA2, 0xDCA13C6F, 0xF4B0FC91, 0xECBFBC5C,
    0x64EA2E08, 0x7CE56EC5, 0x54F4AE3B, 0x4CFBEEF6,
    0x04D6876E, 0x1CD9C7A3, 0x34C8075D, 0x2CC74790,
    0x8D628AF5, 0x956DCA38, 0xBD7C0AC6, 0xA5734A0B,
    0xED5E2393, 0xF551635E, 0xDD40A3A0, 0xC54FE36D,
    0x4D1A7139, 0x551531F4, 0x7D04F10A, 0x650BB1C7,
    0x2D26D85F, 0x35299892, 0x1D38586C, 0x053718A1,
    0xF6DB6BA6, 0xEED42B6B, 0xC6C5EB95, 0xDECAAB58,
    0x96E7C2C0, 0x8EE8820D, 0xA6F942F3, 0xBEF6023E,
    0x36A3906A, 0x2EACD0A7, 0x06BD1059, 0x1EB25094,
    0x569F390C, 0x4E9079C1, 0x6681B93F, 0x7E8EF9F2,
    0xDF2B3497, 0xC724745A, 0xEF35B4A4, 0xF73AF469,
    0xBF179DF1, 0xA718DD3C, 0x8F091DC2, 0x97065D0F,
    0x1F53CF5B, 0x075C8F96, 0x2F4D4F68, 0x37420FA5,
    0x7F6F663D, 0x676026F0, 0x4F71E60E, 0x577EA6C3,
    0xE18D0321, 0xF98243EC, 0xD1938312, 0xC99CC3DF,
    0x81B1AA47, 0x99BEEA8A, 0xB1AF2A74, 0xA9A06AB9,
    0x21F5F8ED, 0x39FAB820, 0x11EB78DE, 0x09E43813,
    0x41C9518B, 0x59C61146, 0x71D7D1B8, 0x69D89175,
    0xC87D5C10, 0xD0721CDD, 0xF863DC23, 0xE06C9CEE,
    0xA841F576, 0xB04EB5BB, 0x985F7545, 0x80503588,
    0x0805A7DC, 0x100AE711, 0x381B27EF, 0x20146722,
    0x68390EBA, 0x70364E77, 0x58278E89, 0x4028CE44,
    0xB3C4BD43, 0xABCBFD8E, 0x83DA3D70, 0x9BD57DBD,
    0xD3F81425, 0xCBF754E8, 0xE3E69416, 0xFBE9D4DB,
    0x73BC468F, 0x6BB30642, 0x43A2C6BC, 0x5BAD8671,
    0x1380EFE9, 0x0B8FAF24, 0x239E6FDA, 0x3B912F17,
    0x9A34E272, 0x823BA2BF, 0xAA2A6241, 0xB225228C,
    0xFA084B14, 0xE2070BD9, 0xCA16CB27, 0xD2198BEA,
    0x5A4C19BE, 0x42435973, 0x6A52998D, 0x725DD940,
    0x3A70B0D8, 0x227FF015, 0x0A6E30EB, 0x12617026,
    0x451FD6E5, 0x5D109628, 0x750156D6, 0x6D0E161B,
    0x25237F83, 0x3D2C3F4E, 0x153DFFB0, 0x0D32BF7D,
    0x85672D29, 0x9D686DE4, 0xB579AD1A, 0xAD76EDD7,
    0xE55B844F, 0xFD54C482, 0xD545047C, 0xCD4A44B1,
    0x6CEF89D4, 0x74E0C919, 0x5CF109E7, 0x44FE492A,
    0x0CD320B2, 0x14DC607F, 0x3CCDA081, 0x24C2E04C,
    0xAC977218, 0xB49832D5, 0x9C89F22B, 0x8486B2E6,
    0xCCABDB7E, 0xD4A49BB3, 0xFCB55B4D, 0xE4BA1B80,
    0x17566887, 0x0F59284A, 0x2748E8B4, 0x3F47A879,
    0x776AC1E1, 0x6F65812C, 0x477441D2, 0x5F7B011F,
    0xD72E934B, 0xCF21D386, 0xE7301378, 0xFF3F53B5,
    0xB7123A2D, 0xAF1D7AE0, 0x870CBA1E, 0x9F03FAD3,
    0x3EA637B6, 0x26A9777B, 0x0EB8B785, 0x16B7F748,
    0x5E9A9ED0, 0x4695DE1D, 0x6E841EE3, 0x768B5E2E,
    0xFEDECC7A, 0xE6D18CB7, 0xCEC04C49, 0xD6CF0C84,
    0x9EE2651C, 0x86ED25D1, 0xAEFCE52F, 0xB6F3A5E2
];


#[derive(Copy)]
pub struct Sosemanuk {
    lfsr: [u32; 10],
    fsm_r: [u32; 2],
    subkeys: [u32; 100],
    output: [u8; 80],
    offset: u32
}

impl Clone for Sosemanuk { fn clone(&self) -> Sosemanuk { *self } }

impl Sosemanuk {
    pub fn new(key: &[u8], nonce: &[u8]) -> Sosemanuk {
        let mut sosemanuk = Sosemanuk { lfsr: [0; 10], fsm_r: [0; 2], subkeys: [0; 100], output: [0; 80], offset: 80 };

        assert!(key.len() <= 32);
        assert!(nonce.len() <= 16);

        key_setup(&key, &mut sosemanuk.subkeys);
        iv_setup(&nonce, &mut sosemanuk.subkeys, &mut sosemanuk.lfsr, &mut sosemanuk.fsm_r);

        sosemanuk
    }

    fn advance_state(&mut self) {
        let mut s0 = self.lfsr[0];
        let mut s1 = self.lfsr[1];
        let mut s2 = self.lfsr[2];
        let mut s3 = self.lfsr[3];
        let mut s4 = self.lfsr[4];
        let mut s5 = self.lfsr[5];
        let mut s6 = self.lfsr[6];
        let mut s7 = self.lfsr[7];
        let mut s8 = self.lfsr[8];
        let mut s9 = self.lfsr[9];
        let mut r1 = self.fsm_r[0];
        let mut r2 = self.fsm_r[1];
        let mut f0 : u32;
        let mut f1 : u32;
        let mut f2 : u32;
        let mut f3 : u32;
        let mut f4 : u32;
        let mut v0 : u32;
        let mut v1 : u32;
        let mut v2 : u32;
        let mut v3 : u32;
        let mut tt : u32;

        let ref mul_alpha = ALPHA_MUL_TABLE;
        let ref div_alpha = ALPHA_DIV_TABLE;

        tt = r1;
        //r1 = r2 + (s1 ^ ((r1 & 0x01) != 0 ? s8 : 0));
        r1 = r2.wrapping_add(s1 ^ match r1 & 0x01 {
            0 => 0,
            _ => s8
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v0 = s0;
        s0 = ((s0 << 8) ^ mul_alpha[s0 as usize >> 24])
            ^ ((s3 >> 8) ^ div_alpha[s3 as usize & 0xFF]) ^ s9;
        f0 = s9.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s2 ^ ((r1 & 0x01) != 0 ? s9 : 0));
        r1 = r2.wrapping_add(s2 ^ match r1 & 0x01 {
            0 => 0,
            _ => s9
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v1 = s1;
        s1 = ((s1 << 8) ^ mul_alpha[s1 as usize >> 24])
            ^ ((s4 >> 8) ^ div_alpha[s4 as usize & 0xFF]) ^ s0;
        f1 = s0.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s3 ^ ((r1 & 0x01) != 0 ? s0 : 0));
        r1 = r2.wrapping_add(s3 ^ match r1 & 0x01 {
            0 => 0,
            _ => s0
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v2 = s2;
        s2 = ((s2 << 8) ^ mul_alpha[s2 as usize >> 24])
            ^ ((s5 >> 8) ^ div_alpha[s5 as usize & 0xFF]) ^ s1;
        f2 = s1.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s4 ^ ((r1 & 0x01) != 0 ? s1 : 0));
        r1 = r2.wrapping_add(s4 ^ match r1 & 0x01 {
            0 => 0,
            _ => s1
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v3 = s3;
        s3 = ((s3 << 8) ^ mul_alpha[s3 as usize >> 24])
            ^ ((s6 >> 8) ^ div_alpha[s6 as usize & 0xFF]) ^ s2;
        f3 = s2.wrapping_add(r1) ^ r2;

        /*
         * Apply the third S-box (number 2) on (f3, f2, f1, f0).
         */
        f4 = f0;
        f0 &= f2;
        f0 ^= f3;
        f2 ^= f1;
        f2 ^= f0;
        f3 |= f4;
        f3 ^= f1;
        f4 ^= f2;
        f1 = f3;
        f3 |= f4;
        f3 ^= f0;
        f0 &= f1;
        f4 ^= f0;
        f1 ^= f3;
        f1 ^= f4;
        f4 = !f4;

        /*
         * S-box result is in (f2, f3, f1, f4).
         */
        let sbox_res = [(f2 ^ v0), (f3 ^ v1), (f1 ^ v2), (f4 ^ v3)];
        write_u32v_le(&mut self.output[0..16], &sbox_res);

        tt = r1;
        //r1 = r2 + (s5 ^ ((r1 & 0x01) != 0 ? s2 : 0));
        r1 = r2.wrapping_add(s5 ^ match r1 & 0x01 {
            0 => 0,
            _ => s2
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v0 = s4;
        s4 = ((s4 << 8) ^ mul_alpha[s4 as usize >> 24])
            ^ ((s7 >> 8) ^ div_alpha[s7 as usize & 0xFF]) ^ s3;
        f0 = s3.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s6 ^ ((r1 & 0x01) != 0 ? s3 : 0));
        r1 = r2.wrapping_add(s6 ^ match r1 & 0x01 {
            0 => 0,
            _ => s3
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v1 = s5;
        s5 = ((s5 << 8) ^ mul_alpha[s5 as usize >> 24])
            ^ ((s8 >> 8) ^ div_alpha[s8 as usize & 0xFF]) ^ s4;
        f1 = s4.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s7 ^ ((r1 & 0x01) != 0 ? s4 : 0));
        r1 = r2.wrapping_add(s7 ^ match r1 & 0x01 {
            0 => 0,
            _ => s4
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v2 = s6;
        s6 = ((s6 << 8) ^ mul_alpha[s6 as usize >> 24])
            ^ ((s9 >> 8) ^ div_alpha[s9 as usize & 0xFF]) ^ s5;
        f2 = s5.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s8 ^ ((r1 & 0x01) != 0 ? s5 : 0));
        r1 = r2.wrapping_add(s8 ^ match r1 & 0x01 {
            0 => 0,
            _ => s5
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v3 = s7;
        s7 = ((s7 << 8) ^ mul_alpha[s7 as usize >> 24])
            ^ ((s0 >> 8) ^ div_alpha[s0 as usize & 0xFF]) ^ s6;
        f3 = s6.wrapping_add(r1) ^ r2;

        /*
         * Apply the third S-box (number 2) on (f3, f2, f1, f0).
         */
        f4 = f0;
        f0 &= f2;
        f0 ^= f3;
        f2 ^= f1;
        f2 ^= f0;
        f3 |= f4;
        f3 ^= f1;
        f4 ^= f2;
        f1 = f3;
        f3 |= f4;
        f3 ^= f0;
        f0 &= f1;
        f4 ^= f0;
        f1 ^= f3;
        f1 ^= f4;
        f4 = !f4;

        /*
         * S-box result is in (f2, f3, f1, f4).
         */
        let sbox_res = [(f2 ^ v0), (f3 ^ v1), (f1 ^ v2), (f4 ^ v3)];
        write_u32v_le(&mut self.output[16..32], &sbox_res);

        tt = r1;
        //r1 = r2 + (s9 ^ ((r1 & 0x01) != 0 ? s6 : 0));
        r1 = r2.wrapping_add(s9 ^ match r1 & 0x01 {
            0 => 0,
            _ => s6
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v0 = s8;
        s8 = ((s8 << 8) ^ mul_alpha[s8 as usize >> 24])
            ^ ((s1 >> 8) ^ div_alpha[s1 as usize & 0xFF]) ^ s7;
        f0 = s7.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s0 ^ ((r1 & 0x01) != 0 ? s7 : 0));
        r1 = r2.wrapping_add(s0 ^ match r1 & 0x01 {
            0 => 0,
            _ => s7
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v1 = s9;
        s9 = ((s9 << 8) ^ mul_alpha[s9 as usize >> 24])
            ^ ((s2 >> 8) ^ div_alpha[s2 as usize & 0xFF]) ^ s8;
        f1 = s8.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s1 ^ ((r1 & 0x01) != 0 ? s8 : 0));
        r1 = r2.wrapping_add(s1 ^ match r1 & 0x01 {
            0 => 0,
            _ => s8
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v2 = s0;
        s0 = ((s0 << 8) ^ mul_alpha[s0 as usize >> 24])
            ^ ((s3 >> 8) ^ div_alpha[s3 as usize & 0xFF]) ^ s9;
        f2 = s9.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s2 ^ ((r1 & 0x01) != 0 ? s9 : 0));
        r1 = r2.wrapping_add(s2 ^ match r1 & 0x01 {
            0 => 0,
            _ => s9
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v3 = s1;
        s1 = ((s1 << 8) ^ mul_alpha[s1 as usize >> 24])
            ^ ((s4 >> 8) ^ div_alpha[s4 as usize & 0xFF]) ^ s0;
        f3 = s0.wrapping_add(r1) ^ r2;

        /*
         * Apply the third S-box (number 2) on (f3, f2, f1, f0).
         */
        f4 = f0;
        f0 &= f2;
        f0 ^= f3;
        f2 ^= f1;
        f2 ^= f0;
        f3 |= f4;
        f3 ^= f1;
        f4 ^= f2;
        f1 = f3;
        f3 |= f4;
        f3 ^= f0;
        f0 &= f1;
        f4 ^= f0;
        f1 ^= f3;
        f1 ^= f4;
        f4 = !f4;

        /*
         * S-box result is in (f2, f3, f1, f4).
         */
        let sbox_res = [(f2 ^ v0), (f3 ^ v1), (f1 ^ v2), (f4 ^ v3)];
        write_u32v_le(&mut self.output[32..48], &sbox_res);

        tt = r1;
        //r1 = r2 + (s3 ^ ((r1 & 0x01) != 0 ? s0 : 0));
        r1 = r2.wrapping_add(s3 ^ match r1 & 0x01 {
            0 => 0,
            _ => s0
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v0 = s2;
        s2 = ((s2 << 8) ^ mul_alpha[s2 as usize >> 24])
            ^ ((s5 >> 8) ^ div_alpha[s5 as usize & 0xFF]) ^ s1;
        f0 = s1.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s4 ^ ((r1 & 0x01) != 0 ? s1 : 0));
        r1 = r2.wrapping_add(s4 ^ match r1 & 0x01 {
            0 => 0,
            _ => s1
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v1 = s3;
        s3 = ((s3 << 8) ^ mul_alpha[s3 as usize >> 24])
            ^ ((s6 >> 8) ^ div_alpha[s6 as usize & 0xFF]) ^ s2;
        f1 = s2.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s5 ^ ((r1 & 0x01) != 0 ? s2 : 0));
        r1 = r2.wrapping_add(s5 ^ match r1 & 0x01 {
            0 => 0,
            _ => s2
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v2 = s4;
        s4 = ((s4 << 8) ^ mul_alpha[s4 as usize >> 24])
            ^ ((s7 >> 8) ^ div_alpha[s7 as usize & 0xFF]) ^ s3;
        f2 = s3.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s6 ^ ((r1 & 0x01) != 0 ? s3 : 0));
        r1 = r2.wrapping_add(s6 ^ match r1 & 0x01 {
            0 => 0,
            _ => s3
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v3 = s5;
        s5 = ((s5 << 8) ^ mul_alpha[s5 as usize >> 24])
            ^ ((s8 >> 8) ^ div_alpha[s8 as usize & 0xFF]) ^ s4;
        f3 = s4.wrapping_add(r1) ^ r2;

        /*
         * Apply the third S-box (number 2) on (f3, f2, f1, f0).
         */
        f4 = f0;
        f0 &= f2;
        f0 ^= f3;
        f2 ^= f1;
        f2 ^= f0;
        f3 |= f4;
        f3 ^= f1;
        f4 ^= f2;
        f1 = f3;
        f3 |= f4;
        f3 ^= f0;
        f0 &= f1;
        f4 ^= f0;
        f1 ^= f3;
        f1 ^= f4;
        f4 = !f4;

        /*
         * S-box result is in (f2, f3, f1, f4).
         */
        let sbox_res = [(f2 ^ v0), (f3 ^ v1), (f1 ^ v2), (f4 ^ v3)];
        write_u32v_le(&mut self.output[48..64], &sbox_res);

        tt = r1;
        //r1 = r2 + (s7 ^ ((r1 & 0x01) != 0 ? s4 : 0));
        r1 = r2.wrapping_add(s7 ^ match r1 & 0x01 {
            0 => 0,
            _ => s4
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v0 = s6;
        s6 = ((s6 << 8) ^ mul_alpha[s6 as usize >> 24])
            ^ ((s9 >> 8) ^ div_alpha[s9 as usize & 0xFF]) ^ s5;
        f0 = s5.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s8 ^ ((r1 & 0x01) != 0 ? s5 : 0));
        r1 = r2.wrapping_add(s8 ^ match r1 & 0x01 {
            0 => 0,
            _ => s5
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v1 = s7;
        s7 = ((s7 << 8) ^ mul_alpha[s7 as usize >> 24])
            ^ ((s0 >> 8) ^ div_alpha[s0 as usize & 0xFF]) ^ s6;
        f1 = s6.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s9 ^ ((r1 & 0x01) != 0 ? s6 : 0));
        r1 = r2.wrapping_add(s9 ^ match r1 & 0x01 {
            0 => 0,
            _ => s6
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v2 = s8;
        s8 = ((s8 << 8) ^ mul_alpha[s8 as usize >> 24])
            ^ ((s1 >> 8) ^ div_alpha[s1 as usize & 0xFF]) ^ s7;
        f2 = s7.wrapping_add(r1) ^ r2;

        tt = r1;
        //r1 = r2 + (s0 ^ ((r1 & 0x01) != 0 ? s7 : 0));
        r1 = r2.wrapping_add(s0 ^ match r1 & 0x01 {
            0 => 0,
            _ => s7
        });
        r2 = tt.wrapping_mul(0x54655307).rotate_left(7);
        v3 = s9;
        s9 = ((s9 << 8) ^ mul_alpha[s9 as usize >> 24])
            ^ ( ( s2 >> 8) ^ div_alpha[s2 as usize & 0xFF]) ^ s8;
        f3 = s8.wrapping_add(r1) ^ r2;

        /*
         * Apply the third S-box (number 2) on (f3, f2, f1, f0).
         */
        f4 = f0;
        f0 &= f2;
        f0 ^= f3;
        f2 ^= f1;
        f2 ^= f0;
        f3 |= f4;
        f3 ^= f1;
        f4 ^= f2;
        f1 = f3;
        f3 |= f4;
        f3 ^= f0;
        f0 &= f1;
        f4 ^= f0;
        f1 ^= f3;
        f1 ^= f4;
        f4 = !f4;

        /*
         * S-box result is in (f2, f3, f1, f4).
         */
        let sbox_res = [(f2 ^ v0), (f3 ^ v1), (f1 ^ v2), (f4 ^ v3)];
        write_u32v_le(&mut self.output[64..80], &sbox_res);

        self.lfsr[0] = s0;
        self.lfsr[1] = s1;
        self.lfsr[2] = s2;
        self.lfsr[3] = s3;
        self.lfsr[4] = s4;
        self.lfsr[5] = s5;
        self.lfsr[6] = s6;
        self.lfsr[7] = s7;
        self.lfsr[8] = s8;
        self.lfsr[9] = s9;
        self.fsm_r[0] = r1;
        self.fsm_r[1] = r2;
        self.offset = 0;
    }

    fn next(&mut self) -> u8 {
        if self.offset == 80 {
            self.advance_state();
        }
        let ret = self.output[self.offset as usize];
        self.offset += 1;
        ret
    }
}


fn key_setup(key : &[u8], subkeys : &mut[u32; 100]) {
    let mut full_key : [u8; 32] = [0; 32];
    if key.len() < 32 {
        copy_memory(&key, &mut full_key[0..key.len()]);
        full_key[key.len()] = 0x01;
    } else {
        copy_memory(&key[0..32], &mut full_key[0..32]);
    }

    let mut w0 = read_u32_le(&full_key[0..4]);
    let mut w1 = read_u32_le(&full_key[4..8]);
    let mut w2 = read_u32_le(&full_key[8..12]);
    let mut w3 = read_u32_le(&full_key[12..16]);
    let mut w4 = read_u32_le(&full_key[16..20]);
    let mut w5 = read_u32_le(&full_key[20..24]);
    let mut w6 = read_u32_le(&full_key[24..28]);
    let mut w7 = read_u32_le(&full_key[28..32]);
    let mut r0 : u32;
    let mut r1 : u32;
    let mut r2 : u32;
    let mut r3 : u32;
    let mut r4 : u32;
    let mut tt : u32;
    let mut i = 0;

    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (0));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (0 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (0 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (0 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r4 = r0;
    r0 |= r3;
    r3 ^= r1;
    r1 &= r4;
    r4 ^= r2;
    r2 ^= r3;
    r3 &= r0;
    r4 |= r1;
    r3 ^= r4;
    r0 ^= r1;
    r4 &= r0;
    r1 ^= r3;
    r4 ^= r2;
    r1 |= r0;
    r1 ^= r2;
    r0 ^= r3;
    r2 = r1;
    r1 |= r3;
    r1 ^= r0;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r4; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (4));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (4 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (4 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (4 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r4 = r0;
    r0 &= r2;
    r0 ^= r3;
    r2 ^= r1;
    r2 ^= r0;
    r3 |= r4;
    r3 ^= r1;
    r4 ^= r2;
    r1 = r3;
    r3 |= r4;
    r3 ^= r0;
    r0 &= r1;
    r4 ^= r0;
    r1 ^= r3;
    r1 ^= r4;
    r4 = !r4;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (8));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (8 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (8 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (8 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r0 = !r0;
    r2 = !r2;
    r4 = r0;
    r0 &= r1;
    r2 ^= r0;
    r0 |= r3;
    r3 ^= r2;
    r1 ^= r0;
    r0 ^= r4;
    r4 |= r1;
    r1 ^= r3;
    r2 |= r0;
    r2 &= r4;
    r0 ^= r1;
    r1 &= r2;
    r1 ^= r0;
    r0 &= r2;
    r0 ^= r4;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r1; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (12));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (12 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (12 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (12 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r3 ^= r0;
    r4 = r1;
    r1 &= r3;
    r4 ^= r2;
    r1 ^= r0;
    r0 |= r3;
    r0 ^= r4;
    r4 ^= r3;
    r3 ^= r2;
    r2 |= r1;
    r2 ^= r4;
    r4 = !r4;
    r4 |= r1;
    r1 ^= r3;
    r1 ^= r4;
    r3 |= r0;
    r1 ^= r3;
    r4 ^= r3;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r0; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (16));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (16 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (16 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (16 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r4 = r1;
    r1 |= r2;
    r1 ^= r3;
    r4 ^= r2;
    r2 ^= r1;
    r3 |= r4;
    r3 &= r0;
    r4 ^= r2;
    r3 ^= r1;
    r1 |= r4;
    r1 ^= r0;
    r0 |= r4;
    r0 ^= r2;
    r1 ^= r4;
    r2 ^= r1;
    r1 &= r0;
    r1 ^= r4;
    r2 = !r2;
    r2 |= r0;
    r4 ^= r2;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r0; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (20));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (20 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (20 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (20 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r2 = !r2;
    r4 = r3;
    r3 &= r0;
    r0 ^= r4;
    r3 ^= r2;
    r2 |= r4;
    r1 ^= r3;
    r2 ^= r0;
    r0 |= r1;
    r2 ^= r1;
    r4 ^= r0;
    r0 |= r3;
    r0 ^= r2;
    r4 ^= r3;
    r4 ^= r0;
    r3 = !r3;
    r2 &= r4;
    r2 ^= r3;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r2; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (24));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (24 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (24 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (24 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r0 ^= r1;
    r1 ^= r3;
    r3 = !r3;
    r4 = r1;
    r1 &= r0;
    r2 ^= r3;
    r1 ^= r2;
    r2 |= r4;
    r4 ^= r3;
    r3 &= r1;
    r3 ^= r0;
    r4 ^= r1;
    r4 ^= r2;
    r2 ^= r0;
    r0 &= r3;
    r2 = !r2;
    r0 ^= r4;
    r4 |= r3;
    r2 ^= r4;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r2; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (28));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (28 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (28 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (28 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r1 ^= r3;
    r3 = !r3;
    r2 ^= r3;
    r3 ^= r0;
    r4 = r1;
    r1 &= r3;
    r1 ^= r2;
    r4 ^= r3;
    r0 ^= r4;
    r2 &= r4;
    r2 ^= r0;
    r0 &= r1;
    r3 ^= r0;
    r4 |= r1;
    r4 ^= r0;
    r0 |= r3;
    r0 ^= r2;
    r2 &= r3;
    r0 = !r0;
    r4 ^= r2;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r3; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (32));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (32 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (32 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (32 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r4 = r0;
    r0 |= r3;
    r3 ^= r1;
    r1 &= r4;
    r4 ^= r2;
    r2 ^= r3;
    r3 &= r0;
    r4 |= r1;
    r3 ^= r4;
    r0 ^= r1;
    r4 &= r0;
    r1 ^= r3;
    r4 ^= r2;
    r1 |= r0;
    r1 ^= r2;
    r0 ^= r3;
    r2 = r1;
    r1 |= r3;
    r1 ^= r0;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r4; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (36));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (36 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (36 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (36 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r4 = r0;
    r0 &= r2;
    r0 ^= r3;
    r2 ^= r1;
    r2 ^= r0;
    r3 |= r4;
    r3 ^= r1;
    r4 ^= r2;
    r1 = r3;
    r3 |= r4;
    r3 ^= r0;
    r0 &= r1;
    r4 ^= r0;
    r1 ^= r3;
    r1 ^= r4;
    r4 = !r4;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (40));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (40 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (40 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (40 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r0 = !r0;
    r2 = !r2;
    r4 = r0;
    r0 &= r1;
    r2 ^= r0;
    r0 |= r3;
    r3 ^= r2;
    r1 ^= r0;
    r0 ^= r4;
    r4 |= r1;
    r1 ^= r3;
    r2 |= r0;
    r2 &= r4;
    r0 ^= r1;
    r1 &= r2;
    r1 ^= r0;
    r0 &= r2;
    r0 ^= r4;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r1; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (44));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (44 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (44 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (44 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r3 ^= r0;
    r4 = r1;
    r1 &= r3;
    r4 ^= r2;
    r1 ^= r0;
    r0 |= r3;
    r0 ^= r4;
    r4 ^= r3;
    r3 ^= r2;
    r2 |= r1;
    r2 ^= r4;
    r4 = !r4;
    r4 |= r1;
    r1 ^= r3;
    r1 ^= r4;
    r3 |= r0;
    r1 ^= r3;
    r4 ^= r3;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r0; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (48));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (48 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (48 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (48 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r4 = r1;
    r1 |= r2;
    r1 ^= r3;
    r4 ^= r2;
    r2 ^= r1;
    r3 |= r4;
    r3 &= r0;
    r4 ^= r2;
    r3 ^= r1;
    r1 |= r4;
    r1 ^= r0;
    r0 |= r4;
    r0 ^= r2;
    r1 ^= r4;
    r2 ^= r1;
    r1 &= r0;
    r1 ^= r4;
    r2 = !r2;
    r2 |= r0;
    r4 ^= r2;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r0; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (52));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (52 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (52 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (52 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r2 = !r2;
    r4 = r3;
    r3 &= r0;
    r0 ^= r4;
    r3 ^= r2;
    r2 |= r4;
    r1 ^= r3;
    r2 ^= r0;
    r0 |= r1;
    r2 ^= r1;
    r4 ^= r0;
    r0 |= r3;
    r0 ^= r2;
    r4 ^= r3;
    r4 ^= r0;
    r3 = !r3;
    r2 &= r4;
    r2 ^= r3;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r2; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (56));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (56 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (56 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (56 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r0 ^= r1;
    r1 ^= r3;
    r3 = !r3;
    r4 = r1;
    r1 &= r0;
    r2 ^= r3;
    r1 ^= r2;
    r2 |= r4;
    r4 ^= r3;
    r3 &= r1;
    r3 ^= r0;
    r4 ^= r1;
    r4 ^= r2;
    r2 ^= r0;
    r0 &= r3;
    r2 = !r2;
    r0 ^= r4;
    r4 |= r3;
    r2 ^= r4;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r2; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (60));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (60 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (60 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (60 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r1 ^= r3;
    r3 = !r3;
    r2 ^= r3;
    r3 ^= r0;
    r4 = r1;
    r1 &= r3;
    r1 ^= r2;
    r4 ^= r3;
    r0 ^= r4;
    r2 &= r4;
    r2 ^= r0;
    r0 &= r1;
    r3 ^= r0;
    r4 |= r1;
    r4 ^= r0;
    r0 |= r3;
    r0 ^= r2;
    r2 &= r3;
    r0 = !r0;
    r4 ^= r2;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r3; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (64));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (64 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (64 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (64 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r4 = r0;
    r0 |= r3;
    r3 ^= r1;
    r1 &= r4;
    r4 ^= r2;
    r2 ^= r3;
    r3 &= r0;
    r4 |= r1;
    r3 ^= r4;
    r0 ^= r1;
    r4 &= r0;
    r1 ^= r3;
    r4 ^= r2;
    r1 |= r0;
    r1 ^= r2;
    r0 ^= r3;
    r2 = r1;
    r1 |= r3;
    r1 ^= r0;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r4; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (68));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (68 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (68 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (68 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r4 = r0;
    r0 &= r2;
    r0 ^= r3;
    r2 ^= r1;
    r2 ^= r0;
    r3 |= r4;
    r3 ^= r1;
    r4 ^= r2;
    r1 = r3;
    r3 |= r4;
    r3 ^= r0;
    r0 &= r1;
    r4 ^= r0;
    r1 ^= r3;
    r1 ^= r4;
    r4 = !r4;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (72));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (72 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (72 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (72 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r0 = !r0;
    r2 = !r2;
    r4 = r0;
    r0 &= r1;
    r2 ^= r0;
    r0 |= r3;
    r3 ^= r2;
    r1 ^= r0;
    r0 ^= r4;
    r4 |= r1;
    r1 ^= r3;
    r2 |= r0;
    r2 &= r4;
    r0 ^= r1;
    r1 &= r2;
    r1 ^= r0;
    r0 &= r2;
    r0 ^= r4;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r1; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (76));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (76 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (76 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (76 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r3 ^= r0;
    r4 = r1;
    r1 &= r3;
    r4 ^= r2;
    r1 ^= r0;
    r0 |= r3;
    r0 ^= r4;
    r4 ^= r3;
    r3 ^= r2;
    r2 |= r1;
    r2 ^= r4;
    r4 = !r4;
    r4 |= r1;
    r1 ^= r3;
    r1 ^= r4;
    r3 |= r0;
    r1 ^= r3;
    r4 ^= r3;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r0; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (80));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (80 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (80 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (80 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r4 = r1;
    r1 |= r2;
    r1 ^= r3;
    r4 ^= r2;
    r2 ^= r1;
    r3 |= r4;
    r3 &= r0;
    r4 ^= r2;
    r3 ^= r1;
    r1 |= r4;
    r1 ^= r0;
    r0 |= r4;
    r0 ^= r2;
    r1 ^= r4;
    r2 ^= r1;
    r1 &= r0;
    r1 ^= r4;
    r2 = !r2;
    r2 |= r0;
    r4 ^= r2;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r0; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (84));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (84 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (84 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (84 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r2 = !r2;
    r4 = r3;
    r3 &= r0;
    r0 ^= r4;
    r3 ^= r2;
    r2 |= r4;
    r1 ^= r3;
    r2 ^= r0;
    r0 |= r1;
    r2 ^= r1;
    r4 ^= r0;
    r0 |= r3;
    r0 ^= r2;
    r4 ^= r3;
    r4 ^= r0;
    r3 = !r3;
    r2 &= r4;
    r2 ^= r3;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r2; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (88));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (88 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (88 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (88 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r0 ^= r1;
    r1 ^= r3;
    r3 = !r3;
    r4 = r1;
    r1 &= r0;
    r2 ^= r3;
    r1 ^= r2;
    r2 |= r4;
    r4 ^= r3;
    r3 &= r1;
    r3 ^= r0;
    r4 ^= r1;
    r4 ^= r2;
    r2 ^= r0;
    r0 &= r3;
    r2 = !r2;
    r0 ^= r4;
    r4 |= r3;
    r2 ^= r4;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r2; i+=1;
    tt = w4 ^ w7 ^ w1 ^ w3 ^ (0x9E3779B9 ^ (92));
    w4 = tt.rotate_left(11);
    tt = w5 ^ w0 ^ w2 ^ w4 ^ (0x9E3779B9 ^ (92 + 1));
    w5 = tt.rotate_left(11);
    tt = w6 ^ w1 ^ w3 ^ w5 ^ (0x9E3779B9 ^ (92 + 2));
    w6 = tt.rotate_left(11);
    tt = w7 ^ w2 ^ w4 ^ w6 ^ (0x9E3779B9 ^ (92 + 3));
    w7 = tt.rotate_left(11);
    r0 = w4;
    r1 = w5;
    r2 = w6;
    r3 = w7;
    r1 ^= r3;
    r3 = !r3;
    r2 ^= r3;
    r3 ^= r0;
    r4 = r1;
    r1 &= r3;
    r1 ^= r2;
    r4 ^= r3;
    r0 ^= r4;
    r2 &= r4;
    r2 ^= r0;
    r0 &= r1;
    r3 ^= r0;
    r4 |= r1;
    r4 ^= r0;
    r0 |= r3;
    r0 ^= r2;
    r2 &= r3;
    r0 = !r0;
    r4 ^= r2;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r4; i+=1;
    subkeys[i] = r0; i+=1;
    subkeys[i] = r3; i+=1;
    tt = w0 ^ w3 ^ w5 ^ w7 ^ (0x9E3779B9 ^ (96));
    w0 = tt.rotate_left(11);
    tt = w1 ^ w4 ^ w6 ^ w0 ^ (0x9E3779B9 ^ (96 + 1));
    w1 = tt.rotate_left(11);
    tt = w2 ^ w5 ^ w7 ^ w1 ^ (0x9E3779B9 ^ (96 + 2));
    w2 = tt.rotate_left(11);
    tt = w3 ^ w6 ^ w0 ^ w2 ^ (0x9E3779B9 ^ (96 + 3));
    w3 = tt.rotate_left(11);
    r0 = w0;
    r1 = w1;
    r2 = w2;
    r3 = w3;
    r4 = r0;
    r0 |= r3;
    r3 ^= r1;
    r1 &= r4;
    r4 ^= r2;
    r2 ^= r3;
    r3 &= r0;
    r4 |= r1;
    r3 ^= r4;
    r0 ^= r1;
    r4 &= r0;
    r1 ^= r3;
    r4 ^= r2;
    r1 |= r0;
    r1 ^= r2;
    r0 ^= r3;
    r2 = r1;
    r1 |= r3;
    r1 ^= r0;
    subkeys[i] = r1; i+=1;
    subkeys[i] = r2; i+=1;
    subkeys[i] = r3; i+=1;
    subkeys[i] = r4;
}

fn iv_setup(iv : &[u8], subkeys : &mut[u32; 100], lfsr : &mut[u32; 10], fsm_r : &mut[u32; 2]) {
    let mut nonce : [u8; 16] = [0; 16];
    if iv.len() < 16 {
        copy_memory(&iv, &mut nonce[0..iv.len()]);
    } else {
        copy_memory(&iv[0..16], &mut nonce[0..16]);
    }

    let mut r0 : u32;
    let mut r1 : u32;
    let mut r2 : u32;
    let mut r3 : u32;
    let mut r4 : u32;
    r0 = read_u32_le(&nonce[0..4]);
    r1 = read_u32_le(&nonce[4..8]);
    r2 = read_u32_le(&nonce[8..12]);
    r3 = read_u32_le(&nonce[12..16]);

    r0 ^= subkeys[0];
    r1 ^= subkeys[0 + 1];
    r2 ^= subkeys[0 + 2];
    r3 ^= subkeys[0 + 3];
    r3 ^= r0;
    r4 = r1;
    r1 &= r3;
    r4 ^= r2;
    r1 ^= r0;
    r0 |= r3;
    r0 ^= r4;
    r4 ^= r3;
    r3 ^= r2;
    r2 |= r1;
    r2 ^= r4;
    r4 = !r4;
    r4 |= r1;
    r1 ^= r3;
    r1 ^= r4;
    r3 |= r0;
    r1 ^= r3;
    r4 ^= r3;
    r1 = r1.rotate_left(13);
    r2 = r2.rotate_left(3);
    r4 = r4 ^ r1 ^ r2;
    r0 = r0 ^ r2 ^ (r1 << 3);
    r4 = r4.rotate_left(1);
    r0 = r0.rotate_left(7);
    r1 = r1 ^ r4 ^ r0;
    r2 = r2 ^ r0 ^ (r4 << 7);
    r1 = r1.rotate_left(5);
    r2 = r2.rotate_left(22);
    r1 ^= subkeys[4];
    r4 ^= subkeys[4 + 1];
    r2 ^= subkeys[4 + 2];
    r0 ^= subkeys[4 + 3];
    r1 = !r1;
    r2 = !r2;
    r3 = r1;
    r1 &= r4;
    r2 ^= r1;
    r1 |= r0;
    r0 ^= r2;
    r4 ^= r1;
    r1 ^= r3;
    r3 |= r4;
    r4 ^= r0;
    r2 |= r1;
    r2 &= r3;
    r1 ^= r4;
    r4 &= r2;
    r4 ^= r1;
    r1 &= r2;
    r1 ^= r3;
    r2 = r2.rotate_left(13);
    r0 = r0.rotate_left(3);
    r1 = r1 ^ r2 ^ r0;
    r4 = r4 ^ r0 ^ (r2 << 3);
    r1 = r1.rotate_left(1);
    r4 = r4.rotate_left(7);
    r2 = r2 ^ r1 ^ r4;
    r0 = r0 ^ r4 ^ (r1 << 7);
    r2 = r2.rotate_left(5);
    r0 = r0.rotate_left(22);
    r2 ^= subkeys[8];
    r1 ^= subkeys[8 + 1];
    r0 ^= subkeys[8 + 2];
    r4 ^= subkeys[8 + 3];
    r3 = r2;
    r2 &= r0;
    r2 ^= r4;
    r0 ^= r1;
    r0 ^= r2;
    r4 |= r3;
    r4 ^= r1;
    r3 ^= r0;
    r1 = r4;
    r4 |= r3;
    r4 ^= r2;
    r2 &= r1;
    r3 ^= r2;
    r1 ^= r4;
    r1 ^= r3;
    r3 = !r3;
    r0 = r0.rotate_left(13);
    r1 = r1.rotate_left(3);
    r4 = r4 ^ r0 ^ r1;
    r3 = r3 ^ r1 ^ (r0 << 3);
    r4 = r4.rotate_left(1);
    r3 = r3.rotate_left(7);
    r0 = r0 ^ r4 ^ r3;
    r1 = r1 ^ r3 ^ (r4 << 7);
    r0 = r0.rotate_left(5);
    r1 = r1.rotate_left(22);
    r0 ^= subkeys[12];
    r4 ^= subkeys[12 + 1];
    r1 ^= subkeys[12 + 2];
    r3 ^= subkeys[12 + 3];
    r2 = r0;
    r0 |= r3;
    r3 ^= r4;
    r4 &= r2;
    r2 ^= r1;
    r1 ^= r3;
    r3 &= r0;
    r2 |= r4;
    r3 ^= r2;
    r0 ^= r4;
    r2 &= r0;
    r4 ^= r3;
    r2 ^= r1;
    r4 |= r0;
    r4 ^= r1;
    r0 ^= r3;
    r1 = r4;
    r4 |= r3;
    r4 ^= r0;
    r4 = r4.rotate_left(13);
    r3 = r3.rotate_left(3);
    r1 = r1 ^ r4 ^ r3;
    r2 = r2 ^ r3 ^ (r4 << 3);
    r1 = r1.rotate_left(1);
    r2 = r2.rotate_left(7);
    r4 = r4 ^ r1 ^ r2;
    r3 = r3 ^ r2 ^ (r1 << 7);
    r4 = r4.rotate_left(5);
    r3 = r3.rotate_left(22);
    r4 ^= subkeys[16];
    r1 ^= subkeys[16 + 1];
    r3 ^= subkeys[16 + 2];
    r2 ^= subkeys[16 + 3];
    r1 ^= r2;
    r2 = !r2;
    r3 ^= r2;
    r2 ^= r4;
    r0 = r1;
    r1 &= r2;
    r1 ^= r3;
    r0 ^= r2;
    r4 ^= r0;
    r3 &= r0;
    r3 ^= r4;
    r4 &= r1;
    r2 ^= r4;
    r0 |= r1;
    r0 ^= r4;
    r4 |= r2;
    r4 ^= r3;
    r3 &= r2;
    r4 = !r4;
    r0 ^= r3;
    r1 = r1.rotate_left(13);
    r4 = r4.rotate_left(3);
    r0 = r0 ^ r1 ^ r4;
    r2 = r2 ^ r4 ^ (r1 << 3);
    r0 = r0.rotate_left(1);
    r2 = r2.rotate_left(7);
    r1 = r1 ^ r0 ^ r2;
    r4 = r4 ^ r2 ^ (r0 << 7);
    r1 = r1.rotate_left(5);
    r4 = r4.rotate_left(22);
    r1 ^= subkeys[20];
    r0 ^= subkeys[20 + 1];
    r4 ^= subkeys[20 + 2];
    r2 ^= subkeys[20 + 3];
    r1 ^= r0;
    r0 ^= r2;
    r2 = !r2;
    r3 = r0;
    r0 &= r1;
    r4 ^= r2;
    r0 ^= r4;
    r4 |= r3;
    r3 ^= r2;
    r2 &= r0;
    r2 ^= r1;
    r3 ^= r0;
    r3 ^= r4;
    r4 ^= r1;
    r1 &= r2;
    r4 = !r4;
    r1 ^= r3;
    r3 |= r2;
    r4 ^= r3;
    r0 = r0.rotate_left(13);
    r1 = r1.rotate_left(3);
    r2 = r2 ^ r0 ^ r1;
    r4 = r4 ^ r1 ^ (r0 << 3);
    r2 = r2.rotate_left(1);
    r4 = r4.rotate_left(7);
    r0 = r0 ^ r2 ^ r4;
    r1 = r1 ^ r4 ^ (r2 << 7);
    r0 = r0.rotate_left(5);
    r1 = r1.rotate_left(22);
    r0 ^= subkeys[24];
    r2 ^= subkeys[24 + 1];
    r1 ^= subkeys[24 + 2];
    r4 ^= subkeys[24 + 3];
    r1 = !r1;
    r3 = r4;
    r4 &= r0;
    r0 ^= r3;
    r4 ^= r1;
    r1 |= r3;
    r2 ^= r4;
    r1 ^= r0;
    r0 |= r2;
    r1 ^= r2;
    r3 ^= r0;
    r0 |= r4;
    r0 ^= r1;
    r3 ^= r4;
    r3 ^= r0;
    r4 = !r4;
    r1 &= r3;
    r1 ^= r4;
    r0 = r0.rotate_left(13);
    r3 = r3.rotate_left(3);
    r2 = r2 ^ r0 ^ r3;
    r1 = r1 ^ r3 ^ (r0 << 3);
    r2 = r2.rotate_left(1);
    r1 = r1.rotate_left(7);
    r0 = r0 ^ r2 ^ r1;
    r3 = r3 ^ r1 ^ (r2 << 7);
    r0 = r0.rotate_left(5);
    r3 = r3.rotate_left(22);
    r0 ^= subkeys[28];
    r2 ^= subkeys[28 + 1];
    r3 ^= subkeys[28 + 2];
    r1 ^= subkeys[28 + 3];
    r4 = r2;
    r2 |= r3;
    r2 ^= r1;
    r4 ^= r3;
    r3 ^= r2;
    r1 |= r4;
    r1 &= r0;
    r4 ^= r3;
    r1 ^= r2;
    r2 |= r4;
    r2 ^= r0;
    r0 |= r4;
    r0 ^= r3;
    r2 ^= r4;
    r3 ^= r2;
    r2 &= r0;
    r2 ^= r4;
    r3 = !r3;
    r3 |= r0;
    r4 ^= r3;
    r4 = r4.rotate_left(13);
    r2 = r2.rotate_left(3);
    r1 = r1 ^ r4 ^ r2;
    r0 = r0 ^ r2 ^ (r4 << 3);
    r1 = r1.rotate_left(1);
    r0 = r0.rotate_left(7);
    r4 = r4 ^ r1 ^ r0;
    r2 = r2 ^ r0 ^ (r1 << 7);
    r4 = r4.rotate_left(5);
    r2 = r2.rotate_left(22);
    r4 ^= subkeys[32];
    r1 ^= subkeys[32 + 1];
    r2 ^= subkeys[32 + 2];
    r0 ^= subkeys[32 + 3];
    r0 ^= r4;
    r3 = r1;
    r1 &= r0;
    r3 ^= r2;
    r1 ^= r4;
    r4 |= r0;
    r4 ^= r3;
    r3 ^= r0;
    r0 ^= r2;
    r2 |= r1;
    r2 ^= r3;
    r3 = !r3;
    r3 |= r1;
    r1 ^= r0;
    r1 ^= r3;
    r0 |= r4;
    r1 ^= r0;
    r3 ^= r0;
    r1 = r1.rotate_left(13);
    r2 = r2.rotate_left(3);
    r3 = r3 ^ r1 ^ r2;
    r4 = r4 ^ r2 ^ (r1 << 3);
    r3 = r3.rotate_left(1);
    r4 = r4.rotate_left(7);
    r1 = r1 ^ r3 ^ r4;
    r2 = r2 ^ r4 ^ (r3 << 7);
    r1 = r1.rotate_left(5);
    r2 = r2.rotate_left(22);
    r1 ^= subkeys[36];
    r3 ^= subkeys[36 + 1];
    r2 ^= subkeys[36 + 2];
    r4 ^= subkeys[36 + 3];
    r1 = !r1;
    r2 = !r2;
    r0 = r1;
    r1 &= r3;
    r2 ^= r1;
    r1 |= r4;
    r4 ^= r2;
    r3 ^= r1;
    r1 ^= r0;
    r0 |= r3;
    r3 ^= r4;
    r2 |= r1;
    r2 &= r0;
    r1 ^= r3;
    r3 &= r2;
    r3 ^= r1;
    r1 &= r2;
    r1 ^= r0;
    r2 = r2.rotate_left(13);
    r4 = r4.rotate_left(3);
    r1 = r1 ^ r2 ^ r4;
    r3 = r3 ^ r4 ^ (r2 << 3);
    r1 = r1.rotate_left(1);
    r3 = r3.rotate_left(7);
    r2 = r2 ^ r1 ^ r3;
    r4 = r4 ^ r3 ^ (r1 << 7);
    r2 = r2.rotate_left(5);
    r4 = r4.rotate_left(22);
    r2 ^= subkeys[40];
    r1 ^= subkeys[40 + 1];
    r4 ^= subkeys[40 + 2];
    r3 ^= subkeys[40 + 3];
    r0 = r2;
    r2 &= r4;
    r2 ^= r3;
    r4 ^= r1;
    r4 ^= r2;
    r3 |= r0;
    r3 ^= r1;
    r0 ^= r4;
    r1 = r3;
    r3 |= r0;
    r3 ^= r2;
    r2 &= r1;
    r0 ^= r2;
    r1 ^= r3;
    r1 ^= r0;
    r0 = !r0;
    r4 = r4.rotate_left(13);
    r1 = r1.rotate_left(3);
    r3 = r3 ^ r4 ^ r1;
    r0 = r0 ^ r1 ^ (r4 << 3);
    r3 = r3.rotate_left(1);
    r0 = r0.rotate_left(7);
    r4 = r4 ^ r3 ^ r0;
    r1 = r1 ^ r0 ^ (r3 << 7);
    r4 = r4.rotate_left(5);
    r1 = r1.rotate_left(22);
    r4 ^= subkeys[44];
    r3 ^= subkeys[44 + 1];
    r1 ^= subkeys[44 + 2];
    r0 ^= subkeys[44 + 3];
    r2 = r4;
    r4 |= r0;
    r0 ^= r3;
    r3 &= r2;
    r2 ^= r1;
    r1 ^= r0;
    r0 &= r4;
    r2 |= r3;
    r0 ^= r2;
    r4 ^= r3;
    r2 &= r4;
    r3 ^= r0;
    r2 ^= r1;
    r3 |= r4;
    r3 ^= r1;
    r4 ^= r0;
    r1 = r3;
    r3 |= r0;
    r3 ^= r4;
    r3 = r3.rotate_left(13);
    r0 = r0.rotate_left(3);
    r1 = r1 ^ r3 ^ r0;
    r2 = r2 ^ r0 ^ (r3 << 3);
    r1 = r1.rotate_left(1);
    r2 = r2.rotate_left(7);
    r3 = r3 ^ r1 ^ r2;
    r0 = r0 ^ r2 ^ (r1 << 7);
    r3 = r3.rotate_left(5);
    r0 = r0.rotate_left(22);
    lfsr[9] = r3;
    lfsr[8] = r1;
    lfsr[7] = r0;
    lfsr[6] = r2;
    r3 ^= subkeys[48];
    r1 ^= subkeys[48 + 1];
    r0 ^= subkeys[48 + 2];
    r2 ^= subkeys[48 + 3];
    r1 ^= r2;
    r2 = !r2;
    r0 ^= r2;
    r2 ^= r3;
    r4 = r1;
    r1 &= r2;
    r1 ^= r0;
    r4 ^= r2;
    r3 ^= r4;
    r0 &= r4;
    r0 ^= r3;
    r3 &= r1;
    r2 ^= r3;
    r4 |= r1;
    r4 ^= r3;
    r3 |= r2;
    r3 ^= r0;
    r0 &= r2;
    r3 = !r3;
    r4 ^= r0;
    r1 = r1.rotate_left(13);
    r3 = r3.rotate_left(3);
    r4 = r4 ^ r1 ^ r3;
    r2 = r2 ^ r3 ^ (r1 << 3);
    r4 = r4.rotate_left(1);
    r2 = r2.rotate_left(7);
    r1 = r1 ^ r4 ^ r2;
    r3 = r3 ^ r2 ^ (r4 << 7);
    r1 = r1.rotate_left(5);
    r3 = r3.rotate_left(22);
    r1 ^= subkeys[52];
    r4 ^= subkeys[52 + 1];
    r3 ^= subkeys[52 + 2];
    r2 ^= subkeys[52 + 3];
    r1 ^= r4;
    r4 ^= r2;
    r2 = !r2;
    r0 = r4;
    r4 &= r1;
    r3 ^= r2;
    r4 ^= r3;
    r3 |= r0;
    r0 ^= r2;
    r2 &= r4;
    r2 ^= r1;
    r0 ^= r4;
    r0 ^= r3;
    r3 ^= r1;
    r1 &= r2;
    r3 = !r3;
    r1 ^= r0;
    r0 |= r2;
    r3 ^= r0;
    r4 = r4.rotate_left(13);
    r1 = r1.rotate_left(3);
    r2 = r2 ^ r4 ^ r1;
    r3 = r3 ^ r1 ^ (r4 << 3);
    r2 = r2.rotate_left(1);
    r3 = r3.rotate_left(7);
    r4 = r4 ^ r2 ^ r3;
    r1 = r1 ^ r3 ^ (r2 << 7);
    r4 = r4.rotate_left(5);
    r1 = r1.rotate_left(22);
    r4 ^= subkeys[56];
    r2 ^= subkeys[56 + 1];
    r1 ^= subkeys[56 + 2];
    r3 ^= subkeys[56 + 3];
    r1 = !r1;
    r0 = r3;
    r3 &= r4;
    r4 ^= r0;
    r3 ^= r1;
    r1 |= r0;
    r2 ^= r3;
    r1 ^= r4;
    r4 |= r2;
    r1 ^= r2;
    r0 ^= r4;
    r4 |= r3;
    r4 ^= r1;
    r0 ^= r3;
    r0 ^= r4;
    r3 = !r3;
    r1 &= r0;
    r1 ^= r3;
    r4 = r4.rotate_left(13);
    r0 = r0.rotate_left(3);
    r2 = r2 ^ r4 ^ r0;
    r1 = r1 ^ r0 ^ (r4 << 3);
    r2 = r2.rotate_left(1);
    r1 = r1.rotate_left(7);
    r4 = r4 ^ r2 ^ r1;
    r0 = r0 ^ r1 ^ (r2 << 7);
    r4 = r4.rotate_left(5);
    r0 = r0.rotate_left(22);
    r4 ^= subkeys[60];
    r2 ^= subkeys[60 + 1];
    r0 ^= subkeys[60 + 2];
    r1 ^= subkeys[60 + 3];
    r3 = r2;
    r2 |= r0;
    r2 ^= r1;
    r3 ^= r0;
    r0 ^= r2;
    r1 |= r3;
    r1 &= r4;
    r3 ^= r0;
    r1 ^= r2;
    r2 |= r3;
    r2 ^= r4;
    r4 |= r3;
    r4 ^= r0;
    r2 ^= r3;
    r0 ^= r2;
    r2 &= r4;
    r2 ^= r3;
    r0 = !r0;
    r0 |= r4;
    r3 ^= r0;
    r3 = r3.rotate_left(13);
    r2 = r2.rotate_left(3);
    r1 = r1 ^ r3 ^ r2;
    r4 = r4 ^ r2 ^ (r3 << 3);
    r1 = r1.rotate_left(1);
    r4 = r4.rotate_left(7);
    r3 = r3 ^ r1 ^ r4;
    r2 = r2 ^ r4 ^ (r1 << 7);
    r3 = r3.rotate_left(5);
    r2 = r2.rotate_left(22);
    r3 ^= subkeys[64];
    r1 ^= subkeys[64 + 1];
    r2 ^= subkeys[64 + 2];
    r4 ^= subkeys[64 + 3];
    r4 ^= r3;
    r0 = r1;
    r1 &= r4;
    r0 ^= r2;
    r1 ^= r3;
    r3 |= r4;
    r3 ^= r0;
    r0 ^= r4;
    r4 ^= r2;
    r2 |= r1;
    r2 ^= r0;
    r0 = !r0;
    r0 |= r1;
    r1 ^= r4;
    r1 ^= r0;
    r4 |= r3;
    r1 ^= r4;
    r0 ^= r4;
    r1 = r1.rotate_left(13);
    r2 = r2.rotate_left(3);
    r0 = r0 ^ r1 ^ r2;
    r3 = r3 ^ r2 ^ (r1 << 3);
    r0 = r0.rotate_left(1);
    r3 = r3.rotate_left(7);
    r1 = r1 ^ r0 ^ r3;
    r2 = r2 ^ r3 ^ (r0 << 7);
    r1 = r1.rotate_left(5);
    r2 = r2.rotate_left(22);
    r1 ^= subkeys[68];
    r0 ^= subkeys[68 + 1];
    r2 ^= subkeys[68 + 2];
    r3 ^= subkeys[68 + 3];
    r1 = !r1;
    r2 = !r2;
    r4 = r1;
    r1 &= r0;
    r2 ^= r1;
    r1 |= r3;
    r3 ^= r2;
    r0 ^= r1;
    r1 ^= r4;
    r4 |= r0;
    r0 ^= r3;
    r2 |= r1;
    r2 &= r4;
    r1 ^= r0;
    r0 &= r2;
    r0 ^= r1;
    r1 &= r2;
    r1 ^= r4;
    r2 = r2.rotate_left(13);
    r3 = r3.rotate_left(3);
    r1 = r1 ^ r2 ^ r3;
    r0 = r0 ^ r3 ^ (r2 << 3);
    r1 = r1.rotate_left(1);
    r0 = r0.rotate_left(7);
    r2 = r2 ^ r1 ^ r0;
    r3 = r3 ^ r0 ^ (r1 << 7);
    r2 = r2.rotate_left(5);
    r3 = r3.rotate_left(22);
    fsm_r[0] = r2;
    lfsr[4] = r1;
    fsm_r[1] = r3;
    lfsr[5] = r0;
    r2 ^= subkeys[72];
    r1 ^= subkeys[72 + 1];
    r3 ^= subkeys[72 + 2];
    r0 ^= subkeys[72 + 3];
    r4 = r2;
    r2 &= r3;
    r2 ^= r0;
    r3 ^= r1;
    r3 ^= r2;
    r0 |= r4;
    r0 ^= r1;
    r4 ^= r3;
    r1 = r0;
    r0 |= r4;
    r0 ^= r2;
    r2 &= r1;
    r4 ^= r2;
    r1 ^= r0;
    r1 ^= r4;
    r4 = !r4;
    r3 = r3.rotate_left(13);
    r1 = r1.rotate_left(3);
    r0 = r0 ^ r3 ^ r1;
    r4 = r4 ^ r1 ^ (r3 << 3);
    r0 = r0.rotate_left(1);
    r4 = r4.rotate_left(7);
    r3 = r3 ^ r0 ^ r4;
    r1 = r1 ^ r4 ^ (r0 << 7);
    r3 = r3.rotate_left(5);
    r1 = r1.rotate_left(22);
    r3 ^= subkeys[76];
    r0 ^= subkeys[76 + 1];
    r1 ^= subkeys[76 + 2];
    r4 ^= subkeys[76 + 3];
    r2 = r3;
    r3 |= r4;
    r4 ^= r0;
    r0 &= r2;
    r2 ^= r1;
    r1 ^= r4;
    r4 &= r3;
    r2 |= r0;
    r4 ^= r2;
    r3 ^= r0;
    r2 &= r3;
    r0 ^= r4;
    r2 ^= r1;
    r0 |= r3;
    r0 ^= r1;
    r3 ^= r4;
    r1 = r0;
    r0 |= r4;
    r0 ^= r3;
    r0 = r0.rotate_left(13);
    r4 = r4.rotate_left(3);
    r1 = r1 ^ r0 ^ r4;
    r2 = r2 ^ r4 ^ (r0 << 3);
    r1 = r1.rotate_left(1);
    r2 = r2.rotate_left(7);
    r0 = r0 ^ r1 ^ r2;
    r4 = r4 ^ r2 ^ (r1 << 7);
    r0 = r0.rotate_left(5);
    r4 = r4.rotate_left(22);
    r0 ^= subkeys[80];
    r1 ^= subkeys[80 + 1];
    r4 ^= subkeys[80 + 2];
    r2 ^= subkeys[80 + 3];
    r1 ^= r2;
    r2 = !r2;
    r4 ^= r2;
    r2 ^= r0;
    r3 = r1;
    r1 &= r2;
    r1 ^= r4;
    r3 ^= r2;
    r0 ^= r3;
    r4 &= r3;
    r4 ^= r0;
    r0 &= r1;
    r2 ^= r0;
    r3 |= r1;
    r3 ^= r0;
    r0 |= r2;
    r0 ^= r4;
    r4 &= r2;
    r0 = !r0;
    r3 ^= r4;
    r1 = r1.rotate_left(13);
    r0 = r0.rotate_left(3);
    r3 = r3 ^ r1 ^ r0;
    r2 = r2 ^ r0 ^ (r1 << 3);
    r3 = r3.rotate_left(1);
    r2 = r2.rotate_left(7);
    r1 = r1 ^ r3 ^ r2;
    r0 = r0 ^ r2 ^ (r3 << 7);
    r1 = r1.rotate_left(5);
    r0 = r0.rotate_left(22);
    r1 ^= subkeys[84];
    r3 ^= subkeys[84 + 1];
    r0 ^= subkeys[84 + 2];
    r2 ^= subkeys[84 + 3];
    r1 ^= r3;
    r3 ^= r2;
    r2 = !r2;
    r4 = r3;
    r3 &= r1;
    r0 ^= r2;
    r3 ^= r0;
    r0 |= r4;
    r4 ^= r2;
    r2 &= r3;
    r2 ^= r1;
    r4 ^= r3;
    r4 ^= r0;
    r0 ^= r1;
    r1 &= r2;
    r0 = !r0;
    r1 ^= r4;
    r4 |= r2;
    r0 ^= r4;
    r3 = r3.rotate_left(13);
    r1 = r1.rotate_left(3);
    r2 = r2 ^ r3 ^ r1;
    r0 = r0 ^ r1 ^ (r3 << 3);
    r2 = r2.rotate_left(1);
    r0 = r0.rotate_left(7);
    r3 = r3 ^ r2 ^ r0;
    r1 = r1 ^ r0 ^ (r2 << 7);
    r3 = r3.rotate_left(5);
    r1 = r1.rotate_left(22);
    r3 ^= subkeys[88];
    r2 ^= subkeys[88 + 1];
    r1 ^= subkeys[88 + 2];
    r0 ^= subkeys[88 + 3];
    r1 = !r1;
    r4 = r0;
    r0 &= r3;
    r3 ^= r4;
    r0 ^= r1;
    r1 |= r4;
    r2 ^= r0;
    r1 ^= r3;
    r3 |= r2;
    r1 ^= r2;
    r4 ^= r3;
    r3 |= r0;
    r3 ^= r1;
    r4 ^= r0;
    r4 ^= r3;
    r0 = !r0;
    r1 &= r4;
    r1 ^= r0;
    r3 = r3.rotate_left(13);
    r4 = r4.rotate_left(3);
    r2 = r2 ^ r3 ^ r4;
    r1 = r1 ^ r4 ^ (r3 << 3);
    r2 = r2.rotate_left(1);
    r1 = r1.rotate_left(7);
    r3 = r3 ^ r2 ^ r1;
    r4 = r4 ^ r1 ^ (r2 << 7);
    r3 = r3.rotate_left(5);
    r4 = r4.rotate_left(22);
    r3 ^= subkeys[92];
    r2 ^= subkeys[92 + 1];
    r4 ^= subkeys[92 + 2];
    r1 ^= subkeys[92 + 3];
    r0 = r2;
    r2 |= r4;
    r2 ^= r1;
    r0 ^= r4;
    r4 ^= r2;
    r1 |= r0;
    r1 &= r3;
    r0 ^= r4;
    r1 ^= r2;
    r2 |= r0;
    r2 ^= r3;
    r3 |= r0;
    r3 ^= r4;
    r2 ^= r0;
    r4 ^= r2;
    r2 &= r3;
    r2 ^= r0;
    r4 = !r4;
    r4 |= r3;
    r0 ^= r4;
    r0 = r0.rotate_left(13);
    r2 = r2.rotate_left(3);
    r1 = r1 ^ r0 ^ r2;
    r3 = r3 ^ r2 ^ (r0 << 3);
    r1 = r1.rotate_left(1);
    r3 = r3.rotate_left(7);
    r0 = r0 ^ r1 ^ r3;
    r2 = r2 ^ r3 ^ (r1 << 7);
    r0 = r0.rotate_left(5);
    r2 = r2.rotate_left(22);
    r0 ^= subkeys[96];
    r1 ^= subkeys[96 + 1];
    r2 ^= subkeys[96 + 2];
    r3 ^= subkeys[96 + 3];
    lfsr[3] = r0;
    lfsr[2] = r1;
    lfsr[1] = r2;
    lfsr[0] = r3;
}


impl SynchronousStreamCipher for Sosemanuk {
    fn process(&mut self, input: &[u8], output: &mut [u8]) {
        assert!(input.len() == output.len());
        for (x, y) in input.iter().zip(output.iter_mut()) {
            *y = *x ^ self.next();
        }
    }
}

impl Encryptor for Sosemanuk {
    fn encrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}

impl Decryptor for Sosemanuk {
    fn decrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}


#[cfg(test)]
mod test {
    use sosemanuk::Sosemanuk;
    use symmetriccipher::SynchronousStreamCipher;
    use serialize::hex::{FromHex};

    // Vectors from http://www.ecrypt.eu.org/stream/svn/viewcvs.cgi/ecrypt/trunk/submissions/sosemanuk/unverified.test-vectors?rev=108&view=markup

    #[test]
    fn test_sosemanuk_ecrypt_set_1_vector_0() {
        let key = "8000000000000000000000000000000000000000000000000000000000000000".from_hex().unwrap();
        let nonce = "00000000000000000000000000000000".from_hex().unwrap();

        let input = [0u8; 64];
        let expected_output_hex = "1782FABFF497A0E89E16E1BCF22F0FE8AA8C566D293AA35B2425E4F26E31C3E7701C08A0D614AF3D3861A7DFF7D6A38A0EFE84A29FADF68D390A3D15B75C972D";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 64];

        let mut sosemanuk = Sosemanuk::new(key.as_ref(), nonce.as_ref());
        sosemanuk.process(&input, &mut output);
        let expected: &[u8] = expected_output.as_ref();
        assert!(output.as_ref() == expected);
    }

    #[test]
    fn test_sosemanuk_ecrypt_set_2_vector_63() {
        let key = "3F3F3F3F3F3F3F3F3F3F3F3F3F3F3F3F".from_hex().unwrap();
        let nonce = "00000000000000000000000000000000".from_hex().unwrap();

        let input = [0u8; 64];
        let expected_output_hex = "7D755F30A2B747A50D7D28147EDF0B3E3FAB6856A7373C7306C00D1D4076969354D7AB4343C0115E7839502C5C699ED06DB119968AEBFD08D8B968A7161D613F";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 64];

        let mut sosemanuk = Sosemanuk::new(key.as_ref(), nonce.as_ref());
        sosemanuk.process(&input, &mut output);
        let expected: &[u8] = expected_output.as_ref();
        assert!(output.as_ref() == expected);
    }

    #[test]
    fn test_sosemanuk_ecrypt_set_2_vector_90() {
        let key = "5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A".from_hex().unwrap();
        let nonce = "00000000000000000000000000000000".from_hex().unwrap();

        let input = [0u8; 64];
        let expected_output_hex = "F5D7D72686322D1751AFD16A1DD98282D2B9A1EE0C305DF52F86AE1B831E90C22E2DE089CEE656A992736385D9135B823B3611098674BF820986A4342B89ABF7";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 64];

        let mut sosemanuk = Sosemanuk::new(key.as_ref(), nonce.as_ref());
        sosemanuk.process(&input, &mut output);
        let expected: &[u8] = expected_output.as_ref();
        assert!(output.as_ref() == expected);
    }

    #[test]
    fn test_sosemanuk_ecrypt_set_3_vector_135() {
        let key = "8788898A8B8C8D8E8F909192939495969798999A9B9C9D9E9FA0A1A2A3A4".from_hex().unwrap();
        let nonce = "00000000000000000000000000000000".from_hex().unwrap();

        let input = [0u8; 64];
        let expected_output_hex = "9D7EE5A10BBB0756D66B8DAA5AE08F41B05C9E7C6B13532EAA81F224282B61C66DEEE5AF6251DB26C49B865C5AD4250AE89787FC86C35409CF2986CF820293AA";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 64];

        let mut sosemanuk = Sosemanuk::new(key.as_ref(), nonce.as_ref());
        sosemanuk.process(&input, &mut output);
        let expected: &[u8] = expected_output.as_ref();
        assert!(output.as_ref() == expected);
    }

    #[test]
    fn test_sosemanuk_ecrypt_set_3_vector_207() {
        let key = "CFD0D1D2D3D4D5D6D7D8D9DADBDCDDDEDFE0E1E2E3E4E5E6E7E8E9EAEBECEDEE".from_hex().unwrap();
        let nonce = "00000000000000000000000000000000".from_hex().unwrap();

        let input = [0u8; 64];
        let expected_output_hex = "F028923659C6C0A17065E013368D93EBCF2F4FD892B6E27E104EF0A2605708EA26336AE966D5058BC144F7954FE2FC3C258F00734AA5BEC8281814B746197084";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 64];

        let mut sosemanuk = Sosemanuk::new(key.as_ref(), nonce.as_ref());
        sosemanuk.process(&input, &mut output);
        let expected: &[u8] = expected_output.as_ref();
        assert!(output.as_ref() == expected);
    }

    #[test]
    fn test_sosemanuk_ecrypt_set_6_vector_3() {
        let key = "0F62B5085BAE0154A7FA4DA0F34699EC3F92E5388BDE3184D72A7DD02376C91C".from_hex().unwrap();
        let nonce = "288FF65DC42B92F960C72E95FC63CA31".from_hex().unwrap();

        let input = [0u8; 64];
        let expected_output_hex = "1FC4F2E266B21C24FDDB3492D40A3FA6DE32CDF13908511E84420ABDFA1D3B0FEC600F83409C57CBE0394B90CDB1D759243EFD8B8E2AB7BC453A8D8A3515183E";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 64];

        let mut sosemanuk = Sosemanuk::new(key.as_ref(), nonce.as_ref());
        sosemanuk.process(&input, &mut output);
        let expected: &[u8] = expected_output.as_ref();
        assert!(output.as_ref() == expected);
    }

    // From TEST_VECTOR_128.txt from reference C implementation

    #[test]
    fn test_sosemanuk_vector128_test1() {
        let key = "A7C083FEB7".from_hex().unwrap();
        let nonce = "00112233445566778899AABBCCDDEEFF".from_hex().unwrap();

        let input = [0u8; 160];
        let expected_output_hex = "FE81D2162C9A100D04895C454A77515BBE6A431A935CB90E2221EBB7EF502328943539492EFF6310C871054C2889CC728F82E86B1AFFF4334B6127A13A155C75151630BD482EB673FF5DB477FA6C53EBE1A4EC38C23C5400C315455D93A2ACED9598604727FA340D5F2A8BD757B77833F74BD2BC049313C80616B4A06268AE350DB92EEC4FA56C171374A67A80C006D0EAD048CE7B640F17D3D5A62D1F251C21";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 160];

        let mut sosemanuk = Sosemanuk::new(key.as_ref(), nonce.as_ref());
        sosemanuk.process(&input, &mut output);
        let expected: &[u8] = expected_output.as_ref();
        assert!(output.as_ref() == expected);
    }

    #[test]
    fn test_sosemanuk_vector128_test2() {
        let key = "00112233445566778899AABBCCDDEEFF".from_hex().unwrap();
        let nonce = "8899AABBCCDDEEFF0011223344556677".from_hex().unwrap();

        let input = [0u8; 160];
        let expected_output_hex = "FA61DBEB71178131A77C714BD2EABF4E1394207A25698AA1308F2F063A0F760604CF67569BA59A3DFAD7F00145C78D29C5FFE5F964950486424451952C84039D234D9C37EECBBCA1EBFB0DD16EA1194A6AFC1A460E33E33FE8D55C48977079C687810D74FEDDEE1B3986218FB1E1C1765E4DF64D7F6911C19A270C59C74B24461717F86CE3B11808FACD4F2E714168DA44CF6360D54DDA2241BCB79401A4EDCC";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 160];

        let mut sosemanuk = Sosemanuk::new(key.as_ref(), nonce.as_ref());
        sosemanuk.process(&input, &mut output);
        let expected: &[u8] = expected_output.as_ref();
        assert!(output.as_ref() == expected);
    }
}

#[cfg(all(test, feature = "with-bench"))]
mod bench {
    use test::Bencher;
    use symmetriccipher::SynchronousStreamCipher;
    use sosemanuk::Sosemanuk;

    #[bench]
    pub fn sosemanuk_10(bh: & mut Bencher) {
        let mut sosemanuk = Sosemanuk::new(&[0; 32], &[0; 16]);
        let input = [1u8; 10];
        let mut output = [0u8; 10];
        bh.iter( || {
            sosemanuk.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }

    #[bench]
    pub fn sosemanuk_1k(bh: & mut Bencher) {
        let mut sosemanuk = Sosemanuk::new(&[0; 32], &[0; 16]);
        let input = [1u8; 1024];
        let mut output = [0u8; 1024];
        bh.iter( || {
            sosemanuk.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }

    #[bench]
    pub fn sosemanuk_64k(bh: & mut Bencher) {
        let mut sosemanuk = Sosemanuk::new(&[0; 32], &[0; 16]);
        let input = [1u8; 65536];
        let mut output = [0u8; 65536];
        bh.iter( || {
            sosemanuk.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }
}
