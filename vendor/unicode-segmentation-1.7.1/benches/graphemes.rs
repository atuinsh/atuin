#[macro_use]
extern crate bencher;
extern crate unicode_segmentation;

use bencher::Bencher;
use unicode_segmentation::UnicodeSegmentation;
use std::fs;

fn graphemes(bench: &mut Bencher, path: &str) {
    let text = fs::read_to_string(path).unwrap();
    bench.iter(|| {
        for g in UnicodeSegmentation::graphemes(&*text, true) {
            bencher::black_box(g);
        }
    });

    bench.bytes = text.len() as u64;
}

fn graphemes_arabic(bench: &mut Bencher) {
    graphemes(bench, "benches/texts/arabic.txt");
}

fn graphemes_english(bench: &mut Bencher) {
    graphemes(bench, "benches/texts/english.txt");
}

fn graphemes_hindi(bench: &mut Bencher) {
    graphemes(bench, "benches/texts/hindi.txt");
}

fn graphemes_japanese(bench: &mut Bencher) {
    graphemes(bench, "benches/texts/japanese.txt");
}

fn graphemes_korean(bench: &mut Bencher) {
    graphemes(bench, "benches/texts/korean.txt");
}

fn graphemes_mandarin(bench: &mut Bencher) {
    graphemes(bench, "benches/texts/mandarin.txt");
}

fn graphemes_russian(bench: &mut Bencher) {
    graphemes(bench, "benches/texts/russian.txt");
}

fn graphemes_source_code(bench: &mut Bencher) {
    graphemes(bench, "benches/texts/source_code.txt");
}

benchmark_group!(
    benches,
    graphemes_arabic,
    graphemes_english,
    graphemes_hindi,
    graphemes_japanese,
    graphemes_korean,
    graphemes_mandarin,
    graphemes_russian,
    graphemes_source_code,
);

benchmark_main!(benches);
