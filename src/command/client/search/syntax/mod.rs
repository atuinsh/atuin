use syntect::parsing::{ParseScopeError, Scope};

use crate::ratatui::style::{Color, Style};

pub struct Theme {
    pub rules: Vec<ThemeRule>,
    pub selection: Color,
}

pub struct ThemeRule {
    pub scope: Scope,
    pub style: Style,
}

// blame syntax highlighting
#[allow(clippy::too_many_lines)]
pub fn get_theme() -> Result<Theme, ParseScopeError> {
    let rules = vec![
        ThemeRule {
            scope: Scope::new("variable.parameter.function")?,
            style: Style::default(),
        },
        ThemeRule {
            scope: Scope::new("comment")?,
            style: Style::default().fg(Color::Gray),
        },
        ThemeRule {
            scope: Scope::new("punctuation.definition.comment")?,
            style: Style::default().fg(Color::Gray),
        },
        ThemeRule {
            scope: Scope::new("punctuation.definition.string")?,
            style: Style::default(),
        },
        ThemeRule {
            scope: Scope::new("punctuation.definition.variable")?,
            style: Style::default(),
        },
        ThemeRule {
            scope: Scope::new("punctuation.definition.parameters")?,
            style: Style::default(),
        },
        ThemeRule {
            scope: Scope::new("punctuation.definition.array")?,
            style: Style::default(),
        },
        ThemeRule {
            scope: Scope::new("none")?,
            style: Style::default(),
        },
        ThemeRule {
            scope: Scope::new("keyword.operator")?,
            style: Style::default(),
        },
        ThemeRule {
            scope: Scope::new("keyword")?,
            style: Style::default().fg(Color::Magenta),
        },
        ThemeRule {
            scope: Scope::new("variable")?,
            style: Style::default().fg(Color::Red),
        },
        ThemeRule {
            scope: Scope::new("entity.name.function")?,
            style: Style::default().fg(Color::LightBlue),
        },
        ThemeRule {
            scope: Scope::new("meta.require")?,
            style: Style::default().fg(Color::LightBlue),
        },
        ThemeRule {
            scope: Scope::new("support.function.any-method")?,
            style: Style::default().fg(Color::LightBlue),
        },
        ThemeRule {
            scope: Scope::new("variable.function")?,
            style: Style::default().fg(Color::LightBlue),
        },
        ThemeRule {
            scope: Scope::new("support.class")?,
            style: Style::default().fg(Color::Yellow),
        },
        ThemeRule {
            scope: Scope::new("entity.name.class")?,
            style: Style::default().fg(Color::Yellow),
        },
        ThemeRule {
            scope: Scope::new("entity.name.type.class")?,
            style: Style::default().fg(Color::Yellow),
        },
        ThemeRule {
            scope: Scope::new("meta.class")?,
            style: Style::default().fg(Color::White),
        },
        ThemeRule {
            scope: Scope::new("keyword.other.special-method")?,
            style: Style::default().fg(Color::LightBlue),
        },
        ThemeRule {
            scope: Scope::new("storage")?,
            style: Style::default().fg(Color::Magenta),
        },
        ThemeRule {
            scope: Scope::new("support.function")?,
            style: Style::default().fg(Color::Gray),
        },
        ThemeRule {
            scope: Scope::new("string")?,
            style: Style::default().fg(Color::Green),
        },
        ThemeRule {
            scope: Scope::new("constant.other.symbol")?,
            style: Style::default().fg(Color::Green),
        },
        ThemeRule {
            scope: Scope::new("entity.other.inherited-class")?,
            style: Style::default().fg(Color::Green),
        },
        ThemeRule {
            scope: Scope::new("constant.numeric")?,
            style: Style::default().fg(Color::LightRed),
        },
        ThemeRule {
            scope: Scope::new("none")?,
            style: Style::default().fg(Color::LightRed),
        },
        ThemeRule {
            scope: Scope::new("constant")?,
            style: Style::default().fg(Color::LightRed),
        },
        ThemeRule {
            scope: Scope::new("entity.name.tag")?,
            style: Style::default().fg(Color::Red),
        },
        ThemeRule {
            scope: Scope::new("entity.other.attribute-name")?,
            style: Style::default().fg(Color::LightRed),
        },
        ThemeRule {
            scope: Scope::new("entity.other.attribute-name.id")?,
            style: Style::default().fg(Color::LightBlue),
        },
        ThemeRule {
            scope: Scope::new("punctuation.definition.entity")?,
            style: Style::default().fg(Color::LightBlue),
        },
        ThemeRule {
            scope: Scope::new("meta.selector")?,
            style: Style::default().fg(Color::Magenta),
        },
        ThemeRule {
            scope: Scope::new("none")?,
            style: Style::default().fg(Color::LightRed),
        },
        ThemeRule {
            scope: Scope::new("markup.heading")?,
            style: Style::default().fg(Color::LightBlue),
        },
        ThemeRule {
            scope: Scope::new("punctuation.definition.heading")?,
            style: Style::default().fg(Color::LightBlue),
        },
        ThemeRule {
            scope: Scope::new("entity.name.section")?,
            style: Style::default().fg(Color::LightBlue),
        },
        ThemeRule {
            scope: Scope::new("keyword.other.unit")?,
            style: Style::default().fg(Color::LightRed),
        },
        ThemeRule {
            scope: Scope::new("constant.other.color")?,
            style: Style::default().fg(Color::Gray),
        },
        ThemeRule {
            scope: Scope::new("string.regexp")?,
            style: Style::default().fg(Color::Gray),
        },
        ThemeRule {
            scope: Scope::new("constant.character.escape")?,
            style: Style::default().fg(Color::Gray),
        },
        ThemeRule {
            scope: Scope::new("punctuation.section.embedded")?,
            style: Style::default().fg(Color::Blue),
        },
        ThemeRule {
            scope: Scope::new("variable.interpolation")?,
            style: Style::default().fg(Color::Blue),
        },
        ThemeRule {
            scope: Scope::new("invalid.illegal")?,
            style: Style::default().fg(Color::Black).bg(Color::Red),
        },
    ];
    Ok(Theme {
        rules,
        selection: Color::DarkGray,
    })
}
