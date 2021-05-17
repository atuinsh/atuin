use std::collections::BTreeMap;
use std::env;
use std::fmt::{self, Write};
use std::thread;

use regex;
use regex_automata::{DenseDFA, ErrorKind, Regex, RegexBuilder, StateID, DFA};
use serde_bytes;
use toml;

macro_rules! load {
    ($col:ident, $path:expr) => {
        $col.extend(RegexTests::load(
            concat!("../data/tests/", $path),
            include_bytes!(concat!("../data/tests/", $path)),
        ));
    };
}

lazy_static! {
    pub static ref SUITE: RegexTestCollection = {
        let mut col = RegexTestCollection::new();
        load!(col, "fowler/basic.toml");
        load!(col, "fowler/nullsubexpr.toml");
        load!(col, "fowler/repetition.toml");
        load!(col, "fowler/repetition-long.toml");
        load!(col, "crazy.toml");
        load!(col, "flags.toml");
        load!(col, "iter.toml");
        load!(col, "no-unicode.toml");
        load!(col, "unicode.toml");
        col
    };
}

#[derive(Clone, Debug)]
pub struct RegexTestCollection {
    pub by_name: BTreeMap<String, RegexTest>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RegexTests {
    pub tests: Vec<RegexTest>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RegexTest {
    pub name: String,
    #[serde(default)]
    pub options: Vec<RegexTestOption>,
    pub pattern: String,
    #[serde(with = "serde_bytes")]
    pub input: Vec<u8>,
    #[serde(rename = "matches")]
    pub matches: Vec<Match>,
    #[serde(default)]
    pub captures: Vec<Option<Match>>,
    #[serde(default)]
    pub fowler_line_number: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum RegexTestOption {
    Anchored,
    CaseInsensitive,
    NoUnicode,
    Escaped,
    #[serde(rename = "invalid-utf8")]
    InvalidUTF8,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
}

impl RegexTestCollection {
    fn new() -> RegexTestCollection {
        RegexTestCollection { by_name: BTreeMap::new() }
    }

    fn extend(&mut self, tests: RegexTests) {
        for test in tests.tests {
            let name = test.name.clone();
            if self.by_name.contains_key(&name) {
                panic!("found duplicate test {}", name);
            }
            self.by_name.insert(name, test);
        }
    }

    pub fn tests(&self) -> Vec<&RegexTest> {
        self.by_name.values().collect()
    }
}

impl RegexTests {
    fn load(path: &str, slice: &[u8]) -> RegexTests {
        let mut data: RegexTests = toml::from_slice(slice)
            .expect(&format!("failed to load {}", path));
        for test in &mut data.tests {
            if test.options.contains(&RegexTestOption::Escaped) {
                test.input = unescape_bytes(&test.input);
            }
        }
        data
    }
}

#[derive(Debug)]
pub struct RegexTester {
    asserted: bool,
    results: RegexTestResults,
    skip_expensive: bool,
    whitelist: Vec<regex::Regex>,
    blacklist: Vec<regex::Regex>,
}

impl Drop for RegexTester {
    fn drop(&mut self) {
        // If we haven't asserted yet, then the test is probably buggy, so
        // fail it. But if we're already panicking (e.g., a bug in the regex
        // engine), then don't double-panic, which causes an immediate abort.
        if !thread::panicking() && !self.asserted {
            panic!("must call RegexTester::assert at end of test");
        }
    }
}

impl RegexTester {
    pub fn new() -> RegexTester {
        let mut tester = RegexTester {
            asserted: false,
            results: RegexTestResults::default(),
            skip_expensive: false,
            whitelist: vec![],
            blacklist: vec![],
        };
        for x in env::var("REGEX_TEST").unwrap_or("".to_string()).split(",") {
            let x = x.trim();
            if x.is_empty() {
                continue;
            }
            if x.starts_with("-") {
                tester = tester.blacklist(&x[1..]);
            } else {
                tester = tester.whitelist(x);
            }
        }
        tester
    }

    pub fn skip_expensive(mut self) -> RegexTester {
        self.skip_expensive = true;
        self
    }

    pub fn whitelist(mut self, name: &str) -> RegexTester {
        self.whitelist.push(regex::Regex::new(name).unwrap());
        self
    }

    pub fn blacklist(mut self, name: &str) -> RegexTester {
        self.blacklist.push(regex::Regex::new(name).unwrap());
        self
    }

    pub fn assert(&mut self) {
        self.asserted = true;
        self.results.assert();
    }

    pub fn build_regex<S: StateID>(
        &self,
        mut builder: RegexBuilder,
        test: &RegexTest,
    ) -> Option<Regex<DenseDFA<Vec<S>, S>>> {
        if self.skip(test) {
            return None;
        }
        self.apply_options(test, &mut builder);

        match builder.build_with_size::<S>(&test.pattern) {
            Ok(re) => Some(re),
            Err(err) => {
                if let ErrorKind::Unsupported(_) = *err.kind() {
                    None
                } else {
                    panic!(
                        "failed to build {:?} with pattern '{:?}': {}",
                        test.name, test.pattern, err
                    );
                }
            }
        }
    }

    pub fn test_all<'a, I, T>(&mut self, builder: RegexBuilder, tests: I)
    where
        I: IntoIterator<IntoIter = T, Item = &'a RegexTest>,
        T: Iterator<Item = &'a RegexTest>,
    {
        for test in tests {
            let builder = builder.clone();
            let re: Regex = match self.build_regex(builder, test) {
                None => continue,
                Some(re) => re,
            };
            self.test(test, &re);
        }
    }

    pub fn test<'a, D: DFA>(&mut self, test: &RegexTest, re: &Regex<D>) {
        self.test_is_match(test, re);
        self.test_find(test, re);
        // Some tests (namely, fowler) are designed only to detect the
        // first match even if there are more subsequent matches. To that
        // end, we only test match iteration when the number of matches
        // expected is not 1, or if the test name has 'iter' in it.
        if test.name.contains("iter") || test.matches.len() != 1 {
            self.test_find_iter(test, re);
        }
    }

    pub fn test_is_match<'a, D: DFA>(
        &mut self,
        test: &RegexTest,
        re: &Regex<D>,
    ) {
        self.asserted = false;

        let got = re.is_match(&test.input);
        let expected = test.matches.len() >= 1;
        if got == expected {
            self.results.succeeded.push(test.clone());
            return;
        }
        self.results.failed.push(RegexTestFailure {
            test: test.clone(),
            kind: RegexTestFailureKind::IsMatch,
        });
    }

    pub fn test_find<'a, D: DFA>(&mut self, test: &RegexTest, re: &Regex<D>) {
        self.asserted = false;

        let got =
            re.find(&test.input).map(|(start, end)| Match { start, end });
        if got == test.matches.get(0).map(|&m| m) {
            self.results.succeeded.push(test.clone());
            return;
        }
        self.results.failed.push(RegexTestFailure {
            test: test.clone(),
            kind: RegexTestFailureKind::Find { got },
        });
    }

