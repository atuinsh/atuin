use atuin_client::settings::WordJumpMode;

pub struct Cursor {
    source: String,
    index: usize,
}

impl From<String> for Cursor {
    fn from(source: String) -> Self {
        Self { source, index: 0 }
    }
}

pub struct WordJumper<'a> {
    word_chars: &'a str,
    word_jump_mode: WordJumpMode,
}

impl WordJumper<'_> {
    fn is_word_boundary(&self, c: char, next_c: char) -> bool {
        (c.is_whitespace() && !next_c.is_whitespace())
            || (!c.is_whitespace() && next_c.is_whitespace())
            || (self.word_chars.contains(c) && !self.word_chars.contains(next_c))
            || (!self.word_chars.contains(c) && self.word_chars.contains(next_c))
    }

    fn emacs_get_next_word_pos(&self, source: &str, index: usize) -> usize {
        let mut valid_range = source.char_indices().filter(|&(char_position, _)| {
            index < char_position && char_position < source.len().saturating_sub(1)
        });
        let index = valid_range
            .find(|&(_, char_value)| self.word_chars.contains(char_value))
            .unwrap_or((source.len(), '\0'))
            .0;
        source
            .char_indices()
            .filter(|&(char_position, _)| {
                index < char_position && char_position < source.len().saturating_sub(1)
            })
            .find(|&(_, char_value)| !self.word_chars.contains(char_value))
            .unwrap_or((source.len(), '\0'))
            .0
    }

    fn emacs_get_prev_word_pos(&self, source: &str, index: usize) -> usize {
        let valid_range = source
            .char_indices()
            .filter(|&(char_position, _)| 1 <= char_position && char_position < index);
        let index_pair = valid_range
            .rev()
            .find(|&(_, char_value)| self.word_chars.contains(char_value))
            .unwrap_or_default();
        source
            .char_indices()
            .filter(|&(char_position, _)| 1 <= char_position && char_position < index_pair.0)
            .rev()
            .find(|&(_, char_value)| !self.word_chars.contains(char_value))
            .map_or(0, |(t, _)| t + 1)
    }

    fn subl_get_next_word_pos(&self, source: &str, index: usize) -> usize {
        let mut iterator = source
            .char_indices()
            .filter(|&(char_index, _)| {
                index <= char_index && char_index < source.len().saturating_sub(1)
            })
            .peekable();
        let index: Option<usize> = loop {
            if let Some((char_index, char_value)) = iterator.next() {
                if let Some(&(_, next_char_value)) = iterator.peek()
                    && self.is_word_boundary(char_value, next_char_value)
                {
                    break Some(char_index);
                }
            } else {
                break None;
            }
        };
        if index.is_none() {
            return source.len();
        }
        source
            .char_indices()
            .filter(|&(char_index, _)| index.unwrap() < char_index && char_index < source.len())
            .find(|(_, char_value)| !char_value.is_whitespace())
            .unwrap_or((source.len(), '\0'))
            .0
    }

    fn subl_get_prev_word_pos(&self, source: &str, index: usize) -> usize {
        let index = source
            .char_indices()
            .filter(|&(char_index, _)| 1 <= char_index && char_index < index)
            .rev()
            .find(|(_, char_value)| !char_value.is_whitespace());
        if index.is_none() {
            return 0;
        }

        let mut iter = source
            .char_indices()
            .filter(|&(char_index, _)| 1 <= char_index && char_index < index.unwrap().0)
            .rev()
            .peekable();
        loop {
            if let Some((char_index, char_value)) = iter.next()
                && let Some(&(_, prev_word)) = iter.peek()
            {
                if self.is_word_boundary(prev_word, char_value) {
                    break char_index;
                }
            } else {
                break 0;
            }
        }
    }

    fn get_next_word_pos(&self, source: &str, index: usize) -> usize {
        match self.word_jump_mode {
            WordJumpMode::Emacs => self.emacs_get_next_word_pos(source, index),
            WordJumpMode::Subl => self.subl_get_next_word_pos(source, index),
        }
    }

    fn get_prev_word_pos(&self, source: &str, index: usize) -> usize {
        match self.word_jump_mode {
            WordJumpMode::Emacs => self.emacs_get_prev_word_pos(source, index),
            WordJumpMode::Subl => self.subl_get_prev_word_pos(source, index),
        }
    }
}

impl Cursor {
    pub fn as_str(&self) -> &str {
        self.source.as_str()
    }

    pub fn into_inner(self) -> String {
        self.source
    }

    /// Returns the string before the cursor
    pub fn substring(&self) -> &str {
        &self.source[..self.index]
    }

    /// Returns the currently selected [`char`]
    pub fn char(&self) -> Option<char> {
        self.source[self.index..].chars().next()
    }

    pub fn right(&mut self) {
        if self.index < self.source.len() {
            loop {
                self.index += 1;
                if self.source.is_char_boundary(self.index) {
                    break;
                }
            }
        }
    }

    pub fn left(&mut self) -> bool {
        if self.index > 0 {
            loop {
                self.index -= 1;
                if self.source.is_char_boundary(self.index) {
                    break true;
                }
            }
        } else {
            false
        }
    }

