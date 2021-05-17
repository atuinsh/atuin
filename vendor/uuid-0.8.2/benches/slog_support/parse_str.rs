extern crate test;

#[bench]
#[cfg(feature = "slog")]
pub fn bench_log_discard_kv(b: &mut test::Bencher) {
    let u1 =
        uuid::Uuid::parse_str("F9168C5E-CEB2-4FAB-B6BF-329BF39FA1E4").unwrap();
    let root =
        slog::Logger::root(::slog::Drain::fuse(::slog::Discard), slog::o!());

    b.iter(|| {
        #[cfg(feature = "slog")]
        slog::crit!(root, "test"; "u1" => u1);
    });
}
