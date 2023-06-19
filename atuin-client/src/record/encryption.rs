use atuin_common::record::{AdditonalData, DecryptedData, EncryptedData, Encryption};
use base64::{engine::general_purpose, Engine};
use eyre::{ensure, Context, ContextCompat, Result};
use rusty_paserk::{
    id::EncodeId,
    wrap::{LocalWrapperExt, Pie},
};
use rusty_paseto::core::{
    Footer, ImplicitAssertion, Key, Local, Paseto, PasetoNonce, PasetoSymmetricKey, Payload, V4,
};
use serde::{Deserialize, Serialize};

/// Use PASETO V4 Local encryption.
/// Using a randomized key which is wrapped using PIE local-wrap and stored in the footer.
/// Using the additional data as an implicit assertion.
#[allow(non_camel_case_types)]
pub struct PASETO_V4_PIE;

impl Encryption for PASETO_V4_PIE {
    fn encrypt(data: DecryptedData, ad: AdditonalData, key: &[u8; 32]) -> EncryptedData {
        let wrapping_key = PasetoSymmetricKey::from(Key::from(key));

        // generate a random key for this entry
        let random_key =
            PasetoSymmetricKey::from(Key::try_new_random().expect("could not source from random"));

        // wrap the random key so we can decrypt it later
        let key_nonce = Key::<32>::try_new_random().expect("could not source from random");
        let footer = AtuinFooter {
            wpk: Pie::wrap_local(&random_key, &wrapping_key, &key_nonce),
            kid: wrapping_key.encode_id(),
        };
        let footer = serde_json::to_string(&footer).expect("could not serialize footer");

        // encode the implicit assertions
        let assertions = Assertions::from(ad).encode();

        // build the payload and encrypt the token
        let payload = general_purpose::URL_SAFE_NO_PAD.encode(data.0);
        let nonce = Key::<32>::try_new_random().expect("could not source from random");
        let nonce = PasetoNonce::<V4, Local>::from(&nonce);

        let token = Paseto::<V4, Local>::builder()
            .set_payload(Payload::from(payload.as_str()))
            .set_footer(Footer::from(footer.as_str()))
            .set_implicit_assertion(ImplicitAssertion::from(assertions.as_str()))
            .try_encrypt(&random_key, &nonce)
            .expect("error encrypting atuin data");

        EncryptedData(token.into_bytes())
    }

    fn decrypt(data: EncryptedData, ad: AdditonalData, key: &[u8; 32]) -> Result<DecryptedData> {
        let wrapping_key = PasetoSymmetricKey::from(Key::from(key));

        let token = String::from_utf8(data.0).context("token is not utf8")?;

        // get the key info from the footer
        let footer = Self::get_footer(&token)?;
        let AtuinFooter { kid, wpk } =
            serde_json::from_str(&footer).context("footer did not contain the correct contents")?;

        // check that the wrapping key matches the required key to decrypt.
        // In future, we could support multiple keys and use this key to
        // look up the key rather than only allow one key.
        // For now though we will only support the one key and key rotation will
        // have to be a hard reset
        let current_kid = wrapping_key.encode_id();
        ensure!(
            current_kid == kid,
            "attempting to decrypt with incorrect key. currently using {current_kid}, expecting {kid}"
        );

        // decrypt the random key
        let mut wrapped_key = wpk.into_bytes();
        let random_key = Pie::unwrap_local(&mut wrapped_key, &wrapping_key)?;

        // encode the implicit assertions
        let assertions = Assertions::from(ad).encode();

        // decrypt the payload with the footer and implicit assertions
        let payload = Paseto::<V4, Local>::try_decrypt(
            &token,
            &random_key,
            Footer::from(&*footer),
            ImplicitAssertion::from(&*assertions),
        )
        .context("could not decrypt entry")?;

        let data = general_purpose::URL_SAFE_NO_PAD.decode(payload)?;
        Ok(DecryptedData(data))
    }
}

impl PASETO_V4_PIE {
    /// parse the footer from the token.
    fn get_footer(token: &str) -> Result<String> {
        // I would have preferred if the paseto library would partially parse
        // for you and give you the footer temporarily, but unfortunately
        // if does not.
        let (_, footer) = token.rsplit_once('.').context("token has no footer")?;
        let footer = general_purpose::URL_SAFE_NO_PAD
            .decode(footer)
            .context("footer was not valid base64url")?;
        String::from_utf8(footer).context("footer was not valid utf8")
    }
}

#[derive(Serialize, Deserialize)]
/// Well-known footer claims for decrypting. This is not encrypted but is stored in the data blob.
/// <https://github.com/paseto-standard/paseto-spec/blob/master/docs/02-Implementation-Guide/04-Claims.md#optional-footer-claims>
struct AtuinFooter {
    /// Wrapped key
    wpk: String,
    /// ID of the key which was used to wrap
    kid: String,
}

/// Used in the implicit assertions. This is not encrypted and not stored in the data blob.
// This cannot be changed, otherwise it breaks the authenticated encryption.
#[derive(Debug, Copy, Clone, Serialize)]
struct Assertions<'a> {
    id: &'a str,
    version: &'a str,
    tag: &'a str,
}

impl<'a> From<AdditonalData<'a>> for Assertions<'a> {
    fn from(ad: AdditonalData<'a>) -> Self {
        Self {
            id: ad.id,
            version: ad.version,
            tag: ad.tag,
        }
    }
}

impl Assertions<'_> {
    fn encode(&self) -> String {
        serde_json::to_string(self).expect("could not serialize implicit assertions")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = Key::try_new_random().unwrap();

        let ad = AdditonalData {
            id: "foo",
            version: "v0",
            tag: "kv",
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4_PIE::encrypt(data.clone(), ad, &key);
        let decrypted = PASETO_V4_PIE::decrypt(encrypted, ad, &key).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn same_entry_different_output() {
        let key = Key::try_new_random().unwrap();

        let ad = AdditonalData {
            id: "foo",
            version: "v0",
            tag: "kv",
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4_PIE::encrypt(data.clone(), ad, &key);
        let encrypted2 = PASETO_V4_PIE::encrypt(data, ad, &key);

        assert_ne!(
            encrypted, encrypted2,
            "re-encrypting the same contents should have different output due to key randomization"
        );
    }

    #[test]
    fn cannot_decrypt_different_key() {
        let key = Key::try_new_random().unwrap();
        let fake_key = Key::try_new_random().unwrap();

        let ad = AdditonalData {
            id: "foo",
            version: "v0",
            tag: "kv",
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4_PIE::encrypt(data, ad, &key);
        let _ = PASETO_V4_PIE::decrypt(encrypted, ad, &fake_key).unwrap_err();
    }

    #[test]
    fn cannot_decrypt_different_id() {
        let key = Key::try_new_random().unwrap();

        let ad = AdditonalData {
            id: "foo",
            version: "v0",
            tag: "kv",
        };

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = PASETO_V4_PIE::encrypt(data, ad, &key);

        let ad = AdditonalData {
            id: "foo1",
            version: "v0",
            tag: "kv",
        };
        let _ = PASETO_V4_PIE::decrypt(encrypted, ad, &key).unwrap_err();
    }
}