    pub fn next_word(&mut self, word_chars: &str, word_jump_mode: WordJumpMode) {
        let word_jumper = WordJumper {
            word_chars,
            word_jump_mode,
        };
        self.index = word_jumper.get_next_word_pos(&self.source, self.index);
    }

    pub fn prev_word(&mut self, word_chars: &str, word_jump_mode: WordJumpMode) {
        let word_jumper = WordJumper {
            word_chars,
            word_jump_mode,
        };
        self.index = word_jumper.get_prev_word_pos(&self.source, self.index);
    }

    /// Move cursor to the end of the current/next word (vim `e` motion).
    ///
    /// If cursor is in the middle of a word, moves to the end of that word.
    /// If cursor is at the end of a word (or on whitespace), moves to the
    /// end of the next word.
    pub fn word_end(&mut self, word_chars: &str) {
        let len = self.source.len();
        if self.index >= len {
            return;
        }

        let chars: Vec<char> = self.source.chars().collect();
        let mut char_idx = self.source[..self.index].chars().count();

        if char_idx >= chars.len() {
            return;
        }

        let current = chars[char_idx];

        // Check if we're at a word boundary (end of current word or on whitespace)
        let at_word_boundary = current.is_whitespace() || char_idx + 1 >= chars.len() || {
            let next = chars[char_idx + 1];
            next.is_whitespace() || (word_chars.contains(current) != word_chars.contains(next))
        };

        // If at word boundary, advance past it and skip whitespace to find next word
        if at_word_boundary {
            char_idx += 1;
            while char_idx < chars.len() && chars[char_idx].is_whitespace() {
                char_idx += 1;
            }
        }

        // If we've gone past end, go to end of string
        if char_idx >= chars.len() {
            self.index = len;
            return;
        }

        // Find end of word: advance until next char is whitespace or different word type
        let in_word_chars = word_chars.contains(chars[char_idx]);
        while char_idx < chars.len() {
            let next_idx = char_idx + 1;
            if next_idx >= chars.len() {
                // At last char, move past it
                char_idx = next_idx;
                break;
            }
            let next_c = chars[next_idx];
            if next_c.is_whitespace() || (word_chars.contains(next_c) != in_word_chars) {
                // Next char is start of new word/whitespace, so current char is end
                char_idx = next_idx;
                break;
            }
            char_idx += 1;
        }

        // Convert char index back to byte index
        self.index = chars.iter().take(char_idx).map(|c| c.len_utf8()).sum();
    }

    pub fn insert(&mut self, c: char) {
        self.source.insert(self.index, c);
        self.index += c.len_utf8();
    }

    pub fn remove(&mut self) -> Option<char> {
        if self.index < self.source.len() {
            Some(self.source.remove(self.index))
        } else {
            None
        }
    }

    pub fn remove_next_word(&mut self, word_chars: &str, word_jump_mode: WordJumpMode) {
        let word_jumper = WordJumper {
            word_chars,
            word_jump_mode,
        };
        let next_index = word_jumper.get_next_word_pos(&self.source, self.index);
        self.source.replace_range(self.index..next_index, "");
    }

    pub fn remove_prev_word(&mut self, word_chars: &str, word_jump_mode: WordJumpMode) {
        let word_jumper = WordJumper {
            word_chars,
            word_jump_mode,
        };
        let next_index = word_jumper.get_prev_word_pos(&self.source, self.index);
        self.source.replace_range(next_index..self.index, "");
        self.index = next_index;
    }

    pub fn back(&mut self) -> Option<char> {
        if self.left() { self.remove() } else { None }
    }

    pub fn clear(&mut self) {
        self.source.clear();
        self.index = 0;
    }

    pub fn clear_to_start(&mut self) {
        self.source.replace_range(..self.index, "");
        self.index = 0;
    }

    pub fn clear_to_end(&mut self) {
        self.source.replace_range(self.index.., "");
        self.index = self.source.len();
    }

    pub fn end(&mut self) {
        self.index = self.source.len();
    }

    pub fn start(&mut self) {
        self.index = 0;
    }

