//! Removes the parts of captured metadata that commonly carry secrets.

use std::ops::Range;

/// Reduces `url` to its scheme, host, and path.
///
/// Query strings, fragments, and user info often hold tokens. A `data:` URL holds the whole
/// document, so only its media type survives.
#[must_use]
pub fn scrub_url(mut url: String) -> String {
    if url.starts_with("data:") {
        let end = url.find([',', ';']).unwrap_or(url.len());
        url.truncate(end);
        return url;
    }
    if let Some(end) = url.find(['?', '#']) {
        url.truncate(end);
    }
    if let Some(authority) = url.find("://").map(|i| i + 3) {
        let authority_end = url[authority..].find('/').map_or(url.len(), |i| authority + i);
        if let Some(at) = url[authority..authority_end].rfind('@') {
            url.replace_range(authority..=authority + at, "");
        }
    }
    url
}

/// Removes query strings and fragments from the URL-like words of a window title.
///
/// Browsers show the URL as the title while a page loads.
#[must_use]
pub fn scrub_title(title: String) -> String {
    let carries_parameters =
        |word: &str| word.find(['?', '#']).filter(|&start| word[start..].contains('='));
    if !title.split(' ').any(|word| carries_parameters(word).is_some()) {
        return title;
    }
    let mut scrubbed = String::with_capacity(title.len());
    for (i, word) in title.split(' ').enumerate() {
        if i > 0 {
            scrubbed.push(' ');
        }
        scrubbed.push_str(&word[..carries_parameters(word).unwrap_or(word.len())]);
    }
    scrubbed
}

/// Removes the parameters and the credentials from the URLs that `text` mentions.
///
/// A page, a message, or a document can show a link that carries a token, such as a callback
/// URL with `#access_token=…`. A query or fragment without `=`, such as `#install`, stays.
#[must_use]
pub fn scrub_text_urls(text: String) -> String {
    let mut scrubbed: Option<String> = None;
    let mut copied = 0;
    for word in words(&text) {
        for removal in url_removals(&text[word.clone()]).into_iter().flatten() {
            let out = scrubbed.get_or_insert_with(|| String::with_capacity(text.len()));
            out.push_str(&text[copied..word.start + removal.start]);
            copied = word.start + removal.end;
        }
    }
    match scrubbed {
        Some(mut out) => {
            out.push_str(&text[copied..]);
            out
        }
        None => text,
    }
}

