use strum_macros;
use std::collections::HashMap;

#[derive(Hash, Debug, Eq, PartialEq, strum_macros::Display)]
#[strum(serialize_all = "snake_case")]
pub enum Meaning {
    Alert {
        severity: usize,
    },
    Annotation,
    Base,
    Guidance,
    Important,
}

use ratatui::{
    style::{Color}
};

pub struct Theme {
    colors: HashMap::<Meaning, Color>
}

impl Theme {
    pub fn new(colors: HashMap::<Meaning, Color>) -> Theme {
        Theme { colors }
    }
}

pub fn load_theme(_name: String) -> Theme {
    let default_theme = HashMap::from([
        (Meaning::Alert { severity: 3 }, Color::Red),
        (Meaning::Alert { severity: 2 }, Color::Yellow),
        (Meaning::Alert { severity: 1 }, Color::Green),
        (Meaning::Annotation, Color::DarkGray),
        (Meaning::Guidance, Color::Blue),
        (Meaning::Important, Color::White),
        (Meaning::Base, Color::Gray),
    ]);
    Theme::new(default_theme)
}
