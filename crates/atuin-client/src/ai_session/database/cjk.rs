/// Characters that belong to Chinese, Japanese, and Korean writing systems.
#[must_use]
pub fn is_cjk(c: char) -> bool {
    matches!(
        u32::from(c),
        0x3040..=0x30FF     // Hiragana and Katakana
            | 0x3100..=0x312F // Bopomofo
            | 0x3130..=0x318F // Hangul Compatibility Jamo
            | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xAC00..=0xD7AF // Hangul Syllables
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
            | 0x20000..=0x323AF // CJK Extensions B through I
    ) && c.is_alphabetic()
}

/// Whether `s` contains any CJK character.
#[must_use]
pub fn contains_cjk(s: &str) -> bool {
    s.chars().any(is_cjk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn detects_cjk_characters() {
        assert!(is_cjk('中'));
        assert!(is_cjk('文'));
        assert!(is_cjk('あ'));
        assert!(is_cjk('ア'));
        assert!(is_cjk('한'));
        assert!(!is_cjk('a'));
        assert!(!is_cjk('1'));
        assert!(!is_cjk(' '));
        assert!(!is_cjk('・'));
    }

    #[rstest]
    fn detects_cjk_in_strings() {
        assert!(contains_cjk("hello 世界"));
        assert!(contains_cjk("在提交代码前把修改暂存进索引，同时遵守仓库约定。"));
        assert!(!contains_cjk("hello world 123"));
    }
}
