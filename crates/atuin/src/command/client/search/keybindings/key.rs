use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MediaKeyCode};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A single key press with modifiers (e.g. `ctrl-c`, `alt-f`, `enter`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(clippy::struct_excessive_bools)]
pub struct SingleKey {
    pub code: KeyCodeValue,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

/// The key code portion of a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCodeValue {
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    F(u8),
    Media(MediaKeyCode),
}

/// A key input that may be a single key or a multi-key sequence (e.g. `g g`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyInput {
    Single(SingleKey),
    Sequence(Vec<SingleKey>),
}

impl SingleKey {
    /// Convert a crossterm `KeyEvent` into a `SingleKey`.
    pub fn from_event(event: &KeyEvent) -> Option<Self> {
        let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
        let alt = event.modifiers.contains(KeyModifiers::ALT);
        let shift = event.modifiers.contains(KeyModifiers::SHIFT);
        let super_key = event.modifiers.contains(KeyModifiers::SUPER);

        let code = match event.code {
            KeyCode::Char(' ') => KeyCodeValue::Space,
            KeyCode::Char(c) => {
                // If shift is the only modifier and it's an uppercase letter,
                // we store the uppercase char directly and clear the shift flag
                // since the case already encodes it.
                if shift && !ctrl && !alt && !super_key && c.is_ascii_uppercase() {
                    return Some(SingleKey {
                        code: KeyCodeValue::Char(c),
                        ctrl: false,
                        alt: false,
                        shift: false,
                        super_key: false,
                    });
                }
                KeyCodeValue::Char(c)
            }
            KeyCode::Enter => KeyCodeValue::Enter,
            KeyCode::Esc => KeyCodeValue::Esc,
            KeyCode::Tab => KeyCodeValue::Tab,
            // BackTab is sent by many terminals for Shift+Tab
            KeyCode::BackTab => {
                return Some(SingleKey {
                    code: KeyCodeValue::Tab,
                    ctrl,
                    alt,
                    shift: true,
                    super_key,
                });
            }
            KeyCode::Backspace => KeyCodeValue::Backspace,
            KeyCode::Delete => KeyCodeValue::Delete,
            KeyCode::Insert => KeyCodeValue::Insert,
            KeyCode::Up => KeyCodeValue::Up,
            KeyCode::Down => KeyCodeValue::Down,
            KeyCode::Left => KeyCodeValue::Left,
            KeyCode::Right => KeyCodeValue::Right,
            KeyCode::Home => KeyCodeValue::Home,
            KeyCode::End => KeyCodeValue::End,
            KeyCode::PageUp => KeyCodeValue::PageUp,
            KeyCode::PageDown => KeyCodeValue::PageDown,
            KeyCode::F(n) => KeyCodeValue::F(n),
            KeyCode::Media(m) => KeyCodeValue::Media(m),
            _ => return None,
        };

        Some(SingleKey {
            code,
            ctrl,
            alt,
            shift: if matches!(code, KeyCodeValue::Char(_)) {
                false
            } else {
                shift
            },
            super_key,
        })
    }

    /// Parse a key string like `"ctrl-c"`, `"alt-f"`, `"enter"`, `"G"`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let parts: Vec<&str> = s.split('-').collect();

        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut super_key = false;

        // All parts except the last are modifiers
        for &part in &parts[..parts.len() - 1] {
            match part.to_lowercase().as_str() {
                "ctrl" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                "super" | "cmd" | "win" => super_key = true,
                _ => return Err(format!("unknown modifier: {part}")),
            }
        }

