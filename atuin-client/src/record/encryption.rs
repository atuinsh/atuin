use atuin_common::record::{DecryptedData, EncryptedData, Encryption};
use base64::{engine::general_purpose, Engine};
use eyre::{ensure, Context, ContextCompat, Result};
use rusty_paserk::{
    id::EncodeId,
    wrap::{LocalWrapperExt, Pie},
};
use rusty_paseto::core::{
    Footer, Key, Local, Paseto, PasetoNonce, PasetoSymmetricKey, Payload, V4,
};
use serde::{Deserialize, Serialize};

/// Encryption using PASETO V4 with a random key
#[allow(non_camel_case_types)]
pub struct PASETO_V4_PIE;

impl Encryption for PASETO_V4_PIE {
    fn encrypt(data: DecryptedData, key: &[u8; 32]) -> EncryptedData {
        let wrapping_key = PasetoSymmetricKey::from(Key::from(key));

        let random_key =
            PasetoSymmetricKey::from(Key::try_new_random().expect("could not source from random"));

        let key_nonce = Key::<32>::try_new_random().expect("could not source from random");
        // wrap the random key so we can decrypt it later
        let footer = AtuinFooter {
            wpk: Pie::wrap_local(&random_key, &wrapping_key, &key_nonce),
            kid: wrapping_key.encode_id(),
        };

        // build the payload
        let payload = general_purpose::URL_SAFE.encode(data.0);
        let footer = serde_json::to_string(&footer).expect("could not serialize footer");
        let nonce = Key::<32>::try_new_random().expect("could not source from random");
        let nonce = PasetoNonce::<V4, Local>::from(&nonce);

        let token = Paseto::<V4, Local>::builder()
            .set_payload(Payload::from(payload.as_str()))
            .set_footer(Footer::from(footer.as_str()))
            .try_encrypt(&random_key, &nonce)
            .expect("error encrypting atuin data");

        EncryptedData(token.into_bytes())
    }

    fn decrypt(data: EncryptedData, key: &[u8; 32]) -> Result<DecryptedData> {
        let wrapping_key = PasetoSymmetricKey::from(Key::from(key));

        // parse the footer from the token.
        // I would have preferred if the paseto library would partially parse
        // for you and give you the footer temporarily, but unfortunately
        // if does not.
        let token = String::from_utf8(data.0).context("token is not utf8")?;
        let (_, footer) = token.rsplit_once('.').context("token has no footer")?;
        let footer = general_purpose::URL_SAFE_NO_PAD
            .decode(footer)
            .context("footer was not valid base64url")?;
        let footer = std::str::from_utf8(&footer).context("footer was not valid utf8")?;
        let AtuinFooter { kid, wpk } =
            serde_json::from_str(footer).context("footer did not contain the correct contents")?;

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

        // decrypt the payload
        let payload =
            Paseto::<V4, Local>::try_decrypt(&token, &random_key, Footer::from(footer), None)
                .context("could not decrypt entry")?;

        let data = general_purpose::URL_SAFE.decode(payload)?;
        Ok(DecryptedData(data))
    }
}

#[derive(Serialize, Deserialize)]
/// Well-known footer claims for decrypting
/// <https://github.com/paseto-standard/paseto-spec/blob/master/docs/02-Implementation-Guide/04-Claims.md#optional-footer-claims>
struct AtuinFooter {
    /// Wrapped key
    wpk: String,
    /// ID of the key which was used to wrap
    kid: String,
}

#[test]
fn round_trip() {
    let key = Key::try_new_random().unwrap();

    let data = DecryptedData(vec![1, 2, 3, 4]);

    let encrypted = PASETO_V4_PIE::encrypt(data.clone(), &key);
    let encrypted2 = PASETO_V4_PIE::encrypt(data.clone(), &key);

    assert_ne!(
        encrypted, encrypted2,
        "re-encrypting the same contents should have different output due to key randomization"
    );

    let decrypted = PASETO_V4_PIE::decrypt(encrypted, &key).unwrap();
    assert_eq!(decrypted, data);

    let fake_key = Key::try_new_random().unwrap();
    let _ = PASETO_V4_PIE::decrypt(encrypted2, &fake_key).unwrap_err();
}