    pub fn position(&self) -> usize {
        self.index
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::Cursor;
    use super::*;

    static EMACS_WORD_JUMPER: WordJumper = WordJumper {
        word_chars: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
        word_jump_mode: WordJumpMode::Emacs,
    };

    static SUBL_WORD_JUMPER: WordJumper = WordJumper {
        word_chars: "./\\()\"'-:,.;<>~!@#$%^&*|+=[]{}`~?",
        word_jump_mode: WordJumpMode::Subl,
    };

    #[test]
    fn right() {
        // ö is 2 bytes
        let mut c = Cursor::from(String::from("öaöböcödöeöfö"));
        let indices = [0, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 20, 20, 20];
        for i in indices {
            assert_eq!(c.index, i);
            c.right();
        }
    }

    #[test]
    fn left() {
        // ö is 2 bytes
        let mut c = Cursor::from(String::from("öaöböcödöeöfö"));
        c.end();
        let indices = [20, 18, 17, 15, 14, 12, 11, 9, 8, 6, 5, 3, 2, 0, 0, 0, 0];
        for i in indices {
            assert_eq!(c.index, i);
            c.left();
        }
    }

    #[test]
    fn test_emacs_get_next_word_pos() {
        let s = String::from("   aaa   ((()))bbb   ((()))   ");
        let indices = [(0, 6), (3, 6), (7, 18), (19, 30)];
        for (i_src, i_dest) in indices {
            assert_eq!(EMACS_WORD_JUMPER.get_next_word_pos(&s, i_src), i_dest);
        }
        assert_eq!(EMACS_WORD_JUMPER.get_next_word_pos("", 0), 0);
    }
    #[test]
    fn test_emacs_get_next_word_pos_non_ascii() {
        let s = String::from(" 😀 test");
        let indices = [(0, 10), (1, 10)];
        for (i_src, i_dest) in indices {
            assert_eq!(EMACS_WORD_JUMPER.get_next_word_pos(&s, i_src), i_dest);
        }
    }

    #[test]
    fn test_emacs_get_prev_word_pos() {
        let s = String::from("   aaa   ((()))bbb   ((()))   ");
        let indices = [(30, 15), (29, 15), (15, 3), (3, 0)];
        for (i_src, i_dest) in indices {
            assert_eq!(EMACS_WORD_JUMPER.get_prev_word_pos(&s, i_src), i_dest);
        }
        assert_eq!(EMACS_WORD_JUMPER.get_prev_word_pos("", 0), 0);
    }
    #[test]
    fn test_emacs_get_prev_word_pos_non_ascii() {
        let s = String::from(" 😀 test");
        let indices = [(6, 0), (1, 0)];
        for (i_src, i_dest) in indices {
            assert_eq!(EMACS_WORD_JUMPER.get_prev_word_pos(&s, i_src), i_dest);
        }
    }

    #[test]
    fn test_subl_get_next_word_pos() {
        let s = String::from("   aaa   ((()))bbb   ((()))   ");
        let indices = [(0, 3), (1, 3), (3, 9), (9, 15), (15, 21), (21, 30)];
        for (i_src, i_dest) in indices {
            assert_eq!(SUBL_WORD_JUMPER.get_next_word_pos(&s, i_src), i_dest);
        }
        assert_eq!(SUBL_WORD_JUMPER.get_next_word_pos("", 0), 0);
    }
    #[test]
    fn test_subl_get_next_word_pos_non_ascii() {
        let s = String::from(" hi 😀 ((test");
        let indices = [(0, 1), (1, 4), (4, 9)];
        for (i_src, i_dest) in indices {
            assert_eq!(SUBL_WORD_JUMPER.get_next_word_pos(&s, i_src), i_dest);
        }
    }

    #[test]
    fn test_subl_get_prev_word_pos() {
        let s = String::from("   aaa   ((()))bbb   ((()))   ");
        let indices = [(30, 21), (21, 15), (15, 9), (9, 3), (3, 0)];
        for (i_src, i_dest) in indices {
            assert_eq!(SUBL_WORD_JUMPER.get_prev_word_pos(&s, i_src), i_dest);
        }
        assert_eq!(SUBL_WORD_JUMPER.get_prev_word_pos("", 0), 0);
    }
    #[test]
    fn test_subl_get_prev_word_pos_non_ascii() {
        let s = String::from(" hi 😀 ((test");
        let indices = [(1, 0), (9, 3)];
        for (i_src, i_dest) in indices {
            assert_eq!(SUBL_WORD_JUMPER.get_prev_word_pos(&s, i_src), i_dest);
        }
    }
    #[test]
    fn pop() {
        let mut s = String::from("öaöböcödöeöfö");
        let mut c = Cursor::from(s.clone());
        c.end();
        while !s.is_empty() {
            let c1 = s.pop();
            let c2 = c.back();
            assert_eq!(c1, c2);
            assert_eq!(s.as_str(), c.substring());
        }
        let c1 = s.pop();
        let c2 = c.back();
        assert_eq!(c1, c2);
    }

    #[test]
    fn back() {
        let mut c = Cursor::from(String::from("öaöböcödöeöfö"));
        // move to                                 ^
        for _ in 0..4 {
            c.right();
        }
        assert_eq!(c.substring(), "öaöb");
        assert_eq!(c.back(), Some('b'));
        assert_eq!(c.back(), Some('ö'));
        assert_eq!(c.back(), Some('a'));
        assert_eq!(c.back(), Some('ö'));
        assert_eq!(c.back(), None);
        assert_eq!(c.as_str(), "öcödöeöfö");
    }

    #[test]
    fn insert() {
        let mut c = Cursor::from(String::from("öaöböcödöeöfö"));
        // move to                                 ^
        for _ in 0..4 {
            c.right();
        }
        assert_eq!(c.substring(), "öaöb");
        c.insert('ö');
        c.insert('g');
        c.insert('ö');
        c.insert('h');
        assert_eq!(c.substring(), "öaöbögöh");
        assert_eq!(c.as_str(), "öaöbögöhöcödöeöfö");
    }
}
