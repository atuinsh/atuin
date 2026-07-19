#![cfg(test)]
#![allow(dead_code)]

use proptest::prelude::*;

/// Alias bodies are arbitrary bytes except NUL, which no shell can carry and which our framing
/// markers rely on being absent.
pub fn value_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(1u8..=255, 0..64)
}

/// Bodies biased toward the characters that break quoting, so random search spends its budget
/// where the bugs are instead of on random high bytes.
pub fn spicy_value_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        prop_oneof![
            4 => prop::sample::select(vec![
                b'\'', b'"', b'\\', b'\n', b'\t', b' ', b'=', b'$', b'`', b'#', b';', b'|', b'&',
                0x01, 0x1b, 0x7f,
            ]),
            1 => 1u8..=255,
        ],
        0..48,
    )
}

/// Deliberately conservative: every shell accepts this shape. Exotic names are covered by the
/// hand-written corpus in Task 2, not by random search, because the *valid* name charset differs
/// per shell and a rejected name makes the shell error rather than fail our assertion.
pub fn alias_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,15}"
}

/// A batch of distinct names paired with bodies.
pub fn alias_batch(n: usize) -> impl Strategy<Value = Vec<(String, Vec<u8>)>> {
    prop::collection::hash_map(alias_name(), spicy_value_bytes(), 1..=n)
        .prop_map(|m| m.into_iter().collect())
}