        let key_part = parts[parts.len() - 1];
        let code = match key_part.to_lowercase().as_str() {
            "enter" | "return" => KeyCodeValue::Enter,
            "esc" | "escape" => KeyCodeValue::Esc,
            "tab" => KeyCodeValue::Tab,
            "backspace" => KeyCodeValue::Backspace,
            "delete" | "del" => KeyCodeValue::Delete,
            "insert" | "ins" => KeyCodeValue::Insert,
            "up" => KeyCodeValue::Up,
            "down" => KeyCodeValue::Down,
            "left" => KeyCodeValue::Left,
            "right" => KeyCodeValue::Right,
            "home" => KeyCodeValue::Home,
            "end" => KeyCodeValue::End,
            "pageup" => KeyCodeValue::PageUp,
            "pagedown" => KeyCodeValue::PageDown,
            "space" => KeyCodeValue::Space,
            s if s.starts_with('f') && s.len() > 1 => {
                // Parse function keys like "f1", "f12"
                if let Ok(n) = s[1..].parse::<u8>() {
                    if (1..=24).contains(&n) {
                        KeyCodeValue::F(n)
                    } else {
                        return Err(format!("function key out of range: {key_part}"));
                    }
                } else {
                    return Err(format!("unknown key: {key_part}"));
                }
            }
            "[" => KeyCodeValue::Char('['),
            "]" => KeyCodeValue::Char(']'),
            "?" => KeyCodeValue::Char('?'),
            "/" => KeyCodeValue::Char('/'),
            "$" => KeyCodeValue::Char('$'),
            // Media keys (no dashes - the parser splits on dash for modifiers)
            "play" => KeyCodeValue::Media(MediaKeyCode::Play),
            "pause" => KeyCodeValue::Media(MediaKeyCode::Pause),
            "playpause" => KeyCodeValue::Media(MediaKeyCode::PlayPause),
            "stop" => KeyCodeValue::Media(MediaKeyCode::Stop),
            "fastforward" => KeyCodeValue::Media(MediaKeyCode::FastForward),
            "rewind" => KeyCodeValue::Media(MediaKeyCode::Rewind),
            "tracknext" => KeyCodeValue::Media(MediaKeyCode::TrackNext),
            "trackprevious" => KeyCodeValue::Media(MediaKeyCode::TrackPrevious),
            "record" => KeyCodeValue::Media(MediaKeyCode::Record),
            "lowervolume" => KeyCodeValue::Media(MediaKeyCode::LowerVolume),
            "raisevolume" => KeyCodeValue::Media(MediaKeyCode::RaiseVolume),
            "mutevolume" | "mute" => KeyCodeValue::Media(MediaKeyCode::MuteVolume),
            _ => {
                let chars: Vec<char> = key_part.chars().collect();
                if chars.len() == 1 {
                    let c = chars[0];
                    // An uppercase letter implies shift (unless shift already specified)
                    if c.is_ascii_uppercase() && !ctrl && !alt && !super_key {
                        return Ok(SingleKey {
                            code: KeyCodeValue::Char(c),
                            ctrl: false,
                            alt: false,
                            shift: false,
                            super_key: false,
                        });
                    }
                    KeyCodeValue::Char(c)
                } else {
                    return Err(format!("unknown key: {key_part}"));
                }
            }
        };

        Ok(SingleKey {
            code,
            ctrl,
            alt,
            shift,
            super_key,
        })
    }
}

impl fmt::Display for SingleKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.super_key {
            write!(f, "super-")?;
        }
        if self.ctrl {
            write!(f, "ctrl-")?;
        }
        if self.alt {
            write!(f, "alt-")?;
        }
        if self.shift {
            write!(f, "shift-")?;
        }
        match &self.code {
            KeyCodeValue::Char(c) => write!(f, "{c}"),
            KeyCodeValue::Enter => write!(f, "enter"),
            KeyCodeValue::Esc => write!(f, "esc"),
            KeyCodeValue::Tab => write!(f, "tab"),
            KeyCodeValue::Backspace => write!(f, "backspace"),
            KeyCodeValue::Delete => write!(f, "delete"),
            KeyCodeValue::Insert => write!(f, "insert"),
            KeyCodeValue::Up => write!(f, "up"),
            KeyCodeValue::Down => write!(f, "down"),
            KeyCodeValue::Left => write!(f, "left"),
            KeyCodeValue::Right => write!(f, "right"),
            KeyCodeValue::Home => write!(f, "home"),
            KeyCodeValue::End => write!(f, "end"),
            KeyCodeValue::PageUp => write!(f, "pageup"),
            KeyCodeValue::PageDown => write!(f, "pagedown"),
            KeyCodeValue::Space => write!(f, "space"),
            KeyCodeValue::F(n) => write!(f, "f{n}"),
            KeyCodeValue::Media(m) => match m {
                MediaKeyCode::Play => write!(f, "play"),
                MediaKeyCode::Pause => write!(f, "media-pause"),
                MediaKeyCode::PlayPause => write!(f, "playpause"),
                MediaKeyCode::Stop => write!(f, "stop"),
                MediaKeyCode::FastForward => write!(f, "fastforward"),
                MediaKeyCode::Rewind => write!(f, "rewind"),
                MediaKeyCode::TrackNext => write!(f, "tracknext"),
                MediaKeyCode::TrackPrevious => write!(f, "trackprevious"),
                MediaKeyCode::Record => write!(f, "record"),
                MediaKeyCode::LowerVolume => write!(f, "lowervolume"),
                MediaKeyCode::RaiseVolume => write!(f, "raisevolume"),
                MediaKeyCode::MuteVolume => write!(f, "mutevolume"),
                MediaKeyCode::Reverse => write!(f, "reverse"),
            },
        }
    }
}

