use strum_macros;
use std::collections::HashMap;
use palette::named;
use lazy_static::lazy_static;

#[derive(Copy, Clone, Hash, Debug, Eq, PartialEq, strum_macros::Display)]
#[strum(serialize_all = "snake_case")]
pub enum Level {
    Info,
    Warning,
    Error,
}

#[derive(Copy, Clone, Hash, Debug, Eq, PartialEq, strum_macros::Display)]
#[strum(serialize_all = "snake_case")]
pub enum Meaning {
    Alert {
        severity: Level,
    },
    Annotation,
    Base,
    Guidance,
    Important,
}

use crossterm::{
    style::{Color, ContentStyle},
};

pub struct Theme {
    pub colors: HashMap::<Meaning, Color>
}

impl Theme {
    pub fn get_base(&self) -> Color {
        self.colors[&Meaning::Base]
    }

    pub fn get_info(&self) -> Color {
        self.get_alert(Level::Info)
    }

    pub fn get_warning(&self) -> Color {
        self.get_alert(Level::Warning)
    }

    pub fn get_error(&self) -> Color {
        self.get_alert(Level::Error)
    }

    pub fn get_alert(&self, severity: Level) -> Color {
        self.colors[&Meaning::Alert { severity: severity }]
    }

    pub fn new(colors: HashMap::<Meaning, Color>) -> Theme {
        Theme { colors }
    }

    pub fn as_style(&self, meaning: Meaning) -> ContentStyle {
        let mut style = ContentStyle::default();
        style.foreground_color = Some(self.colors[&meaning]);
        style
    }
}

fn from_named(name: &str) -> Color {
    let srgb = named::from_str(name).unwrap();
    Color::Rgb {
        r: srgb.red,
        g: srgb.green,
        b: srgb.blue,
    }
}

lazy_static! {
    static ref BUILTIN_THEMES: HashMap<&'static str, HashMap<Meaning, Color>> = {
        HashMap::from([
            ("autumn", HashMap::from([
                (Meaning::Alert { severity: Level::Error }, from_named("saddlebrown")),
                (Meaning::Alert { severity: Level::Warning }, from_named("darkorange")),
                (Meaning::Alert { severity: Level::Info }, from_named("gold")),
                (Meaning::Annotation, Color::DarkGrey),
                (Meaning::Guidance, from_named("khaki")),
            ]))
        ])
    };
}

pub fn load_theme(name: &str) -> Theme {
    let mut default_theme = HashMap::from([
        (Meaning::Alert { severity: Level::Error }, Color::Red),
        (Meaning::Alert { severity: Level::Warning }, Color::Yellow),
        (Meaning::Alert { severity: Level::Info }, Color::Green),
        (Meaning::Annotation, Color::DarkGrey),
        (Meaning::Guidance, Color::Blue),
        (Meaning::Important, Color::White),
        (Meaning::Base, Color::Grey),
    ]);
    let built_ins = &BUILTIN_THEMES;
    let theme = match built_ins.get(name) {
        Some(theme) => {
            theme.iter().for_each(|(k, v)| {
                default_theme.insert(*k, *v);
            });
            default_theme
        },
        None => default_theme
    };
    Theme::new(theme)
}
