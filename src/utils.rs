use crypto::digest::Digest;
use crypto::sha2::Sha256;
use sodiumoxide::crypto::pwhash::argon2id13;

pub fn hash_secret(secret: &str) -> String {
    sodiumoxide::init().unwrap();
    let hash = argon2id13::pwhash(
        secret.as_bytes(),
        argon2id13::OPSLIMIT_INTERACTIVE,
        argon2id13::MEMLIMIT_INTERACTIVE,
    )
    .unwrap();
    let texthash = std::str::from_utf8(&hash.0).unwrap().to_string();

    // postgres hates null chars. don't do that to postgres
    texthash.trim_end_matches('\u{0}').to_string()
}

pub fn hash_str(string: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.input_str(string);

    hasher.result_str()
}
