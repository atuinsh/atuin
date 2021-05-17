pub use self::grapheme::{decode_grapheme, GraphemeIndices, Graphemes};
pub use self::sentence::{SentenceIndices, Sentences};
pub use self::whitespace::{whitespace_len_fwd, whitespace_len_rev};
pub use self::word::{
    WordIndices, Words, WordsWithBreakIndices, WordsWithBreaks,
};

mod fsm;
mod grapheme;
mod sentence;
mod whitespace;
mod word;
