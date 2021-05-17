use ahash::{AHasher, CallHasher};
use criterion::*;
use fxhash::FxHasher;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[cfg(all(any(target_arch = "x86", target_arch = "x86_64"), target_feature = "aes"))]
fn aeshash<H: Hash>(b: &H) -> u64 {
    let hasher = AHasher::default();
    b.get_hash(hasher)
}
#[cfg(not(all(any(target_arch = "x86", target_arch = "x86_64"), target_feature = "aes")))]
fn aeshash<H: Hash>(_b: &H) -> u64 {
    panic!("aes must be enabled")
}

#[cfg(not(all(any(target_arch = "x86", target_arch = "x86_64"), target_feature = "aes")))]
fn fallbackhash<H: Hash>(b: &H) -> u64 {
    let hasher = AHasher::default();
    b.get_hash(hasher)
}
#[cfg(all(any(target_arch = "x86", target_arch = "x86_64"), target_feature = "aes"))]
fn fallbackhash<H: Hash>(_b: &H) -> u64 {
    panic!("aes must be disabled")
}

fn fnvhash<H: Hash>(b: &H) -> u64 {
    let mut hasher = fnv::FnvHasher::default();
    b.hash(&mut hasher);
    hasher.finish()
}

fn siphash<H: Hash>(b: &H) -> u64 {
    let mut hasher = DefaultHasher::default();
    b.hash(&mut hasher);
    hasher.finish()
}

fn fxhash<H: Hash>(b: &H) -> u64 {
    let mut hasher = FxHasher::default();
    b.hash(&mut hasher);
    hasher.finish()
}

fn seahash<H: Hash>(b: &H) -> u64 {
    let mut hasher = seahash::SeaHasher::default();
    b.hash(&mut hasher);
    hasher.finish()
}

const STRING_LENGTHS: [u32; 12] = [1, 3, 4, 7, 8, 15, 16, 24, 33, 68, 132, 1024];

fn gen_strings() -> Vec<String> {
    STRING_LENGTHS
        .iter()
        .map(|len| {
            let mut string = String::default();
            for pos in 1..=*len {
                let c = (48 + (pos % 10) as u8) as char;
                string.push(c);
            }
            string
        })
        .collect()
}

const U8_VALUES: [u8; 1] = [123];
const U16_VALUES: [u16; 1] = [1234];
const U32_VALUES: [u32; 1] = [12345678];
const U64_VALUES: [u64; 1] = [1234567890123456];
const U128_VALUES: [u128; 1] = [12345678901234567890123456789012];

fn bench_ahash(c: &mut Criterion) {
    c.bench(
        "aeshash",
        ParameterizedBenchmark::new("u8", |b, &s| b.iter(|| black_box(aeshash(s))), &U8_VALUES),
    );
    c.bench(
        "aeshash",
        ParameterizedBenchmark::new("u16", |b, &s| b.iter(|| black_box(aeshash(s))), &U16_VALUES),
    );
    c.bench(
        "aeshash",
        ParameterizedBenchmark::new("u32", |b, &s| b.iter(|| black_box(aeshash(s))), &U32_VALUES),
    );
    c.bench(
        "aeshash",
        ParameterizedBenchmark::new("u64", |b, &s| b.iter(|| black_box(aeshash(s))), &U64_VALUES),
    );
    c.bench(
        "aeshash",
        ParameterizedBenchmark::new("u128", |b, &s| b.iter(|| black_box(aeshash(s))), &U128_VALUES),
    );
    c.bench(
        "aeshash",
        ParameterizedBenchmark::new("string", |b, s| b.iter(|| black_box(aeshash(s))), gen_strings()),
    );
}

fn bench_fallback(c: &mut Criterion) {
    c.bench(
        "fallback",
        ParameterizedBenchmark::new("u8", |b, &s| b.iter(|| black_box(fallbackhash(s))), &U8_VALUES),
    );
    c.bench(
        "fallback",
        ParameterizedBenchmark::new("u16", |b, &s| b.iter(|| black_box(fallbackhash(s))), &U16_VALUES),
    );
    c.bench(
        "fallback",
        ParameterizedBenchmark::new("u32", |b, &s| b.iter(|| black_box(fallbackhash(s))), &U32_VALUES),
    );
    c.bench(
        "fallback",
        ParameterizedBenchmark::new("u64", |b, &s| b.iter(|| black_box(fallbackhash(s))), &U64_VALUES),
    );
    c.bench(
        "fallback",
        ParameterizedBenchmark::new("u128", |b, &s| b.iter(|| black_box(fallbackhash(s))), &U128_VALUES),
    );
    c.bench(
        "fallback",
        ParameterizedBenchmark::new("string", |b, s| b.iter(|| black_box(fallbackhash(s))), gen_strings()),
    );
}