impl KeyInput {
    /// Parse a key input string. Supports multi-key sequences separated by spaces
    /// (e.g. `"g g"`).
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        // Check for space-separated multi-key sequences
        // But don't split "space" or modifier combos like "ctrl-a"
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() > 1 {
            let keys: Result<Vec<SingleKey>, String> =
                parts.iter().map(|p| SingleKey::parse(p)).collect();
            Ok(KeyInput::Sequence(keys?))
        } else {
            Ok(KeyInput::Single(SingleKey::parse(s)?))
        }
    }
}

impl fmt::Display for KeyInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyInput::Single(k) => write!(f, "{k}"),
            KeyInput::Sequence(keys) => {
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{k}")?;
                }
                Ok(())
            }
        }
    }
}

impl Serialize for KeyInput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for KeyInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        KeyInput::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rstest::rstest;

    use super::*;

    // (A) parse -> (code, flags). Asserting ALL flags is a strengthening,
    // verified consistent with the parse logic.
    #[rstest]
    #[case::char_a("a", KeyCodeValue::Char('a'), false, false, false, false)]
    #[case::enter("enter", KeyCodeValue::Enter, false, false, false, false)]
    #[case::esc("esc", KeyCodeValue::Esc, false, false, false, false)]
    #[case::tab("tab", KeyCodeValue::Tab, false, false, false, false)]
    #[case::space("space", KeyCodeValue::Space, false, false, false, false)]
    #[case::ctrl_c("ctrl-c", KeyCodeValue::Char('c'), true, false, false, false)]
    #[case::alt_f("alt-f", KeyCodeValue::Char('f'), false, true, false, false)]
    #[case::ctrl_alt_x("ctrl-alt-x", KeyCodeValue::Char('x'), true, true, false, false)]
    #[case::upper_g("G", KeyCodeValue::Char('G'), false, false, false, false)]
    #[case::ctrl_lbracket("ctrl-[", KeyCodeValue::Char('['), true, false, false, false)]
    #[case::question("?", KeyCodeValue::Char('?'), false, false, false, false)]
    #[case::slash("/", KeyCodeValue::Char('/'), false, false, false, false)]
    #[case::super_a("super-a", KeyCodeValue::Char('a'), false, false, false, true)]
    #[case::super_ctrl_c("super-ctrl-c", KeyCodeValue::Char('c'), true, false, false, true)]
    #[case::super_g("super-G", KeyCodeValue::Char('G'), false, false, false, true)]
    #[case::f1("f1", KeyCodeValue::F(1), false, false, false, false)]
    #[case::f12_upper("F12", KeyCodeValue::F(12), false, false, false, false)]
    #[case::ctrl_f5("ctrl-f5", KeyCodeValue::F(5), true, false, false, false)]
    #[case::f24("f24", KeyCodeValue::F(24), false, false, false, false)]
    #[case::insert("insert", KeyCodeValue::Insert, false, false, false, false)]
    #[case::ins("ins", KeyCodeValue::Insert, false, false, false, false)]
    #[case::ctrl_insert("ctrl-insert", KeyCodeValue::Insert, true, false, false, false)]
    fn parse_single_key(
        #[case] input: &str,
        #[case] code: KeyCodeValue,
        #[case] ctrl: bool,
        #[case] alt: bool,
        #[case] shift: bool,
        #[case] super_key: bool,
    ) {
        let k = SingleKey::parse(input).unwrap();
        assert_eq!(k.code, code);
        assert_eq!(k.ctrl, ctrl);
        assert_eq!(k.alt, alt);
        assert_eq!(k.shift, shift);
        assert_eq!(k.super_key, super_key);
    }

    // (B) super aliases parse equal.
    #[rstest]
    fn super_aliases_parse_equal(#[values("super-a", "cmd-a", "win-a")] input: &str) {
        assert_eq!(SingleKey::parse(input).unwrap(), SingleKey::parse("super-a").unwrap());
    }

    // (C) parse errors.
    #[rstest]
    #[case("ctrl-alt-shift-xxx")]
    #[case("foobar-a")]
    #[case("f0")]
    #[case("f25")]
    fn parse_errors(#[case] input: &str) {
        assert!(SingleKey::parse(input).is_err());
    }

    // (D) from_event -> (code, flags).
    #[rstest]
    #[case::ctrl_c(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
        KeyCodeValue::Char('c'),
        true,
        false,
        false,
        false
    )]
    #[case::enter(
        KeyCode::Enter,
        KeyModifiers::NONE,
        KeyCodeValue::Enter,
        false,
        false,
        false,
        false
    )]
    #[case::upper_g(
        KeyCode::Char('G'),
        KeyModifiers::SHIFT,
        KeyCodeValue::Char('G'),
        false,
        false,
        false,
        false
    )]
    #[case::super_a(
        KeyCode::Char('a'),
        KeyModifiers::SUPER,
        KeyCodeValue::Char('a'),
        false,
        false,
        false,
        true
    )]
    #[case::f1(KeyCode::F(1), KeyModifiers::NONE, KeyCodeValue::F(1), false, false, false, false)]
    #[case::f12_ctrl(
        KeyCode::F(12),
        KeyModifiers::CONTROL,
        KeyCodeValue::F(12),
        true,
        false,
        false,
        false
    )]
    #[case::insert(
        KeyCode::Insert,
        KeyModifiers::NONE,
        KeyCodeValue::Insert,
        false,
        false,
        false,
        false
    )]
    #[case::backtab(
        KeyCode::BackTab,
        KeyModifiers::NONE,
        KeyCodeValue::Tab,
        false,
        false,
        true,
        false
    )]
    #[case::backtab_ctrl(
        KeyCode::BackTab,
        KeyModifiers::CONTROL,
        KeyCodeValue::Tab,
        true,
        false,
        true,
        false
    )]
    fn from_event_cases(
        #[case] code: KeyCode,
        #[case] mods: KeyModifiers,
        #[case] expected: KeyCodeValue,
        #[case] ctrl: bool,
        #[case] alt: bool,
        #[case] shift: bool,
        #[case] super_key: bool,
    ) {
        let event = KeyEvent::new(code, mods);
        let k = SingleKey::from_event(&event).unwrap();
        assert_eq!(k.code, expected);
        assert_eq!(k.ctrl, ctrl);
        assert_eq!(k.alt, alt);
        assert_eq!(k.shift, shift);
        assert_eq!(k.super_key, super_key);
    }

    // (E) from_event matches parsed.
    #[rstest]
    #[case(KeyCode::Char('c'), KeyModifiers::CONTROL, "ctrl-c")]
    #[case(KeyCode::Char('G'), KeyModifiers::SHIFT, "G")]
    #[case(KeyCode::Char('a'), KeyModifiers::SUPER, "super-a")]
    #[case(KeyCode::F(12), KeyModifiers::NONE, "f12")]
    #[case(KeyCode::BackTab, KeyModifiers::NONE, "shift-tab")]
    fn from_event_matches_parsed(
        #[case] code: KeyCode,
        #[case] mods: KeyModifiers,
        #[case] parsed: &str,
    ) {
        assert_eq!(
            SingleKey::from_event(&KeyEvent::new(code, mods)).unwrap(),
            SingleKey::parse(parsed).unwrap()
        );
    }

    // (F) display round-trip.
    #[rstest]
    fn display_round_trip(
        #[values(
            "ctrl-c", "alt-f", "enter", "G", "tab", "pageup", "f1", "f12", "ctrl-f5", "alt-f10",
            "insert", "super-a", "g g"
        )]
        s: &str,
    ) {
        let k = KeyInput::parse(s).unwrap();
        assert_eq!(k, KeyInput::parse(&k.to_string()).unwrap(), "round-trip failed for {s}");
    }

    // (G) exact display.
    #[rstest]
    #[case("g g", "g g")]
    #[case("super-a", "super-a")]
    #[case("super-ctrl-x", "super-ctrl-x")]
    #[case("f1", "f1")]
    #[case("ctrl-f12", "ctrl-f12")]
    #[case("insert", "insert")]
    fn display_exact(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(KeyInput::parse(input).unwrap().to_string(), expected);
    }

    #[rstest]
    fn parse_multi_key_sequence() {
        let ki = KeyInput::parse("g g").unwrap();
        match ki {
            KeyInput::Sequence(keys) => {
                assert_eq!(keys.len(), 2);
                assert_eq!(keys[0].code, KeyCodeValue::Char('g'));
                assert_eq!(keys[1].code, KeyCodeValue::Char('g'));
            }
            KeyInput::Single(_) => panic!("expected sequence"),
        }
    }
}
