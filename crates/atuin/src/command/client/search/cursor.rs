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
        let index = (index + 1..source.len().saturating_sub(1))
            .find(|&i| self.word_chars.contains(source.chars().nth(i).unwrap()))
            .unwrap_or(source.len());
        (index + 1..source.len().saturating_sub(1))
            .find(|&i| !self.word_chars.contains(source.chars().nth(i).unwrap()))
            .unwrap_or(source.len())
    }

    fn emacs_get_prev_word_pos(&self, source: &str, index: usize) -> usize {
        let index = (1..index)
            .rev()
            .find(|&i| self.word_chars.contains(source.chars().nth(i).unwrap()))
            .unwrap_or(0);
        (1..index)
            .rev()
            .find(|&i| !self.word_chars.contains(source.chars().nth(i).unwrap()))
            .map_or(0, |i| i + 1)
    }

    fn subl_get_next_word_pos(&self, source: &str, index: usize) -> usize {
        let index = (index..source.len().saturating_sub(1)).find(|&i| {
            self.is_word_boundary(
                source.chars().nth(i).unwrap(),
                source.chars().nth(i + 1).unwrap(),
            )
        });
        if index.is_none() {
            return source.len();
        }
        (index.unwrap() + 1..source.len())
            .find(|&i| !source.chars().nth(i).unwrap().is_whitespace())
            .unwrap_or(source.len())
    }

    fn subl_get_prev_word_pos(&self, source: &str, index: usize) -> usize {
        let index = (1..index)
            .rev()
            .find(|&i| !source.chars().nth(i).unwrap().is_whitespace());
        if index.is_none() {
            return 0;
        }
        (1..index.unwrap())
            .rev()
            .find(|&i| {
                self.is_word_boundary(
                    source.chars().nth(i - 1).unwrap(),
                    source.chars().nth(i).unwrap(),
                )
            })
            .unwrap_or(0)
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

    /// Checks if there's currently no input
    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
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
        if self.left() {
            self.remove()
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.source.clear();
        self.index = 0;
    }

    pub fn end(&mut self) {
        self.index = self.source.len();
    }

    pub fn start(&mut self) {
        self.index = 0;
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
    fn test_emacs_get_prev_word_pos() {
        let s = String::from("   aaa   ((()))bbb   ((()))   ");
        let indices = [(30, 15), (29, 15), (15, 3), (3, 0)];
        for (i_src, i_dest) in indices {
            assert_eq!(EMACS_WORD_JUMPER.get_prev_word_pos(&s, i_src), i_dest);
        }
        assert_eq!(EMACS_WORD_JUMPER.get_prev_word_pos("", 0), 0);
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
    fn test_subl_get_prev_word_pos() {
        let s = String::from("   aaa   ((()))bbb   ((()))   ");
        let indices = [(30, 21), (21, 15), (15, 9), (9, 3), (3, 0)];
        for (i_src, i_dest) in indices {
            assert_eq!(SUBL_WORD_JUMPER.get_prev_word_pos(&s, i_src), i_dest);
        }
        assert_eq!(SUBL_WORD_JUMPER.get_prev_word_pos("", 0), 0);
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