fn bench_fx(c: &mut Criterion) {
    c.bench(
        "fx",
        ParameterizedBenchmark::new("u8", |b, &s| b.iter(|| black_box(fxhash(s))), &U8_VALUES),
    );
    c.bench(
        "fx",
        ParameterizedBenchmark::new("u16", |b, &s| b.iter(|| black_box(fxhash(s))), &U16_VALUES),
    );
    c.bench(
        "fx",
        ParameterizedBenchmark::new("u32", |b, &s| b.iter(|| black_box(fxhash(s))), &U32_VALUES),
    );
    c.bench(
        "fx",
        ParameterizedBenchmark::new("u64", |b, &s| b.iter(|| black_box(fxhash(s))), &U64_VALUES),
    );
    c.bench(
        "fx",
        ParameterizedBenchmark::new("u128", |b, &s| b.iter(|| black_box(fxhash(s))), &U128_VALUES),
    );
    c.bench(
        "fx",
        ParameterizedBenchmark::new("string", |b, s| b.iter(|| black_box(fxhash(s))), gen_strings()),
    );
}

fn bench_fnv(c: &mut Criterion) {
    c.bench(
        "fnv",
        ParameterizedBenchmark::new("u8", |b, &s| b.iter(|| black_box(fnvhash(s))), &U8_VALUES),
    );
    c.bench(
        "fnv",
        ParameterizedBenchmark::new("u16", |b, &s| b.iter(|| black_box(fnvhash(s))), &U16_VALUES),
    );
    c.bench(
        "fnv",
        ParameterizedBenchmark::new("u32", |b, &s| b.iter(|| black_box(fnvhash(s))), &U32_VALUES),
    );
    c.bench(
        "fnv",
        ParameterizedBenchmark::new("u64", |b, &s| b.iter(|| black_box(fnvhash(s))), &U64_VALUES),
    );
    c.bench(
        "fnv",
        ParameterizedBenchmark::new("u128", |b, &s| b.iter(|| black_box(fnvhash(s))), &U128_VALUES),
    );
    c.bench(
        "fnv",
        ParameterizedBenchmark::new("string", |b, s| b.iter(|| black_box(fnvhash(s))), gen_strings()),
    );
}

fn bench_sea(c: &mut Criterion) {
    c.bench(
        "sea",
        ParameterizedBenchmark::new("string", |b, s| b.iter(|| black_box(seahash(s))), gen_strings()),
    );
}

fn bench_sip(c: &mut Criterion) {
    c.bench(
        "sip",
        ParameterizedBenchmark::new("u8", |b, &s| b.iter(|| black_box(siphash(s))), &U8_VALUES),
    );
    c.bench(
        "sip",
        ParameterizedBenchmark::new("u16", |b, &s| b.iter(|| black_box(siphash(s))), &U16_VALUES),
    );
    c.bench(
        "sip",
        ParameterizedBenchmark::new("u32", |b, &s| b.iter(|| black_box(siphash(s))), &U32_VALUES),
    );
    c.bench(
        "sip",
        ParameterizedBenchmark::new("u64", |b, &s| b.iter(|| black_box(siphash(s))), &U64_VALUES),
    );
    c.bench(
        "sip",
        ParameterizedBenchmark::new("u128", |b, &s| b.iter(|| black_box(siphash(s))), &U128_VALUES),
    );
    c.bench(
        "sip",
        ParameterizedBenchmark::new("string", |b, s| b.iter(|| black_box(siphash(s))), gen_strings()),
    );
}

criterion_main!(benches);
criterion_group!(
    benches,
    bench_ahash,
    bench_fallback,
    bench_fx,
    bench_fnv,
    bench_sea,
    bench_sip
);
