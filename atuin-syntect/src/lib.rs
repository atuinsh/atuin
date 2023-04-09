use once_cell::sync::OnceCell;
use syntect::{
    dumps::from_uncompressed_data,
    parsing::{
        BasicScopeStackOp, ParseScopeError, Scope, ScopeStack, ScopeStackOp, SyntaxReference,
        SyntaxSet,
    },
};

mod style;
pub use style::*;

impl Theme {
    // this is a manual/simpler implementation of
    // syntect::highlight::HighlightIterator
    // to use a custom theme using `ratatui::Style`.
    // This is so we don't have to care about RGB and can instead use
    // terminal colours
    pub fn highlight(&self, h: &str, parsed: &ParsedSyntax, draw: &mut dyn FnMut(&str, Style)) {
        let mut stack = ScopeStack::default();
        let mut styles: Vec<(f64, Style)> = vec![];
        for (line, parsed_line) in h.lines().zip(parsed) {
            draw("", Style::default());

            let mut last = 0;
            for &(index, ref op) in parsed_line {
                let style = styles.last().copied().unwrap_or_default().1;
                stack
                    .apply_with_hook(op, |op, stack| {
                        highlight_hook(&op, stack, &self.rules, &mut styles);
                    })
                    .unwrap();

                draw(&line[last..index], style);
                last = index;
            }
            let style = styles.last().copied().unwrap_or_default().1;
            draw(&line[last..], style);
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn highlight_hook(
    op: &BasicScopeStackOp,
    stack: &[Scope],
    rules: &[ThemeRule],
    styles: &mut Vec<(f64, Style)>,
) {
    match op {
        BasicScopeStackOp::Push(scope) => {
            let mut scored_style = styles
                .last()
                .copied()
                .unwrap_or_else(|| (-1.0, Style::default()));

            for rule in rules.iter().filter(|a| a.scope.is_prefix_of(*scope)) {
                let single_score =
                    f64::from(rule.scope.len()) * f64::from(3 * ((stack.len() - 1) as u32)).exp2();

                if single_score > scored_style.0 {
                    scored_style.0 = single_score;
                    scored_style.1 = rule.style;
                }
            }

            styles.push(scored_style);
        }
        BasicScopeStackOp::Pop => {
            styles.pop();
        }
    }
}

pub fn get_syntax() -> ShellSyntax<'static> {
    static SYNTAX: OnceCell<SyntaxSet> = OnceCell::new();
    let syntax = SYNTAX.get_or_init(|| {
        from_uncompressed_data(include_bytes!("default_nonewlines.packdump")).unwrap()
    });
    ShellSyntax {
        syntax,
        sh: syntax.find_syntax_by_extension("sh").unwrap(),
        fish: syntax.find_syntax_by_extension("fish").unwrap(),
        nu: syntax.find_syntax_by_extension("nu").unwrap(),
    }
}

#[derive(Clone, Copy)]
pub struct ShellSyntax<'s> {
    syntax: &'s SyntaxSet,
    sh: &'s SyntaxReference,
    fish: &'s SyntaxReference,
    nu: &'s SyntaxReference,
}
pub type ParsedSyntax = Vec<Vec<(usize, ScopeStackOp)>>;

impl ShellSyntax<'_> {
    pub fn parse_shell(self, h: &str) -> ParsedSyntax {
        let mut sh = syntect::parsing::ParseState::new(self.sh);
        let mut fish = syntect::parsing::ParseState::new(self.fish);
        let mut nu = syntect::parsing::ParseState::new(self.nu);

        let mut lines = vec![];
        for line in h.lines() {
            if let Ok(line) = sh.parse_line(line, self.syntax) {
                lines.push(line);
            } else if let Ok(line) = fish.parse_line(line, self.syntax) {
                lines.push(line);
            } else if let Ok(line) = nu.parse_line(line, self.syntax) {
                lines.push(line);
            } else {
                lines.push(Vec::new());
            }
        }
        lines
    }
}

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
