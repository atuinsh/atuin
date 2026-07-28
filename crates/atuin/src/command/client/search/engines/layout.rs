const LEADING_KEYS: [&str; 19] = [
    "r", "R", "s", "e", "E", "f", "a", "q", "Q", "t", "T", "d", "w", "W", "c", "z", "x", "v", "g",
];

const VOWEL_KEYS: [&str; 21] = [
    "k", "o", "i", "O", "j", "p", "u", "P", "h", "hk", "ho", "hl", "y", "n", "nj", "np", "nl", "b",
    "m", "ml", "l",
];

const TRAILING_KEYS: [&str; 28] = [
    "", "r", "R", "rt", "s", "sw", "sg", "e", "f", "fr", "fa", "fq", "ft", "fx", "fv", "fg", "a",
    "q", "qt", "t", "T", "d", "w", "c", "z", "x", "v", "g",
];

pub fn dubeolsik_to_qwerty(input: &str) -> Option<String> {
    let mut output = String::with_capacity(input.len());
    let mut converted = false;

    for character in input.chars() {
        if let Some(keys) = jamo_keys(character) {
            output.push_str(keys);
            converted = true;
        } else if let Some(keys) = syllable_keys(character) {
            output.push_str(&keys);
            converted = true;
        } else {
            output.push(character);
        }
    }

    converted.then_some(output)
}

fn syllable_keys(character: char) -> Option<String> {
    const FIRST_SYLLABLE: u32 = 0xac00;
    const LAST_SYLLABLE: u32 = 0xd7a3;
    const TRAILING_COUNT: u32 = 28;
    const VOWEL_COUNT: u32 = 21;

    let codepoint = u32::from(character);
    if !(FIRST_SYLLABLE..=LAST_SYLLABLE).contains(&codepoint) {
        return None;
    }

    let offset = codepoint - FIRST_SYLLABLE;
    let leading = usize::try_from(offset / (VOWEL_COUNT * TRAILING_COUNT)).ok()?;
    let vowel = usize::try_from((offset / TRAILING_COUNT) % VOWEL_COUNT).ok()?;
    let trailing = usize::try_from(offset % TRAILING_COUNT).ok()?;

    Some(
        [
            LEADING_KEYS[leading],
            VOWEL_KEYS[vowel],
            TRAILING_KEYS[trailing],
        ]
        .concat(),
    )
}

fn jamo_keys(character: char) -> Option<&'static str> {
    let codepoint = u32::from(character);

    if ('\u{1100}'..='\u{1112}').contains(&character) {
        return LEADING_KEYS
            .get(usize::try_from(codepoint - 0x1100).ok()?)
            .copied();
    }
    if ('\u{1161}'..='\u{1175}').contains(&character) {
        return VOWEL_KEYS
            .get(usize::try_from(codepoint - 0x1161).ok()?)
            .copied();
    }
    if ('\u{11a8}'..='\u{11c2}').contains(&character) {
        return TRAILING_KEYS
            .get(usize::try_from(codepoint - 0x11a7).ok()?)
            .copied();
    }

    match character {
        'ㄱ' => Some("r"),
        'ㄲ' => Some("R"),
        'ㄳ' => Some("rt"),
        'ㄴ' => Some("s"),
        'ㄵ' => Some("sw"),
        'ㄶ' => Some("sg"),
        'ㄷ' => Some("e"),
        'ㄸ' => Some("E"),
        'ㄹ' => Some("f"),
        'ㄺ' => Some("fr"),
        'ㄻ' => Some("fa"),
        'ㄼ' => Some("fq"),
        'ㄽ' => Some("ft"),
        'ㄾ' => Some("fx"),
        'ㄿ' => Some("fv"),
        'ㅀ' => Some("fg"),
        'ㅁ' => Some("a"),
        'ㅂ' => Some("q"),
        'ㅃ' => Some("Q"),
        'ㅄ' => Some("qt"),
        'ㅅ' => Some("t"),
        'ㅆ' => Some("T"),
        'ㅇ' => Some("d"),
        'ㅈ' => Some("w"),
        'ㅉ' => Some("W"),
        'ㅊ' => Some("c"),
        'ㅋ' => Some("z"),
        'ㅌ' => Some("x"),
        'ㅍ' => Some("v"),
        'ㅎ' => Some("g"),
        'ㅏ' => Some("k"),
        'ㅐ' => Some("o"),
        'ㅑ' => Some("i"),
        'ㅒ' => Some("O"),
        'ㅓ' => Some("j"),
        'ㅔ' => Some("p"),
        'ㅕ' => Some("u"),
        'ㅖ' => Some("P"),
        'ㅗ' => Some("h"),
        'ㅘ' => Some("hk"),
        'ㅙ' => Some("ho"),
        'ㅚ' => Some("hl"),
        'ㅛ' => Some("y"),
        'ㅜ' => Some("n"),
        'ㅝ' => Some("nj"),
        'ㅞ' => Some("np"),
        'ㅟ' => Some("nl"),
        'ㅠ' => Some("b"),
        'ㅡ' => Some("m"),
        'ㅢ' => Some("ml"),
        'ㅣ' => Some("l"),
        _ => None,
    }
}
