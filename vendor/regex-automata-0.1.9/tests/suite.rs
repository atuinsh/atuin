use regex_automata::{DenseDFA, Regex, RegexBuilder, SparseDFA};

use collection::{RegexTester, SUITE};

#[test]
fn unminimized_standard() {
    let mut builder = RegexBuilder::new();
    builder.minimize(false).premultiply(false).byte_classes(false);

    let mut tester = RegexTester::new().skip_expensive();
    tester.test_all(builder, SUITE.tests());
    tester.assert();
}

#[test]
fn unminimized_premultiply() {
    let mut builder = RegexBuilder::new();
    builder.minimize(false).premultiply(true).byte_classes(false);

    let mut tester = RegexTester::new().skip_expensive();
    tester.test_all(builder, SUITE.tests());
    tester.assert();
}

#[test]
fn unminimized_byte_class() {
    let mut builder = RegexBuilder::new();
    builder.minimize(false).premultiply(false).byte_classes(true);

    let mut tester = RegexTester::new();
    tester.test_all(builder, SUITE.tests());
    tester.assert();
}

#[test]
fn unminimized_premultiply_byte_class() {
    let mut builder = RegexBuilder::new();
    builder.minimize(false).premultiply(true).byte_classes(true);

    let mut tester = RegexTester::new();
    tester.test_all(builder, SUITE.tests());
    tester.assert();
}

#[test]
fn unminimized_standard_no_nfa_shrink() {
    let mut builder = RegexBuilder::new();
    builder
        .minimize(false)
        .premultiply(false)
        .byte_classes(false)
        .shrink(false);

    let mut tester = RegexTester::new().skip_expensive();
    tester.test_all(builder, SUITE.tests());
    tester.assert();
}

#[test]
fn minimized_standard() {
    let mut builder = RegexBuilder::new();
    builder.minimize(true).premultiply(false).byte_classes(false);

    let mut tester = RegexTester::new().skip_expensive();
    tester.test_all(builder, SUITE.tests());
    tester.assert();
}

#[test]
fn minimized_premultiply() {
    let mut builder = RegexBuilder::new();
    builder.minimize(true).premultiply(true).byte_classes(false);

    let mut tester = RegexTester::new().skip_expensive();
    tester.test_all(builder, SUITE.tests());
    tester.assert();
}

#[test]
fn minimized_byte_class() {
    let mut builder = RegexBuilder::new();
    builder.minimize(true).premultiply(false).byte_classes(true);

    let mut tester = RegexTester::new();
    tester.test_all(builder, SUITE.tests());
    tester.assert();
}

#[test]
fn minimized_premultiply_byte_class() {
    let mut builder = RegexBuilder::new();
    builder.minimize(true).premultiply(true).byte_classes(true);

    let mut tester = RegexTester::new();
    tester.test_all(builder, SUITE.tests());
    tester.assert();
}

#[test]
fn minimized_standard_no_nfa_shrink() {
    let mut builder = RegexBuilder::new();
    builder
        .minimize(true)
        .premultiply(false)
        .byte_classes(false)
        .shrink(false);

    let mut tester = RegexTester::new().skip_expensive();
    tester.test_all(builder, SUITE.tests());
    tester.assert();
}

// A basic sanity test that checks we can convert a regex to a smaller
// representation and that the resulting regex still passes our tests.
//
// If tests grow minimal regexes that cannot be represented in 16 bits, then
// we'll either want to skip those or increase the size to test to u32.
#[test]
fn u16() {
    let mut builder = RegexBuilder::new();
    builder.minimize(true).premultiply(false).byte_classes(true);

    let mut tester = RegexTester::new().skip_expensive();
    for test in SUITE.tests() {
        let builder = builder.clone();
        let re: Regex = match tester.build_regex(builder, test) {
            None => continue,
            Some(re) => re,
        };
        let small_re = Regex::from_dfas(
            re.forward().to_u16().unwrap(),
            re.reverse().to_u16().unwrap(),
        );

        tester.test(test, &small_re);
    }
    tester.assert();
}

// Test that sparse DFAs work using the standard configuration.
#[test]
fn sparse_unminimized_standard() {
    let mut builder = RegexBuilder::new();
    builder.minimize(false).premultiply(false).byte_classes(false);

    let mut tester = RegexTester::new().skip_expensive();
    for test in SUITE.tests() {
        let builder = builder.clone();
        let re: Regex = match tester.build_regex(builder, test) {
            None => continue,
            Some(re) => re,
        };
        let fwd = re.forward().to_sparse().unwrap();
        let rev = re.reverse().to_sparse().unwrap();
        let sparse_re = Regex::from_dfas(fwd, rev);

        tester.test(test, &sparse_re);
    }
    tester.assert();
}

// Test that sparse DFAs work after converting them to a different state ID
// representation.
#[test]
fn sparse_u16() {
    let mut builder = RegexBuilder::new();
    builder.minimize(true).premultiply(false).byte_classes(false);

    let mut tester = RegexTester::new().skip_expensive();
    for test in SUITE.tests() {
        let builder = builder.clone();
        let re: Regex = match tester.build_regex(builder, test) {
            None => continue,
            Some(re) => re,
        };
        let fwd = re.forward().to_sparse().unwrap().to_u16().unwrap();
        let rev = re.reverse().to_sparse().unwrap().to_u16().unwrap();
        let sparse_re = Regex::from_dfas(fwd, rev);

        tester.test(test, &sparse_re);
    }
    tester.assert();
}

// Another basic sanity test that checks we can serialize and then deserialize
// a regex, and that the resulting regex can be used for searching correctly.
#[test]
fn serialization_roundtrip() {
    let mut builder = RegexBuilder::new();
    builder.premultiply(false).byte_classes(true);

    let mut tester = RegexTester::new().skip_expensive();
    for test in SUITE.tests() {
        let builder = builder.clone();
        let re: Regex = match tester.build_regex(builder, test) {
            None => continue,
            Some(re) => re,
        };

        let fwd_bytes = re.forward().to_bytes_native_endian().unwrap();
        let rev_bytes = re.reverse().to_bytes_native_endian().unwrap();
        let fwd: DenseDFA<&[usize], usize> =
            unsafe { DenseDFA::from_bytes(&fwd_bytes) };
        let rev: DenseDFA<&[usize], usize> =
            unsafe { DenseDFA::from_bytes(&rev_bytes) };
        let re = Regex::from_dfas(fwd, rev);

        tester.test(test, &re);
    }
    tester.assert();
}

// A basic sanity test that checks we can serialize and then deserialize a
// regex using sparse DFAs, and that the resulting regex can be used for
// searching correctly.
#[test]
fn sparse_serialization_roundtrip() {
    let mut builder = RegexBuilder::new();
    builder.byte_classes(true);

    let mut tester = RegexTester::new().skip_expensive();
    for test in SUITE.tests() {
        let builder = builder.clone();
        let re: Regex = match tester.build_regex(builder, test) {
            None => continue,
            Some(re) => re,
        };

        let fwd_bytes = re
            .forward()
            .to_sparse()
            .unwrap()
            .to_bytes_native_endian()
            .unwrap();
        let rev_bytes = re
            .reverse()
            .to_sparse()
            .unwrap()
            .to_bytes_native_endian()
            .unwrap();
        let fwd: SparseDFA<&[u8], usize> =
            unsafe { SparseDFA::from_bytes(&fwd_bytes) };
        let rev: SparseDFA<&[u8], usize> =
            unsafe { SparseDFA::from_bytes(&rev_bytes) };
        let re = Regex::from_dfas(fwd, rev);

        tester.test(test, &re);
    }
    tester.assert();
}
