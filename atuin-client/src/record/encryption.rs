use atuin_common::record::{DecryptedData, Record};
use base64::{engine::general_purpose, Engine};
use eyre::{ensure, Context, ContextCompat, Result};
use rusty_paseto::prelude::{
    Footer, Key, Local, Paseto, PasetoNonce, PasetoSymmetricKey, Payload, V4,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub struct EncryptedData(pub Vec<u8>);

pub trait Encrypt {
    type Output;
    type Key;

    fn encrypt(self, key: &Self::Key) -> Self::Output;
}

pub trait Decrypt {
    type Output;
    type Key;

    fn decrypt(self, key: &Self::Key) -> Result<Self::Output>;
}

impl Encrypt for DecryptedData {
    type Output = EncryptedData;
    type Key = PasetoSymmetricKey<V4, Local>;

    fn encrypt(self, key: &Self::Key) -> Self::Output {
        let ptk =
            PasetoSymmetricKey::from(Key::try_new_random().expect("could not source from random"));

        let footer = AtuinFooter {
            wpk: paserk::wrap::pie::local_wrap_v4(&ptk, key),
            kid: paserk::id::encode_lid(key),
        };

        let payload = general_purpose::URL_SAFE.encode(self.0);
        let footer = serde_json::to_string(&footer).expect("could not serialize footer");
        let nonce = Key::<32>::try_new_random().expect("could not source from random");
        let nonce = PasetoNonce::<V4, Local>::from(&nonce);

        let token = Paseto::<V4, Local>::builder()
            .set_payload(Payload::from(payload.as_str()))
            .set_footer(Footer::from(footer.as_str()))
            .try_encrypt(&ptk, &nonce)
            .expect("error encrypting atuin data");

        EncryptedData(token.into_bytes())
    }
}

impl Encrypt for Record<DecryptedData> {
    type Output = Record<EncryptedData>;
    type Key = PasetoSymmetricKey<V4, Local>;

    fn encrypt(self, key: &Self::Key) -> Self::Output {
        Record {
            id: self.id,
            host: self.host,
            parent: self.parent,
            timestamp: self.timestamp,
            version: self.version,
            tag: self.tag,
            data: self.data.encrypt(key),
        }
    }
}

impl Decrypt for EncryptedData {
    type Output = DecryptedData;
    type Key = PasetoSymmetricKey<V4, Local>;

    fn decrypt(self, key: &Self::Key) -> Result<Self::Output> {
        let token = String::from_utf8(self.0).context("token is not utf8")?;
        let (_, footer) = token.rsplit_once('.').context("token has no footer")?;
        let footer = general_purpose::URL_SAFE_NO_PAD
            .decode(footer)
            .context("footer was not valid base64url")?;
        let footer = std::str::from_utf8(&footer).context("footer was not valid utf8")?;
        let AtuinFooter { kid, wpk } =
            serde_json::from_str(footer).context("footer did not contain the correct contents")?;

        let current_kid = paserk::id::encode_lid(key);
        ensure!(
            current_kid == kid,
            "attempting to decrypt with incorrect key. currently using {current_kid}, expecting {kid}"
        );

        let ptk = paserk::wrap::pie::local_unwrap_v4(&wpk, key)?;

        let payload = Paseto::<V4, Local>::try_decrypt(&token, &ptk, Footer::from(footer), None)
            .context("could not decrypt entry")?;

        let data = general_purpose::URL_SAFE.decode(payload)?;
        Ok(DecryptedData(data))
    }
}

impl Decrypt for Record<EncryptedData> {
    type Output = Record<DecryptedData>;
    type Key = PasetoSymmetricKey<V4, Local>;

    fn decrypt(self, key: &Self::Key) -> Result<Self::Output> {
        Ok(Record {
            id: self.id,
            host: self.host,
            parent: self.parent,
            timestamp: self.timestamp,
            version: self.version,
            tag: self.tag,
            data: self.data.decrypt(key)?,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct AtuinFooter {
    /// Wrapped key
    wpk: String,
    /// ID of the key which was used to wrap
    kid: String,
}

#[test]
fn round_trip() {
    let key = PasetoSymmetricKey::from(Key::try_new_random().unwrap());

    let data = DecryptedData(vec![1, 2, 3, 4]);

    let encrypted = data.clone().encrypt(&key);
    let encrypted2 = data.clone().encrypt(&key);

    assert_ne!(
        encrypted, encrypted2,
        "re-encrypting the same contents should have different output due to key randomization"
    );

    let decrypted = encrypted.decrypt(&key).unwrap();
    assert_eq!(decrypted, data);

    let fake_key = PasetoSymmetricKey::from(Key::try_new_random().unwrap());
    let _ = encrypted2.decrypt(&fake_key).map(|_| ()).unwrap_err();
}

/// Platform-Agnostic Serialized Keys
///
/// <https://github.com/paseto-standard/paserk>
///
/// Unfortunately, the rusty_paseto library doesn't implement this. but they're not too complicated.
mod paserk {
    /// Key ID encodings
    ///
    /// <https://github.com/paseto-standard/paserk/blob/master/operations/ID.md>
    pub mod id {
        use std::io::Write;

        use base64::{engine::general_purpose, write::EncoderStringWriter};
        use blake2::{digest::Digest, Blake2b};
        use generic_array::typenum::U33;
        use rusty_paseto::prelude::{Local, PasetoSymmetricKey, V4};

        /// local-id <https://github.com/paseto-standard/paserk/blob/master/types/lid.md>
        pub fn encode_lid(key: &PasetoSymmetricKey<V4, Local>) -> String {
            let h = "k4.lid.";

            let mut derive_d = Blake2b::<U33>::new();
            derive_d.update(h);
            derive_d.update(key.as_ref());
            let d = derive_d.finalize();

            let mut enc =
                EncoderStringWriter::from_consumer(h.to_owned(), &general_purpose::URL_SAFE);
            enc.write_all(h.as_bytes()).unwrap();
            enc.write_all(&d).unwrap();
            enc.into_inner()
        }
    }

    pub mod wrap {

        /// Paragon Initiative Enterprises standard key-wrapping
        /// <https://github.com/paseto-standard/paserk/blob/master/operations/Wrap/pie.md>
        pub mod pie {
            use std::io::Write;

            use base64::{engine::general_purpose, write::EncoderStringWriter, Engine};
            use blake2::{digest::Mac, Blake2bMac};
            use chacha20::{
                cipher::{inout::InOutBuf, KeyIvInit, StreamCipher},
                XChaCha20,
            };
            use eyre::{ensure, Context, ContextCompat, Result};
            use generic_array::{
                sequence::Split,
                typenum::{U24, U32, U56},
                GenericArray,
            };
            use rand::{rngs::OsRng, RngCore};
            use rusty_paseto::prelude::{Key, Local, PasetoSymmetricKey, V4};
            use subtle::ConstantTimeEq;

            /// Implementation of <https://github.com/paseto-standard/paserk/blob/master/operations/Wrap/pie.md#v2v4-encryption>
            pub fn local_wrap_v4(
                ptk: &PasetoSymmetricKey<V4, Local>,
                wk: &PasetoSymmetricKey<V4, Local>,
            ) -> String {
                let h = "k4.local-wrap.pie.";

                // step 1: Enforce Algorithm Lucidity
                // asserted by the function type signature.

                // step 2: Generate a 256 bit (32 bytes) random nonce, n.
                let mut n = [0u8; 32];
                OsRng.fill_bytes(&mut n);

                // step 3: Derive the encryption key `Ek` and XChaCha nonce `n2`
                let mut derive_ek = Blake2bMac::<U56>::new_from_slice(wk.as_ref()).unwrap();
                derive_ek.update(&[0x80]);
                derive_ek.update(&n);
                let (ek, n2): (GenericArray<u8, U32>, GenericArray<u8, U24>) =
                    derive_ek.finalize().into_bytes().split();

                // step 4: Derive the authentication key `Ak`
                let mut derive_ak = Blake2bMac::<U32>::new_from_slice(wk.as_ref()).unwrap();
                derive_ak.update(&[0x81]);
                derive_ak.update(&n);
                let ak = derive_ak.finalize().into_bytes();

                // step 5: Encrypt the plaintext key `ptk` with `Ek` and `n2` to obtain the wrapped key `c`
                let mut c = [0; 32];
                let mut chacha = XChaCha20::new(&ek, &n2);
                chacha.apply_keystream_inout(InOutBuf::new(ptk.as_ref(), &mut c).unwrap());

                // step 6: Calculate the authentication tag `t`
                let mut derive_tag = Blake2bMac::<U32>::new_from_slice(&ak).unwrap();
                derive_tag.update(h.as_bytes());
                derive_tag.update(&n);
                derive_tag.update(&c);
                let t = derive_tag.finalize().into_bytes();

                // step 7: Return base64url(t || n || c)
                let mut enc =
                    EncoderStringWriter::from_consumer(h.to_owned(), &general_purpose::URL_SAFE);
                enc.write_all(&t).unwrap();
                enc.write_all(&n).unwrap();
                enc.write_all(&c).unwrap();
                enc.into_inner()
            }

            /// Implementation of <https://github.com/paseto-standard/paserk/blob/master/operations/Wrap/pie.md#v2v4-decryption>
            pub fn local_unwrap_v4(
                wpk: &str,
                wk: &PasetoSymmetricKey<V4, Local>,
            ) -> Result<PasetoSymmetricKey<V4, Local>> {
                let h = "k4.local-wrap.pie.";

                let wpk = wpk
                    .strip_prefix(h)
                    .context("wrapped key is not a local-wrap pie v4 paserk")?;

                // step 1: Decode `b` from Base64url
                let b = general_purpose::URL_SAFE
                    .decode(wpk)
                    .context("wrapped key was not base64url encoded")?;
                ensure!(
                    b.len() > 64,
                    "wrapped key does not contain all information required"
                );
                let (t, b) = b.split_at(32);
                let (n, c) = b.split_at(32);

                // step 2: Derive the authentication key `Ak`
                let mut derive_ak = Blake2bMac::<U32>::new_from_slice(wk.as_ref()).unwrap();
                derive_ak.update(&[0x81]);
                derive_ak.update(n);
                let ak = derive_ak.finalize().into_bytes();

                // step 3: Recalculate the authentication tag t2
                let mut derive_tag = Blake2bMac::<U32>::new_from_slice(&ak).unwrap();
                derive_tag.update(h.as_bytes());
                derive_tag.update(n);
                derive_tag.update(c);
                let t2 = derive_tag.finalize().into_bytes();

                // step 4: Compare t with t2 in constant-time. If it doesn't match, abort.
                ensure!(bool::from(t.ct_eq(&t2)), "invalid message tag");

                // step 5: Derive the encryption key `Ek` and XChaCha nonce `n2`
                let mut derive_ek = Blake2bMac::<U56>::new_from_slice(wk.as_ref()).unwrap();
                derive_ek.update(&[0x80]);
                derive_ek.update(n);
                let (ek, n2): (GenericArray<u8, U32>, GenericArray<u8, U24>) =
                    derive_ek.finalize().into_bytes().split();

                // step 6: Decrypt the wrapped key `c` with `Ek` and `n2` to obtain the plaintext key `ptk`
                let mut ptk = [0; 32];
                let mut chacha = XChaCha20::new(&ek, &n2);
                chacha.apply_keystream_inout(InOutBuf::new(c, &mut ptk).unwrap());

                // step 7: Enforce Algorithm Lucidity
                // asserted by the function type signature.

                // step 8: return ptk
                Ok(PasetoSymmetricKey::from(Key::from(ptk)))
            }

            #[test]
            fn round_trip() {
                let ptk = PasetoSymmetricKey::from(Key::try_new_random().unwrap());
                let wk = PasetoSymmetricKey::from(Key::try_new_random().unwrap());

                let token = local_wrap_v4(&ptk, &wk);
                let ptk2 = local_unwrap_v4(&token, &wk).unwrap();

                assert_eq!(ptk.as_ref(), ptk2.as_ref());

                let fake_wk = PasetoSymmetricKey::from(Key::try_new_random().unwrap());
                let _ = local_unwrap_v4(&token, &fake_wk).map(|_| ()).unwrap_err();
            }
        }
    }
}
