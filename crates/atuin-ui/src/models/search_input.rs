/// The search query text plus an editing cursor. The cursor is a byte offset
/// into `value`, always kept on a char boundary so multi-byte input is safe.
#[derive(Default)]
pub struct SearchInput {
    value: String,
    cursor: usize,
}

impl SearchInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// Characters before the cursor — the column to draw the terminal cursor at.
    pub fn cursor_char(&self) -> usize {
        self.value[..self.cursor].chars().count()
    }

    pub fn insert(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if let Some(prev) = self.value[..self.cursor].chars().next_back() {
            let start = self.cursor - prev.len_utf8();
            self.value.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }

    pub fn delete(&mut self) {
        if let Some(next) = self.value[self.cursor..].chars().next() {
            self.value
                .replace_range(self.cursor..self.cursor + next.len_utf8(), "");
        }
    }

    pub fn left(&mut self) {
        if let Some(prev) = self.value[..self.cursor].chars().next_back() {
            self.cursor -= prev.len_utf8();
        }
    }

    pub fn right(&mut self) {
        if let Some(next) = self.value[self.cursor..].chars().next() {
            self.cursor += next.len_utf8();
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.value.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> SearchInput {
        let mut input = SearchInput::new();
        for c in s.chars() {
            input.insert(c);
        }
        input
    }

    #[test]
    fn insert_appends_and_advances() {
        let input = typed("ls");
        assert_eq!(input.value(), "ls");
        assert_eq!(input.cursor_char(), 2);
    }

    #[test]
    fn backspace_removes_char_before_cursor() {
        let mut input = typed("lsx");
        input.backspace();
        assert_eq!(input.value(), "ls");
        assert_eq!(input.cursor_char(), 2);
    }

    #[test]
    fn left_right_move_over_utf8() {
        let mut input = typed("aé");
        input.left(); // over 'é' (2 bytes)
        assert_eq!(input.cursor_char(), 1);
        input.insert('X'); // insert between 'a' and 'é'
        assert_eq!(input.value(), "aXé");
        input.right();
        assert_eq!(input.cursor_char(), 3);
    }

    #[test]
    fn delete_removes_char_at_cursor() {
        let mut input = typed("abc");
        input.home();
        input.delete();
        assert_eq!(input.value(), "bc");
        assert_eq!(input.cursor_char(), 0);
    }

    #[test]
    fn home_and_end() {
        let mut input = typed("hello");
        input.home();
        assert_eq!(input.cursor_char(), 0);
        input.end();
        assert_eq!(input.cursor_char(), 5);
    }

    #[test]
    fn backspace_and_left_are_noops_when_empty() {
        let mut input = SearchInput::new();
        input.backspace();
        input.left();
        assert_eq!(input.value(), "");
        assert_eq!(input.cursor_char(), 0);
    }
}
