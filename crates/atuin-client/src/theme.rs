use strum_macros;
use std::path::PathBuf;
use std::io::BufReader;
use std::fs::File;
use eyre::Result;
use std::collections::HashMap;
use palette::named;
use serde::{Serialize, Deserialize};
use lazy_static::lazy_static;

#[derive(Serialize, Deserialize, Copy, Clone, Hash, Debug, Eq, PartialEq, strum_macros::Display)]
#[strum(serialize_all = "camel_case")]
pub enum Level {
    Info,
    Warning,
    Error,
}

#[derive(Serialize, Deserialize, Copy, Clone, Hash, Debug, Eq, PartialEq, strum_macros::Display)]
#[strum(serialize_all = "camel_case")]
pub enum Meaning {
    AlertInfo,
    AlertWarning,
    AlertError,
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
        self.colors[ALERT_TYPES.get(&severity).unwrap()]
    }

    pub fn new(colors: HashMap::<Meaning, Color>) -> Theme {
        Theme { colors }
    }

    pub fn as_style(&self, meaning: Meaning) -> ContentStyle {
        let mut style = ContentStyle::default();
        style.foreground_color = Some(self.colors[&meaning]);
        style
    }

    pub fn from_named(colors: HashMap::<Meaning, String>) -> Theme {
        let colors: HashMap::<Meaning, Color> =
            colors.iter().map(|(name, color)| { (*name, from_named(color)) }).collect();
        make_theme(&colors)
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

fn make_theme(overrides: &HashMap<Meaning, Color>) -> Theme {
    let colors = HashMap::from([
        (Meaning::AlertError, Color::Red),
        (Meaning::AlertWarning, Color::Yellow),
        (Meaning::AlertInfo, Color::Green),
        (Meaning::Annotation, Color::DarkGrey),
        (Meaning::Guidance, Color::Blue),
        (Meaning::Important, Color::White),
        (Meaning::Base, Color::Grey),
    ]).iter().map(|(name, color)| {
        match overrides.get(name) {
            Some(value) => (*name, *value),
            None => (*name, *color)
        }
    }).collect();
    Theme::new(colors)
}

lazy_static! {
    static ref ALERT_TYPES: HashMap<Level, Meaning> = {
        HashMap::from([
            (Level::Info, Meaning::AlertInfo),
            (Level::Warning, Meaning::AlertWarning),
            (Level::Error, Meaning::AlertError),
        ])
    };

    static ref BUILTIN_THEMES: HashMap<&'static str, Theme> = {
        HashMap::from([
            ("", HashMap::new()),
            ("autumn", HashMap::from([
                (Meaning::AlertError, from_named("saddlebrown")),
                (Meaning::AlertWarning, from_named("darkorange")),
                (Meaning::AlertInfo, from_named("gold")),
                (Meaning::Annotation, Color::DarkGrey),
                (Meaning::Guidance, from_named("brown")),
            ])),
            ("marine", HashMap::from([
                (Meaning::AlertError, from_named("seagreen")),
                (Meaning::AlertWarning, from_named("turquoise")),
                (Meaning::AlertInfo, from_named("cyan")),
                (Meaning::Annotation, from_named("midnightblue")),
                (Meaning::Guidance, from_named("teal")),
            ]))
        ]).iter().map(|(name, theme)| (*name, make_theme(theme))).collect()
    };
}

pub struct ThemeManager {
    loaded_themes: HashMap::<String, Theme>
}

impl ThemeManager {
    pub fn new() -> Self {
        Self { loaded_themes: HashMap::new() }
    }

    pub fn load_theme_from_file(&mut self, name: &str) -> Result<&Theme> {
        let mut theme_file = if let Ok(p) = std::env::var("ATUIN_THEME_DIR") {
            PathBuf::from(p)
        } else {
            let config_dir = atuin_common::utils::config_dir();
            let mut theme_file = PathBuf::new();
            theme_file.push(config_dir);
            theme_file.push("themes");
            theme_file
        };

        let theme_yaml = format!["{}.yaml", name];
        theme_file.push(theme_yaml);

        let file = File::open(theme_file.as_path())?;
        let reader: BufReader<File> = BufReader::new(file);
        let colors: HashMap<Meaning, String> = serde_json::from_reader(reader)?;
        let theme = Theme::from_named(colors);
        let name = name.to_string();
        self.loaded_themes.insert(name.clone(), theme);
        let theme = self.loaded_themes.get(&name).unwrap();
        Ok(theme)
    }

    pub fn load_theme(&mut self, name: &str) -> &Theme {
        if self.loaded_themes.contains_key(name) {
            return self.loaded_themes.get(name).unwrap();
        }
        let built_ins = &BUILTIN_THEMES;
        match built_ins.get(name) {
            Some(theme) => theme,
            None => match self.load_theme_from_file(name) {
                Ok(theme) => theme,
                Err(err) => {
                    print!["Could not load theme {}: {}", name, err];
                    built_ins.get("").unwrap()
                }
            }
        }
    }
}
