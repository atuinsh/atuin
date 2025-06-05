#[derive(Default)]
pub struct ListState {
    pub offset: usize,
    pub selected: usize,
    pub max_entries: usize,
}

impl ListState {
    pub fn select(&mut self, index: usize) {
        self.selected = index;
    }
}
