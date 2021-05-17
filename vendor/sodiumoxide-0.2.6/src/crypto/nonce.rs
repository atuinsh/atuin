use randombytes::randombytes_into;

/// The `Nonce` trait allows for generic construction
/// from an array of bytes, e.g. to generically create random nonces.
pub trait Nonce {
    type Bytes: Default + AsMut<[u8]>;
    fn from_bytes(Self::Bytes) -> Self;
}

/// `gen_random_nonce()` randomly generates a nonce for symmetric encryption
///
/// THREAD SAFETY: `gen_random_nonce()` is thread-safe provided that you have
/// called `sodiumoxide::init()` once before using any other function
/// from sodiumoxide.
///
/// NOTE: When using primitives with short nonces (e.g. salsa20, salsa208, salsa2012)
/// do not use random nonces since the probability of nonce-collision is not negligible
pub fn gen_random_nonce<N: Nonce>() -> N {
    let mut n = N::Bytes::default();
    randombytes_into(n.as_mut());
    N::from_bytes(n)
}
