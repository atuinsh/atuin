use std::io::Write;

use atuin_common::record::{AdditonalData, DecryptedData, EncryptedData, Encryption};
use base64::{engine::general_purpose, write::EncoderStringWriter, Engine};
use eyre::{ensure, Context, ContextCompat, Result};

/// Store record data unencrypted. Only for very specific use cases of record data not being sensitive.
/// If in doubt, use [`super::paseto_v4::PASETO_V4`].
pub struct UnsafeNoEncryption;

static CONTENT_HEADER: &str = "v0.none.";
static CEK_HEADER: &str = "k0.none.";

impl Encryption for UnsafeNoEncryption {
    fn re_encrypt(
        data: EncryptedData,
        _ad: AdditonalData,
        _old_key: &[u8; 32],
        _new_key: &[u8; 32],
    ) -> Result<EncryptedData> {
        Ok(data)
    }

    fn encrypt(data: DecryptedData, _ad: AdditonalData, _key: &[u8; 32]) -> EncryptedData {
        let mut token = EncoderStringWriter::from_consumer(
            CONTENT_HEADER.to_owned(),
            &general_purpose::URL_SAFE_NO_PAD,
        );
        token
            .write_all(&data.0)
            .expect("base64 encoding should always succeed");
        EncryptedData {
            data: token.into_inner(),
            content_encryption_key: CEK_HEADER.to_owned(),
        }
    }

    fn decrypt(data: EncryptedData, _ad: AdditonalData, _key: &[u8; 32]) -> Result<DecryptedData> {
        ensure!(
            data.content_encryption_key == CEK_HEADER,
            "exected unencrypted data, found a content encryption key"
        );
        let content = data
            .data
            .strip_prefix(CONTENT_HEADER)
            .context("exected unencrypted data, found an encrypted token")?;
        let data = general_purpose::URL_SAFE_NO_PAD
            .decode(content)
            .context("could not decode data")?;
        Ok(DecryptedData(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AD: AdditonalData<'static> = AdditonalData {
        id: "foo",
        version: "v0",
        tag: "kv",
        host: "1234",
    };

    #[test]
    fn round_trip() {
        let key = [0x55; 32];

        let data = DecryptedData(vec![1, 2, 3, 4]);

        let encrypted = UnsafeNoEncryption::encrypt(data.clone(), AD, &key);
        let decrypted = UnsafeNoEncryption::decrypt(encrypted, AD, &key).unwrap();
        assert_eq!(decrypted, data);
    }
}
