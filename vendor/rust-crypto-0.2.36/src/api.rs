// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

enum DigestSpec {
    Md5,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Blake2b,
    Ripemd160,
    Whirlpool,
}

enum MacSpec {
    Hmac,
}

enum BlockModeSpec {
    Ebc,
    Cbc,
    Ctr,
}

enum BlodeModePaddingSpec {
    NoPadding,
    Pkcs,
}

enum SymmetricCipherSpec {
    Aes,
    Des,
    Rc4,
    Blowfish,
    ChaCha20,
    Hc128,
    Salsa20,
    XSalsa20,
    Sosemanuk,
}

enum KdfSpec {
    Pbkdf2,
    Bcrypt,
    Scrypt,
}
