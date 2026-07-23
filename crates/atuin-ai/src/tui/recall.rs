//! Readline-style recall of the session's submitted messages: Up steps
//! back through them, Down steps forward and finally restores the
//! in-progress draft. The caller decides *when* a keypress means recall
//! (the editor cursor had nowhere to go); this owns only the position.

#[derive(Debug, Default)]
pub(crate) struct RecallState {
    /// Index into the message list currently loaded in the editor;
    /// `None` means the editor holds the live draft.
    index: Option<usize>,
    /// Editor text stashed when recall began, restored when the user
    /// steps forward past the newest message.
    draft: String,
}

impl RecallState {
    /// Step to an older message. `current` is the editor's text, stashed
    /// as the draft when recall begins. Returns the text to load, or
    /// `None` when there is nothing older.
    pub fn back(&mut self, messages: &[&str], current: &str) -> Option<String> {
        // A stale index (the event list shrank) clamps to the newest.
        let next = match self.index {
            None => messages.len().checked_sub(1)?,
            Some(i) => i.min(messages.len()).checked_sub(1)?,
        };
        if self.index.is_none() {
            self.draft = current.to_string();
        }
        self.index = Some(next);
        Some(messages[next].to_string())
    }

    /// Step toward newer messages; past the newest, leave recall and
    /// restore the draft. Returns the text to load, or `None` when not
    /// recalling.
    pub fn forward(&mut self, messages: &[&str]) -> Option<String> {
        let next = self.index? + 1;
        if next >= messages.len() {
            self.index = None;
            return Some(std::mem::take(&mut self.draft));
        }
        self.index = Some(next);
        Some(messages[next].to_string())
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MSGS: &[&str] = &["one", "two", "three"];

    #[test]
    fn back_walks_newest_to_oldest_then_stops() {
        let mut r = RecallState::default();
        assert_eq!(r.back(MSGS, "draft").as_deref(), Some("three"));
        assert_eq!(r.back(MSGS, "").as_deref(), Some("two"));
        assert_eq!(r.back(MSGS, "").as_deref(), Some("one"));
        assert_eq!(r.back(MSGS, ""), None);
    }

    #[test]
    fn forward_returns_to_the_draft() {
        let mut r = RecallState::default();
        r.back(MSGS, "draft");
        r.back(MSGS, "ignored: draft only stashes on entry");
        assert_eq!(r.forward(MSGS).as_deref(), Some("three"));
        assert_eq!(r.forward(MSGS).as_deref(), Some("draft"));
        // Out of recall: forward is a no-op until back re-enters.
        assert_eq!(r.forward(MSGS), None);
    }

    #[test]
    fn back_with_no_messages_is_a_noop() {
        let mut r = RecallState::default();
        assert_eq!(r.back(&[], "draft"), None);
        assert_eq!(r.forward(&[]), None);
    }

    #[test]
    fn stale_index_clamps_to_the_newest_message() {
        let mut r = RecallState::default();
        r.back(MSGS, "");
        assert_eq!(r.back(&["only"], "").as_deref(), Some("only"));
    }

    #[test]
    fn reset_forgets_position_and_draft() {
        let mut r = RecallState::default();
        r.back(MSGS, "draft");
        r.reset();
        assert_eq!(r.forward(MSGS), None);
        assert_eq!(r.back(MSGS, "new").as_deref(), Some("three"));
        assert_eq!(r.forward(MSGS).as_deref(), Some("new"));
    }
}
