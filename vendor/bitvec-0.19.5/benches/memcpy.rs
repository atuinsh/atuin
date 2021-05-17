/*! Benchmarks for `BitSlice::copy_from_slice`.

The `copy_from_slice` implementation attempts to detect slice conditions that
allow element-wise `memcpy` behavior, rather than the conservative bit-by-bit
iteration, in the hopes that element load/stores are faster than reading and
writing each bit in an element individually.

At least on the author’s machine, this appears not to be the case. The author
has not inspected the object code emitted by `clone_from_bitslice` and has no
speculation on why this is the case.
!*/

use bitvec::prelude::*;

use criterion::{
	criterion_group,
	criterion_main,
	BenchmarkId,
	Criterion,
	Throughput,
};

const FACTOR: usize = 1024;

pub fn benchmarks(crit: &mut Criterion) {
	let mut group = crit.benchmark_group("accel");
	for (kibi, bits) in [1, 2, 4, 8, 16, 32, 64, 128]
		.iter()
		.copied()
		.map(|n| (n, n * FACTOR))
	{
		group.throughput(Throughput::Elements(bits as u64));
		group.bench_with_input(
			BenchmarkId::from_parameter(kibi),
			&bits,
			|b, bits| {
				let mut dst: BitVec = BitVec::repeat(false, *bits);
				let src: BitVec = BitVec::repeat(true, *bits);
				b.iter(|| {
					dst[10 .. *bits - 10]
						.copy_from_bitslice(&src[10 .. *bits - 10])
				});
			},
		);
	}
	group.finish();

	let mut group = crit.benchmark_group("bitwise");
	for (kibi, bits) in [1, 2, 4, 8, 16, 32, 64, 128]
		.iter()
		.copied()
		.map(|n| (n, n * FACTOR))
	{
		group.throughput(Throughput::Elements(bits as u64));
		group.bench_with_input(
			BenchmarkId::from_parameter(kibi),
			&bits,
			|b, bits| {
				let mut dst: BitVec = BitVec::repeat(false, *bits);
				let src: BitVec = BitVec::repeat(true, *bits);
				b.iter(|| dst.clone_from_bitslice(&src));
			},
		);
	}
	group.finish();

	let mut group = crit.benchmark_group("mismatch");
	for (kibi, bits) in [1, 2, 4, 8, 16, 32, 64, 128]
		.iter()
		.copied()
		.map(|n| (n, n * FACTOR))
	{
		group.throughput(Throughput::Elements(bits as u64));
		group.bench_with_input(
			BenchmarkId::from_parameter(kibi),
			&bits,
			|b, bits| {
				let mut dst: BitVec<Msb0, u16> = BitVec::repeat(false, *bits);
				let src: BitVec<Lsb0, u32> = BitVec::repeat(true, *bits);
				b.iter(|| dst.clone_from_bitslice(&src));
			},
		);
	}
	group.finish();
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