    pub fn test_find_iter<'a, D: DFA>(
        &mut self,
        test: &RegexTest,
        re: &Regex<D>,
    ) {
        self.asserted = false;

        let got: Vec<Match> = re
            .find_iter(&test.input)
            .map(|(start, end)| Match { start, end })
            .collect();
        if got == test.matches {
            self.results.succeeded.push(test.clone());
            return;
        }
        self.results.failed.push(RegexTestFailure {
            test: test.clone(),
            kind: RegexTestFailureKind::FindIter { got },
        });
    }

    fn skip(&self, test: &RegexTest) -> bool {
        if self.skip_expensive {
            if test.name.starts_with("repetition-long") {
                return true;
            }
        }
        if !self.blacklist.is_empty() {
            if self.blacklist.iter().any(|re| re.is_match(&test.name)) {
                return true;
            }
        }
        if !self.whitelist.is_empty() {
            if !self.whitelist.iter().any(|re| re.is_match(&test.name)) {
                return true;
            }
        }
        false
    }

    fn apply_options(&self, test: &RegexTest, builder: &mut RegexBuilder) {
        for opt in &test.options {
            match *opt {
                RegexTestOption::Anchored => {
                    builder.anchored(true);
                }
                RegexTestOption::CaseInsensitive => {
                    builder.case_insensitive(true);
                }
                RegexTestOption::NoUnicode => {
                    builder.unicode(false);
                }
                RegexTestOption::Escaped => {}
                RegexTestOption::InvalidUTF8 => {
                    builder.allow_invalid_utf8(true);
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RegexTestResults {
    /// Tests that succeeded.
    pub succeeded: Vec<RegexTest>,
    /// Failed tests, indexed by group name.
    pub failed: Vec<RegexTestFailure>,
}

#[derive(Clone, Debug)]
pub struct RegexTestFailure {
    test: RegexTest,
    kind: RegexTestFailureKind,
}

#[derive(Clone, Debug)]
pub enum RegexTestFailureKind {
    IsMatch,
    Find { got: Option<Match> },
    FindIter { got: Vec<Match> },
}

impl RegexTestResults {
    pub fn assert(&self) {
        if self.failed.is_empty() {
            return;
        }
        let failures = self
            .failed
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<String>>()
            .join("\n\n");
        panic!(
            "found {} failures:\n{}\n{}\n{}\n\n\
             Set the REGEX_TEST environment variable to filter tests, \n\
             e.g., REGEX_TEST=crazy-misc,-crazy-misc2 runs every test \n\
             whose name contains crazy-misc but not crazy-misc2\n\n",
            self.failed.len(),
            "~".repeat(79),
            failures.trim(),
            "~".repeat(79)
        )
    }
}

impl fmt::Display for RegexTestFailure {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}: {}\n    \
             options: {:?}\n    \
             pattern: {}\n    \
             pattern (escape): {}\n    \
             input: {}\n    \
             input (escape): {}\n    \
             input (hex): {}",
            self.test.name,
            self.kind.fmt(&self.test)?,
            self.test.options,
            self.test.pattern,
            escape_default(&self.test.pattern),
            nice_raw_bytes(&self.test.input),
            escape_bytes(&self.test.input),
            hex_bytes(&self.test.input)
        )
    }
}

impl RegexTestFailureKind {
    fn fmt(&self, test: &RegexTest) -> Result<String, fmt::Error> {
        let mut buf = String::new();
        match *self {
            RegexTestFailureKind::IsMatch => {
                if let Some(&m) = test.matches.get(0) {
                    write!(buf, "expected match (at {}), but none found", m)?
                } else {
                    write!(buf, "expected no match, but found a match")?
                }
            }
            RegexTestFailureKind::Find { got } => write!(
                buf,
                "expected {:?}, but found {:?}",
                test.matches.get(0),
                got
            )?,
            RegexTestFailureKind::FindIter { ref got } => write!(
                buf,
                "expected {:?}, but found {:?}",
                test.matches, got
            )?,
        }
        Ok(buf)
    }
}

impl fmt::Display for Match {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "({}, {})", self.start, self.end)
    }
}

impl fmt::Debug for Match {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "({}, {})", self.start, self.end)
    }
}

fn nice_raw_bytes(bytes: &[u8]) -> String {
    use std::str;

    match str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => escape_bytes(bytes),
    }
}

fn escape_bytes(bytes: &[u8]) -> String {
    use std::ascii;

    let escaped = bytes
        .iter()
        .flat_map(|&b| ascii::escape_default(b))
        .collect::<Vec<u8>>();
    String::from_utf8(escaped).unwrap()
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| format!(r"\x{:02X}", b)).collect()
}

fn escape_default(s: &str) -> String {
    s.chars().flat_map(|c| c.escape_default()).collect()
}

fn unescape_bytes(bytes: &[u8]) -> Vec<u8> {
    use std::str;
    use unescape::unescape;

    unescape(&str::from_utf8(bytes).expect("all input must be valid UTF-8"))
}
