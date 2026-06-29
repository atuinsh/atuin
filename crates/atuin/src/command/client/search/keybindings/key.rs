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
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn parse_simple_keys() {
        let k = SingleKey::parse("a").unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('a'));
        assert!(!k.ctrl && !k.alt && !k.shift);

        let k = SingleKey::parse("enter").unwrap();
        assert_eq!(k.code, KeyCodeValue::Enter);

        let k = SingleKey::parse("esc").unwrap();
        assert_eq!(k.code, KeyCodeValue::Esc);

        let k = SingleKey::parse("tab").unwrap();
        assert_eq!(k.code, KeyCodeValue::Tab);

        let k = SingleKey::parse("space").unwrap();
        assert_eq!(k.code, KeyCodeValue::Space);
    }

    #[test]
    fn parse_modifiers() {
        let k = SingleKey::parse("ctrl-c").unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('c'));
        assert!(k.ctrl);
        assert!(!k.alt);

        let k = SingleKey::parse("alt-f").unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('f'));
        assert!(k.alt);
        assert!(!k.ctrl);

        let k = SingleKey::parse("ctrl-alt-x").unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('x'));
        assert!(k.ctrl && k.alt);
    }

    #[test]
    fn parse_uppercase_implies_no_shift_flag() {
        let k = SingleKey::parse("G").unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('G'));
        assert!(!k.shift);
        assert!(!k.ctrl);
    }

    #[test]
    fn parse_special_chars() {
        let k = SingleKey::parse("ctrl-[").unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('['));
        assert!(k.ctrl);

        let k = SingleKey::parse("?").unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('?'));

        let k = SingleKey::parse("/").unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('/'));
    }

    #[test]
    fn parse_multi_key_sequence() {
        let ki = KeyInput::parse("g g").unwrap();
        match ki {
            KeyInput::Sequence(keys) => {
                assert_eq!(keys.len(), 2);
                assert_eq!(keys[0].code, KeyCodeValue::Char('g'));
                assert_eq!(keys[1].code, KeyCodeValue::Char('g'));
            }
            _ => panic!("expected sequence"),
        }
    }

    #[test]
    fn display_round_trip() {
        let cases = ["ctrl-c", "alt-f", "enter", "G", "tab", "pageup"];
        for s in cases {
            let k = KeyInput::parse(s).unwrap();
            let display = k.to_string();
            let k2 = KeyInput::parse(&display).unwrap();
            assert_eq!(k, k2, "round-trip failed for {s}");
        }

        let ki = KeyInput::parse("g g").unwrap();
        assert_eq!(ki.to_string(), "g g");
    }

    #[test]
    fn from_event_basic() {
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let k = SingleKey::from_event(&event).unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('c'));
        assert!(k.ctrl);
        assert!(!k.alt);

        let event = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let k = SingleKey::from_event(&event).unwrap();
        assert_eq!(k.code, KeyCodeValue::Enter);
    }

    #[test]
    fn from_event_uppercase() {
        // Crossterm sends uppercase chars with SHIFT modifier
        let event = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        let k = SingleKey::from_event(&event).unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('G'));
        // shift flag should be cleared since the case encodes it
        assert!(!k.shift);
    }

    #[test]
    fn from_event_matches_parsed() {
        // Verify that from_event and parse produce the same SingleKey
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let from_event = SingleKey::from_event(&event).unwrap();
        let parsed = SingleKey::parse("ctrl-c").unwrap();
        assert_eq!(from_event, parsed);

        let event = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        let from_event = SingleKey::from_event(&event).unwrap();
        let parsed = SingleKey::parse("G").unwrap();
        assert_eq!(from_event, parsed);
    }

    #[test]
    fn parse_super_modifier() {
        let k = SingleKey::parse("super-a").unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('a'));
        assert!(k.super_key);
        assert!(!k.ctrl && !k.alt && !k.shift);

        // "cmd" is an alias for "super"
        let k2 = SingleKey::parse("cmd-a").unwrap();
        assert_eq!(k, k2);

        // "win" is an alias for "super"
        let k3 = SingleKey::parse("win-a").unwrap();
        assert_eq!(k, k3);
    }

    #[test]
    fn parse_super_with_other_modifiers() {
        let k = SingleKey::parse("super-ctrl-c").unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('c'));
        assert!(k.super_key && k.ctrl);
        assert!(!k.alt && !k.shift);
    }

    #[test]
    fn display_super_modifier() {
        let k = SingleKey::parse("super-a").unwrap();
        assert_eq!(k.to_string(), "super-a");

        let k = SingleKey::parse("super-ctrl-x").unwrap();
        assert_eq!(k.to_string(), "super-ctrl-x");
    }

    #[test]
    fn display_round_trip_super() {
        let k = KeyInput::parse("super-a").unwrap();
        let display = k.to_string();
        let k2 = KeyInput::parse(&display).unwrap();
        assert_eq!(k, k2, "round-trip failed for super-a");
    }

    #[test]
    fn from_event_super() {
        let event = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SUPER);
        let k = SingleKey::from_event(&event).unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('a'));
        assert!(k.super_key);
        assert!(!k.ctrl && !k.alt && !k.shift);
    }

    #[test]
    fn from_event_super_matches_parsed() {
        let event = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SUPER);
        let from_event = SingleKey::from_event(&event).unwrap();
        let parsed = SingleKey::parse("super-a").unwrap();
        assert_eq!(from_event, parsed);
    }

    #[test]
    fn super_uppercase_preserves_super() {
        // super-G should keep the super flag (unlike bare "G" which clears shift)
        let k = SingleKey::parse("super-G").unwrap();
        assert_eq!(k.code, KeyCodeValue::Char('G'));
        assert!(k.super_key);
    }

    #[test]
    fn parse_errors() {
        assert!(SingleKey::parse("ctrl-alt-shift-xxx").is_err());
        assert!(SingleKey::parse("foobar-a").is_err());
    }

    #[test]
    fn parse_function_keys() {
        let k = SingleKey::parse("f1").unwrap();
        assert_eq!(k.code, KeyCodeValue::F(1));
        assert!(!k.ctrl && !k.alt && !k.shift);

        let k = SingleKey::parse("F12").unwrap();
        assert_eq!(k.code, KeyCodeValue::F(12));

        let k = SingleKey::parse("ctrl-f5").unwrap();
        assert_eq!(k.code, KeyCodeValue::F(5));
        assert!(k.ctrl);

        // F24 is valid (some keyboards have extended function keys)
        let k = SingleKey::parse("f24").unwrap();
        assert_eq!(k.code, KeyCodeValue::F(24));

        // F0 and F25+ are invalid
        assert!(SingleKey::parse("f0").is_err());
        assert!(SingleKey::parse("f25").is_err());
    }

    #[test]
    fn from_event_function_keys() {
        let event = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE);
        let k = SingleKey::from_event(&event).unwrap();
        assert_eq!(k.code, KeyCodeValue::F(1));

        let event = KeyEvent::new(KeyCode::F(12), KeyModifiers::CONTROL);
        let k = SingleKey::from_event(&event).unwrap();
        assert_eq!(k.code, KeyCodeValue::F(12));
        assert!(k.ctrl);
    }

    #[test]
    fn display_function_keys() {
        let k = SingleKey::parse("f1").unwrap();
        assert_eq!(k.to_string(), "f1");

        let k = SingleKey::parse("ctrl-f12").unwrap();
        assert_eq!(k.to_string(), "ctrl-f12");
    }

    #[test]
    fn function_key_round_trip() {
        let cases = ["f1", "f12", "ctrl-f5", "alt-f10"];
        for s in cases {
            let k = KeyInput::parse(s).unwrap();
            let display = k.to_string();
            let k2 = KeyInput::parse(&display).unwrap();
            assert_eq!(k, k2, "round-trip failed for {s}");
        }
    }

    #[test]
    fn from_event_function_key_matches_parsed() {
        let event = KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE);
        let from_event = SingleKey::from_event(&event).unwrap();
        let parsed = SingleKey::parse("f12").unwrap();
        assert_eq!(from_event, parsed);
    }

    #[test]
    fn from_event_backtab_becomes_shift_tab() {
        // Many terminals send BackTab for Shift+Tab
        let event = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        let k = SingleKey::from_event(&event).unwrap();
        assert_eq!(k.code, KeyCodeValue::Tab);
        assert!(k.shift);
        assert!(!k.ctrl && !k.alt);
    }

    #[test]
    fn from_event_backtab_matches_parsed_shift_tab() {
        let event = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        let from_event = SingleKey::from_event(&event).unwrap();
        let parsed = SingleKey::parse("shift-tab").unwrap();
        assert_eq!(from_event, parsed);
    }

    #[test]
    fn from_event_backtab_with_ctrl() {
        // BackTab with ctrl modifier
        let event = KeyEvent::new(KeyCode::BackTab, KeyModifiers::CONTROL);
        let k = SingleKey::from_event(&event).unwrap();
        assert_eq!(k.code, KeyCodeValue::Tab);
        assert!(k.shift);
        assert!(k.ctrl);
    }

    #[test]
    fn parse_insert_key() {
        let k = SingleKey::parse("insert").unwrap();
        assert_eq!(k.code, KeyCodeValue::Insert);
        assert!(!k.ctrl && !k.alt && !k.shift);

        let k = SingleKey::parse("ins").unwrap();
        assert_eq!(k.code, KeyCodeValue::Insert);

        let k = SingleKey::parse("ctrl-insert").unwrap();
        assert_eq!(k.code, KeyCodeValue::Insert);
        assert!(k.ctrl);
    }

    #[test]
    fn from_event_insert_key() {
        let event = KeyEvent::new(KeyCode::Insert, KeyModifiers::NONE);
        let k = SingleKey::from_event(&event).unwrap();
        assert_eq!(k.code, KeyCodeValue::Insert);
    }

    #[test]
    fn insert_key_round_trip() {
        let k = KeyInput::parse("insert").unwrap();
        let display = k.to_string();
        assert_eq!(display, "insert");
        let k2 = KeyInput::parse(&display).unwrap();
        assert_eq!(k, k2);
    }
}
