// Keyframe generation is a complete, self-contained unit exercised by its own
// test, but its only in-crate consumer is the `session` module, which lands in a
// later task of this plan. Until then this function is technically dead code and
// `cargo clippy -- -D warnings` (what CI runs) would reject the crate.
#![allow(dead_code)]

//! Late-join / resync keyframes derived from the child's `vt100` screen.
//!
//! A keyframe is a self-contained byte sequence that repaints the child's
//! current visible state onto a blank terminal, so a viewer joining mid-session
//! (or one whose replay buffer the hub dropped) can catch up in a single frame.

/// Produce a byte sequence that repaints `screen` from a blank terminal.
/// `vt100::Screen::contents_formatted` emits a clear followed by the full
/// visible contents with SGR state, so a fresh parser fed these bytes ends in
/// the same visible state. Used as the resync/late-join keyframe.
#[must_use]
pub fn keyframe_bytes(screen: &vt100::Screen) -> Vec<u8> {
    screen.contents_formatted()
}

#[cfg(test)]
mod tests {
    use super::keyframe_bytes;

    #[test]
    fn keyframe_reproduces_screen_contents() {
        let mut a = vt100::Parser::new(10, 40, 0);
        a.process(b"\x1b[2J\x1b[HHello\r\n\x1b[1;31mWorld\x1b[0m");

        let kf = keyframe_bytes(a.screen());

        let mut b = vt100::Parser::new(10, 40, 0);
        b.process(&kf);

        assert_eq!(a.screen().contents(), b.screen().contents());
    }
}