/// Whether `c` ends a URL that text mentions: a blank, or a character that encloses a URL in
/// markup or prose.
fn ends_url(c: char) -> bool {
    c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`' | '|')
}

/// The byte ranges of the runs of `text` between the characters that [`ends_url`] accepts.
fn words(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut chars = text.char_indices().peekable();
    std::iter::from_fn(move || {
        while chars.next_if(|&(_, c)| ends_url(c)).is_some() {}
        let &(start, _) = chars.peek()?;
        let mut end = start;
        while let Some((at, c)) = chars.next_if(|&(_, c)| !ends_url(c)) {
            end = at + c.len_utf8();
        }
        Some(start..end)
    })
}

/// The byte ranges of `word` that hold a URL's credentials and its parameters, in that order.
/// Both are `None` when `word` isn't a URL.
fn url_removals(word: &str) -> [Option<Range<usize>>; 2] {
    let Some(host) = url_host_start(word) else {
        return [None, None];
    };
    let host_end = word[host..].find(['/', '?', '#']).map_or(word.len(), |end| host + end);
    let credentials = word[host..host_end].rfind('@').map(|at| host..host + at + 1);
    let parameters = word[host_end..]
        .find(['?', '#'])
        .map(|start| host_end + start)
        .filter(|&start| word[start..].contains('='))
        .map(|start| start..word.len());
    [credentials, parameters]
}

/// Where the host of the URL in `word` starts, when `word` holds a URL.
///
/// A URL either has a scheme, such as `https://`, or starts with a host name such as
/// `example.com` that a path, a query, or a fragment follows directly.
fn url_host_start(word: &str) -> Option<usize> {
    if let Some(scheme_end) = word.find("://") {
        return word[..scheme_end]
            .ends_with(|c: char| c.is_ascii_alphanumeric())
            .then_some(scheme_end + 3);
    }
    let start = word.find(|c: char| c.is_ascii_alphanumeric())?;
    let end = word[start..].find(['/', '?', '#']).map(|end| start + end)?;
    is_host_name(&word[start..end]).then_some(start)
}

/// Whether `host` reads as a host name: labels of letters, digits, and dashes, joined by dots,
/// that end in a top-level domain of letters.
fn is_host_name(host: &str) -> bool {
    let mut labels = host.split('.');
    let Some(top) = labels.next_back() else {
        return false;
    };
    host.contains('.')
        && top.len() >= 2
        && top.bytes().all(|b| b.is_ascii_alphabetic())
        && labels.all(|label| {
            !label.is_empty() && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// The text that replaces a one-time code.
pub const ONE_TIME_CODE: &str = "[one-time code]";

/// The words that mark a nearby number as a one-time code.
const CODE_WORDS: [&str; 10] = [
    "code",
    "otp",
    "passcode",
    "verification",
    "verify",
    "pin",
    "one-time",
    "2fa",
    "login",
    "security",
];

/// The number of characters on each side of a number in which a code word counts.
const CODE_WORD_REACH: usize = 40;

/// Replaces the one-time login codes in `text` with [`ONE_TIME_CODE`].
///
/// A code is a run of 4 to 8 digits that a code word such as "verification" or "login"
/// accompanies. A number that reads as a year or as a reference such as "#4711" needs the code
/// word right before it, or "is your" and a code word after it. Messages, notifications, and
/// mail carry such codes, and they must not leave capture.
#[must_use]
pub fn scrub_one_time_codes(text: String) -> String {
    let mut scrubbed: Option<String> = None;
    let mut copied = 0;
    let mut at = 0;
    while at < text.len() {
        if !text.as_bytes()[at].is_ascii_digit() {
            at += 1;
            continue;
        }
        let Scan { code, next } = scan_number(&text, at);
        at = next;
        let Some(code) = code.filter(|code| has_code_word(&text, code)) else {
            continue;
        };
        let scrubbed = scrubbed.get_or_insert_with(|| String::with_capacity(text.len()));
        scrubbed.push_str(&text[copied..code.start]);
        scrubbed.push_str(ONE_TIME_CODE);
        copied = code.end;
    }
    match scrubbed {
        Some(mut scrubbed) => {
            scrubbed.push_str(&text[copied..]);
            scrubbed
        }
        None => text,
    }
}

/// The byte range of a code candidate, including its letter prefix.
#[derive(Clone, Copy)]
struct Span {
    start: usize,
    digits_start: usize,
    end: usize,
}

/// The outcome of reading one number.
struct Scan {
    code: Option<Span>,
    /// The offset after the number.
    next: usize,
}

/// Reads the number that starts with the digit at `at`.
///
/// Returns the code, if the number is one, and the offset after the number.
fn scan_number(text: &str, at: usize) -> Scan {
    let bytes = text.as_bytes();
    let digits = digit_groups(bytes, at);
    let code = code_start(bytes, at)
        .filter(|_| (4..=8).contains(&digits.count) && !chains_on(bytes, &digits))
        .map(|start| Span {
            start,
            digits_start: at,
            end: digits.end,
        });
    Scan {
        code,
        next: digits.end,
    }
}

/// The digits of a number: one group, or two that a blank or a hyphen splits.
struct Digits {
    /// The offset after the last digit.
    end: usize,
    /// How many digits the number has.
    count: usize,
    groups: Groups,
}

/// How many groups the digits of a number form.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Groups {
    One,
    /// A blank or a hyphen splits the digits into two groups.
    Two,
}

/// Reads the digits from `at`, with a second group when the two groups hold 8 digits at most.
fn digit_groups(bytes: &[u8], at: usize) -> Digits {
    let digits_end = |from: usize| {
        bytes[from..].iter().position(|b| !b.is_ascii_digit()).map_or(bytes.len(), |n| from + n)
    };
    let first_end = digits_end(at);
    let first = Digits {
        end: first_end,
        count: first_end - at,
        groups: Groups::One,
    };
    if !matches!(bytes.get(first_end), Some(b' ' | b'-'))
        || !bytes.get(first_end + 1).is_some_and(u8::is_ascii_digit)
    {
        return first;
    }
    let second_end = digits_end(first_end + 1);
    let count = first.count + (second_end - first_end - 1);
    if count > 8 {
        return first;
    }
    Digits {
        end: second_end,
        count,
        groups: Groups::Two,
    }
}

/// Where a code that has its digits at `at` starts, with the letter prefix that a hyphen
/// joins to it. `None` when the digits continue a word or a larger number.
fn code_start(bytes: &[u8], at: usize) -> Option<usize> {
    let Some(&before) = at.checked_sub(1).and_then(|before| bytes.get(before)) else {
        return Some(at);
    };
    if before.is_ascii_alphabetic() {
        return None;
    }
    if matches!(before, b'.' | b',' | b':') && at > 1 && bytes[at - 2].is_ascii_digit() {
        return None;
    }
    if before == b'-' {
        let letters = bytes[..at - 1].iter().rev().take_while(|b| b.is_ascii_alphabetic());
        let prefix = letters.count();
        if prefix > 0 && (prefix == at - 1 || !bytes[at - 2 - prefix].is_ascii_alphanumeric()) {
            return Some(at - 1 - prefix);
        }
    }
    Some(at)
}

/// Whether the number of `digits` goes on as part of a larger one. A split number also goes on
/// after a hyphen or a blank.
fn chains_on(bytes: &[u8], digits: &Digits) -> bool {
    let separators: &[u8] = match digits.groups {
        Groups::One => b".,:",
        Groups::Two => b".,:- ",
    };
    bytes.get(digits.end).is_some_and(|b| separators.contains(b))
        && bytes.get(digits.end + 1).is_some_and(u8::is_ascii_digit)
}

/// Whether a code word occurs near `code`, or in its letter prefix.
fn has_code_word(text: &str, code: &Span) -> bool {
    let Span {
        start,
        digits_start,
        end,
    } = *code;
    let before_start =
        text[..start].char_indices().rev().nth(CODE_WORD_REACH - 1).map_or(0, |(offset, _)| offset);
    let after_end = text[end..]
        .char_indices()
        .nth(CODE_WORD_REACH)
        .map_or(text.len(), |(offset, _)| end + offset);
    if reads_as_year_or_reference(text, code) {
        return ends_with_code_word(&text[..digits_start])
            || (starts_with_ignoring_case(&text[end..], " is your ")
                && contains_code_word(text, end..after_end));
    }
    contains_code_word(text, before_start..digits_start) || contains_code_word(text, end..after_end)
}

/// Whether the number of `code` reads as a year, such as "2019", or as a reference, such as
/// "#4711".
///
/// Pages and documents are full of years and references near words such as "code" or "login",
/// so these numbers count as codes only when a code word stands right next to them.
fn reads_as_year_or_reference(text: &str, code: &Span) -> bool {
    let Span {
        start,
        digits_start,
        end,
    } = *code;
    if start != digits_start {
        return false;
    }
    let digits = &text[digits_start..end];
    let year = digits.len() == 4 && digits.parse().is_ok_and(|n: u16| (1900..=2099).contains(&n));
    year || text[..start].ends_with('#')
}

/// Whether `text` ends with a code word, apart from blanks, a colon, a hyphen, a `#`, an `=`,
/// or the word "is". For example, "Your PIN is " and "Login code: " end with code words.
fn ends_with_code_word(text: &str) -> bool {
    let separators: &[char] = &[' ', ':', '-', '#', '='];
    let mut text = text.trim_end_matches(separators);
    if let Some(rest) = strip_suffix_ignoring_case(text, " is") {
        text = rest.trim_end_matches(separators);
    }
    let bytes = text.as_bytes();
    CODE_WORDS.iter().any(|word| {
        [word.len(), word.len() + 1]
            .into_iter()
            .filter_map(|len| bytes.len().checked_sub(len))
            .any(|start| code_word_at(bytes, start, word) == Some(bytes.len()))
    })
}

/// Whether a code word occurs within `window` of `text` as a word of its own, ignoring case.
fn contains_code_word(text: &str, window: Range<usize>) -> bool {
    let bytes = text.as_bytes();
    window.clone().any(|at| {
        CODE_WORDS
            .iter()
            .any(|word| code_word_at(bytes, at, word).is_some_and(|end| end <= window.end))
    })
}

/// Where the code word `word` ends when it starts at `at` in `bytes`, ignoring case.
///
/// The code word may take a plural "s", as in "codes". It counts only as a whole word, and
/// "code" doesn't count after a word of [`CODE_QUALIFIERS`].
fn code_word_at(bytes: &[u8], at: usize, word: &str) -> Option<usize> {
    let candidate = bytes.get(at..at + word.len())?;
    if !candidate.eq_ignore_ascii_case(word.as_bytes())
        || at.checked_sub(1).is_some_and(|before| bytes[before].is_ascii_alphanumeric())
    {
        return None;
    }
    let is_word_byte = |offset: usize| bytes.get(offset).is_some_and(u8::is_ascii_alphanumeric);
    let mut end = at + word.len();
    if matches!(bytes.get(end), Some(b's' | b'S')) && !is_word_byte(end + 1) {
        end += 1;
    }
    if is_word_byte(end) || (word == "code" && is_qualified(&bytes[..at])) {
        return None;
    }
    Some(end)
}

/// The words that make a following "code" name something other than a one-time code. For
/// example, "ZIP code 10005" and "exit code 1234" don't hold one-time codes.
const CODE_QUALIFIERS: [&str; 9] =
    ["area", "country", "error", "exit", "post", "postal", "source", "status", "zip"];

/// Whether `before` ends with one of the [`CODE_QUALIFIERS`], apart from blanks and hyphens.
fn is_qualified(before: &[u8]) -> bool {
    let before = before.trim_ascii_end();
    let before = before.strip_suffix(b"-").unwrap_or(before);
    let letters = before.iter().rev().take_while(|b| b.is_ascii_alphabetic()).count();
    let last_word = &before[before.len() - letters..];
    CODE_QUALIFIERS.iter().any(|qualifier| last_word.eq_ignore_ascii_case(qualifier.as_bytes()))
}

fn starts_with_ignoring_case(text: &str, prefix: &str) -> bool {
    text.as_bytes()
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix.as_bytes()))
}

fn strip_suffix_ignoring_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let start = text.len().checked_sub(suffix.len())?;
    let rest = text.get(..start)?;
    text.as_bytes()[start..].eq_ignore_ascii_case(suffix.as_bytes()).then_some(rest)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::plain("https://example.com/a/b", "https://example.com/a/b")]
    #[case::query("https://example.com/a?token=hunter2", "https://example.com/a")]
    #[case::fragment("https://example.com/a#access_token=x", "https://example.com/a")]
    #[case::query_and_fragment("https://example.com/?a=1#b", "https://example.com/")]
    #[case::user_info("https://user:pw@example.com/a", "https://example.com/a")]
    #[case::at_in_path("https://example.com/@user/repo", "https://example.com/@user/repo")]
    #[case::file("file:///home/u/ci.html?token=x", "file:///home/u/ci.html")]
    #[case::data("data:text/html,<h1>secret</h1>", "data:text/html")]
    #[case::data_with_encoding("data:text/html;base64,c2VjcmV0", "data:text/html")]
    #[case::no_scheme("about:blank", "about:blank")]
    fn scrubs_urls(#[case] url: &str, #[case] expected: &str) {
        assert_eq!(scrub_url(url.to_owned()), expected);
    }

    #[rstest]
    #[case::plain("CI run #4711 – failed - Chromium", "CI run #4711 – failed - Chromium")]
    #[case::question("What is Rust? - Search", "What is Rust? - Search")]
    #[case::query("ci.html?token=hunter2 - Chromium", "ci.html - Chromium")]
    #[case::fragment("example.com/cb#access_token=abc - Firefox", "example.com/cb - Firefox")]
    #[case::two_words("a?x=1 b?y=2", "a b")]
    fn scrubs_titles(#[case] title: &str, #[case] expected: &str) {
        assert_eq!(scrub_title(title.to_owned()), expected);
    }

    #[rstest]
    #[case::query(
        "see https://example.com/cb?token=abc for details",
        "see https://example.com/cb for details"
    )]
    #[case::fragment("https://x.io/cb#access_token=abc", "https://x.io/cb")]
    #[case::anchor("https://docs.rs/serde#derive", "https://docs.rs/serde#derive")]
    #[case::search_without_value("https://example.com/a?print", "https://example.com/a?print")]
    #[case::credentials("https://user:pw@example.com/a", "https://example.com/a")]
    #[case::credentials_and_query("ftp://u:p@h.example/f?k=v", "ftp://h.example/f")]
    #[case::at_in_path("https://example.com/@user/repo", "https://example.com/@user/repo")]
    #[case::bare_host("open example.com/cb?token=x now", "open example.com/cb now")]
    #[case::www("www.example.com?ref=mail", "www.example.com")]
    #[case::in_parentheses("(https://example.com/a?t=1)", "(https://example.com/a")]
    #[case::in_markup("<a href=\"https://e.com/?t=1\">", "<a href=\"https://e.com/\">")]
    #[case::two_urls(
        "https://a.com/?x=1 and https://b.org/#y=2",
        "https://a.com/ and https://b.org/"
    )]
    #[case::code("if (a?b=c) { x?.y == 1 }", "if (a?b=c) { x?.y == 1 }")]
    #[case::prose("What is this? token=x", "What is this? token=x")]
    #[case::version("v1.2/x?y=1", "v1.2/x?y=1")]
    #[case::email("mail alice@example.com today", "mail alice@example.com today")]
    #[case::unicode(
        "ドキュメント https://例.jp/a?t=1 終わり",
        "ドキュメント https://例.jp/a 終わり"
    )]
    #[case::empty("", "")]
    fn scrubs_urls_in_text(#[case] text: &str, #[case] expected: &str) {
        assert_eq!(scrub_text_urls(text.to_owned()), expected);
    }

    proptest::proptest! {
        /// A scrubbed text is scrubbed again to itself, and a text without `?`, `#`, or `@`
        /// stays as it is.
        #[test]
        fn scrubbing_text_urls_is_idempotent(text in "[a-z.:/?#=@ ()-]{0,40}") {
            let once = scrub_text_urls(text.clone());
            if !text.contains(['?', '#', '@']) {
                proptest::prop_assert_eq!(&once, &text);
            }
            proptest::prop_assert_eq!(scrub_text_urls(once.clone()), once);
        }
    }

    #[rstest]
    #[case::verification(
        "Your verification code is 482913",
        "Your verification code is [one-time code]"
    )]
    #[case::prefix(
        "G-482913 is your Google verification code",
        "[one-time code] is your Google verification code"
    )]
    #[case::split_by_space("Your code: 123 456", "Your code: [one-time code]")]
    #[case::split_by_dash(
        "Use 8842-1190 to log in. Login code expires soon",
        "Use [one-time code] to log in. Login code expires soon"
    )]
    #[case::no_code_word("Meeting at 1430 in room 4021", "Meeting at 1430 in room 4021")]
    #[case::decimal("Order total 1299.00, code word: none", "Order total 1299.00, code word: none")]
    #[case::too_long("Build 20261114 finished", "Build 20261114 finished")]
    #[case::time("Your PIN is 12:30", "Your PIN is 12:30")]
    #[case::number_after_space(
        "Your code is 482913 7731",
        "Your code is [one-time code] [one-time code]"
    )]
    #[case::year_after_a_code("Your code is 482913 2026", "Your code is [one-time code] 2026")]
    #[case::year_near_a_code_word("Rust code shipped in 2016", "Rust code shipped in 2016")]
    #[case::year_right_after_a_code_word("Your Uber code: 2019", "Your Uber code: [one-time code]")]
    #[case::year_after_is("Your PIN is 1987", "Your PIN is [one-time code]")]
    #[case::year_before_is_your(
        "2019 is your Instagram code",
        "[one-time code] is your Instagram code"
    )]
    #[case::reference("Fix the login form (#4711)", "Fix the login form (#4711)")]
    #[case::reference_after_a_code_word("Use code #4711", "Use code #[one-time code]")]
    #[case::whole_words_only(
        "Codex rows in the codebase 482913",
        "Codex rows in the codebase 482913"
    )]
    #[case::zip_code("ZIP Code 10005, near the ZIP code", "ZIP Code 10005, near the ZIP code")]
    #[case::exit_code("Exited with exit-code 1234", "Exited with exit-code 1234")]
    #[case::number_after_dash("Code 482913 100 attempts", "Code [one-time code] 100 attempts")]
    #[case::plural("Backup codes: 12345678", "Backup codes: [one-time code]")]
    #[case::plural_abbreviation("OTPs 482913", "OTPs [one-time code]")]
    #[case::prefix_is_the_code_word("OTP-482913", "[one-time code]")]
    #[case::two_codes(
        "Code 111111, then code 222222",
        "Code [one-time code], then code [one-time code]"
    )]
    #[case::at_start_and_end("123456 code 654321", "[one-time code] code [one-time code]")]
    #[case::mixed_case("YOUR VeRiFiCaTiOn is 482913", "YOUR VeRiFiCaTiOn is [one-time code]")]
    #[case::nine_digits("Your code is 123456789", "Your code is 123456789")]
    #[case::multibyte("Ünï code → 482913 ✓ fertig", "Ünï code → [one-time code] ✓ fertig")]
    #[case::word_40_before(
        "code___________________________________ 482913",
        "code___________________________________ [one-time code]"
    )]
    #[case::word_41_before(
        "code____________________________________ 482913",
        "code____________________________________ 482913"
    )]
    #[case::multibyte_window(
        "codeééééééééééééééééééééééééééééééééééé 482913",
        "codeééééééééééééééééééééééééééééééééééé [one-time code]"
    )]
    #[case::word_40_after(
        "482913 ___________________________________code",
        "[one-time code] ___________________________________code"
    )]
    #[case::word_41_after(
        "482913 ____________________________________code",
        "482913 ____________________________________code"
    )]
    fn one_time_codes(#[case] text: &str, #[case] expected: &str) {
        assert_eq!(scrub_one_time_codes(text.to_owned()), expected);
    }

    #[rstest]
    fn text_without_codes_keeps_its_buffer() {
        let text = String::from("Your code is ready");
        let pointer = text.as_ptr();
        let scrubbed = scrub_one_time_codes(text);
        assert_eq!(scrubbed.as_ptr(), pointer);
    }
}
