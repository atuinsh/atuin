pub struct Cursor {
    source: String,
    index: usize,
}

impl From<String> for Cursor {
    fn from(source: String) -> Self {
        Self { source, index: 0 }
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
