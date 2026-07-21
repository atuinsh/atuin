use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Atomic (leaf) conditions that can be evaluated against state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionAtom {
    CursorAtStart,
    CursorAtEnd,
    InputEmpty,
    OriginalInputEmpty,
    ListAtEnd,
    ListAtStart,
    NoResults,
    HasResults,
    HasContext,
}

/// Boolean expression tree over condition atoms.
///
/// Supports negation, conjunction, and disjunction with standard precedence:
/// `!` binds tightest, then `&&`, then `||`.
///
/// Examples of valid expression strings:
/// - `"cursor-at-start"` (bare atom)
/// - `"!no-results"` (negation)
/// - `"cursor-at-start && input-empty"` (conjunction)
/// - `"list-at-start || no-results"` (disjunction)
/// - `"(cursor-at-start && !input-empty) || no-results"` (grouping)
#[derive(Debug, Clone, PartialEq, Eq, derive_more::From)]
pub enum ConditionExpr {
    #[from]
    Atom(ConditionAtom),
    #[from(skip)]
    Not(Box<ConditionExpr>),
    And(Box<ConditionExpr>, Box<ConditionExpr>),
    Or(Box<ConditionExpr>, Box<ConditionExpr>),
}

/// Context needed to evaluate conditions. This is a pure snapshot of state —
/// no references to mutable data.
pub struct EvalContext {
    /// Current cursor position (unicode width units).
    pub cursor_position: usize,
    /// Width of the input string in unicode width units.
    pub input_width: usize,
    /// Byte length of the input string.
    pub input_byte_len: usize,
    /// Currently selected index in the results list.
    pub selected_index: usize,
    /// Total number of results.
    pub results_len: usize,
    /// Whether the original input (query passed to the TUI) was empty.
    pub original_input_empty: bool,
    /// Whether we use a search context of a command from the history.
    pub has_context: bool,
}

// ---------------------------------------------------------------------------
// ConditionAtom
// ---------------------------------------------------------------------------

impl ConditionAtom {
    /// Evaluate this atom against the given context.
    pub fn evaluate(&self, ctx: &EvalContext) -> bool {
        match self {
            ConditionAtom::CursorAtStart => ctx.cursor_position == 0,
            ConditionAtom::CursorAtEnd => ctx.cursor_position == ctx.input_width,
            ConditionAtom::InputEmpty => ctx.input_byte_len == 0,
            ConditionAtom::OriginalInputEmpty => ctx.original_input_empty,
            ConditionAtom::ListAtEnd => {
                ctx.results_len == 0 || ctx.selected_index >= ctx.results_len.saturating_sub(1)
            }
            ConditionAtom::ListAtStart => ctx.results_len == 0 || ctx.selected_index == 0,
            ConditionAtom::NoResults => ctx.results_len == 0,
            ConditionAtom::HasResults => ctx.results_len > 0,
            ConditionAtom::HasContext => ctx.has_context,
        }
    }

    /// Parse from a kebab-case string.
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "cursor-at-start" => Ok(ConditionAtom::CursorAtStart),
            "cursor-at-end" => Ok(ConditionAtom::CursorAtEnd),
            "input-empty" => Ok(ConditionAtom::InputEmpty),
            "original-input-empty" => Ok(ConditionAtom::OriginalInputEmpty),
            "list-at-end" => Ok(ConditionAtom::ListAtEnd),
            "list-at-start" => Ok(ConditionAtom::ListAtStart),
            "no-results" => Ok(ConditionAtom::NoResults),
            "has-results" => Ok(ConditionAtom::HasResults),
            "has-context" => Ok(ConditionAtom::HasContext),
            _ => Err(format!("unknown condition: {s}")),
        }
    }

    /// Convert to a kebab-case string.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConditionAtom::CursorAtStart => "cursor-at-start",
            ConditionAtom::CursorAtEnd => "cursor-at-end",
            ConditionAtom::InputEmpty => "input-empty",
            ConditionAtom::OriginalInputEmpty => "original-input-empty",
            ConditionAtom::ListAtEnd => "list-at-end",
            ConditionAtom::ListAtStart => "list-at-start",
            ConditionAtom::NoResults => "no-results",
            ConditionAtom::HasResults => "has-results",
            ConditionAtom::HasContext => "has-context",
        }
    }
}

impl fmt::Display for ConditionAtom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// ConditionExpr — evaluation
// ---------------------------------------------------------------------------

impl ConditionExpr {
    /// Evaluate this expression against the given context.
    pub fn evaluate(&self, ctx: &EvalContext) -> bool {
        match self {
            ConditionExpr::Atom(atom) => atom.evaluate(ctx),
            ConditionExpr::Not(inner) => !inner.evaluate(ctx),
            ConditionExpr::And(lhs, rhs) => lhs.evaluate(ctx) && rhs.evaluate(ctx),
            ConditionExpr::Or(lhs, rhs) => lhs.evaluate(ctx) || rhs.evaluate(ctx),
        }
    }
}

// ---------------------------------------------------------------------------
// ConditionExpr — ergonomic builders
// ---------------------------------------------------------------------------

#[allow(dead_code)]
impl ConditionExpr {
    /// Negate this expression: `!self`.
    pub fn not(self) -> Self {
        ConditionExpr::Not(Box::new(self))
    }

    /// Conjoin with another expression: `self && other`.
    pub fn and(self, other: ConditionExpr) -> Self {
        ConditionExpr::And(Box::new(self), Box::new(other))
    }

    /// Disjoin with another expression: `self || other`.
    pub fn or(self, other: ConditionExpr) -> Self {
        ConditionExpr::Or(Box::new(self), Box::new(other))
    }
}

// ---------------------------------------------------------------------------
// ConditionExpr — parser
// ---------------------------------------------------------------------------

/// Recursive descent parser for boolean condition expressions.
///
/// Grammar (standard boolean precedence):
/// ```text
/// expr     = or_expr
/// or_expr  = and_expr ("||" and_expr)*
/// and_expr = unary ("&&" unary)*
/// unary    = "!" unary | primary
/// primary  = atom | "(" expr ")"
/// atom     = [a-z][a-z0-9-]*
/// ```
struct ExprParser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> ExprParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() && self.input.as_bytes()[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn starts_with(&mut self, s: &str) -> bool {
        self.skip_whitespace();
        self.input[self.pos..].starts_with(s)
    }

    fn consume(&mut self, s: &str) -> bool {
        self.skip_whitespace();
        if self.input[self.pos..].starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    /// Parse a full expression, expecting to consume all input.
    fn parse(mut self) -> Result<ConditionExpr, String> {
        let expr = self.parse_or()?;
        self.skip_whitespace();
        if self.pos < self.input.len() {
            return Err(format!(
                "unexpected input at position {}: {:?}",
                self.pos,
                &self.input[self.pos..]
            ));
        }
        Ok(expr)
    }

    /// `or_expr` = `and_expr` ("||" `and_expr`)*
    fn parse_or(&mut self) -> Result<ConditionExpr, String> {
        let mut left = self.parse_and()?;
        while self.starts_with("||") {
            self.consume("||");
            let right = self.parse_and()?;
            left = ConditionExpr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// `and_expr` = unary ("&&" unary)*
    fn parse_and(&mut self) -> Result<ConditionExpr, String> {
        let mut left = self.parse_unary()?;
        while self.starts_with("&&") {
            self.consume("&&");
            let right = self.parse_unary()?;
            left = ConditionExpr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// unary = "!" unary | primary
    fn parse_unary(&mut self) -> Result<ConditionExpr, String> {
        if self.consume("!") {
            let inner = self.parse_unary()?;
            Ok(ConditionExpr::Not(Box::new(inner)))
        } else {
            self.parse_primary()
        }
    }

    /// primary = "(" expr ")" | atom
    fn parse_primary(&mut self) -> Result<ConditionExpr, String> {
        if self.consume("(") {
            let expr = self.parse_or()?;
            if !self.consume(")") {
                return Err(format!("expected ')' at position {}", self.pos));
            }
            Ok(expr)
        } else {
            self.parse_atom()
        }
    }

    /// atom = `[a-z][a-z0-9-]*`
    fn parse_atom(&mut self) -> Result<ConditionExpr, String> {
        self.skip_whitespace();
        let start = self.pos;
        while self.pos < self.input.len() {
            let b = self.input.as_bytes()[self.pos];
            if b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(format!("expected condition name at position {}", self.pos));
        }
        let name = &self.input[start..self.pos];
        let atom = ConditionAtom::from_str(name)?;
        Ok(ConditionExpr::Atom(atom))
    }
}

impl ConditionExpr {
    /// Parse a condition expression from a string.
    pub fn parse(s: &str) -> Result<Self, String> {
        let parser = ExprParser::new(s);
        parser.parse()
    }
}

// ---------------------------------------------------------------------------
// ConditionExpr — Display
// ---------------------------------------------------------------------------

/// Precedence levels for minimal-parentheses display.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Prec {
    Or = 0,
    And = 1,
    Not = 2,
    Atom = 3,
}

impl ConditionExpr {
    fn prec(&self) -> Prec {
        match self {
            ConditionExpr::Or(..) => Prec::Or,
            ConditionExpr::And(..) => Prec::And,
            ConditionExpr::Not(..) => Prec::Not,
            ConditionExpr::Atom(..) => Prec::Atom,
        }
    }

    fn fmt_with_prec(&self, f: &mut fmt::Formatter<'_>, parent_prec: Prec) -> fmt::Result {
        let needs_parens = self.prec() < parent_prec;
        if needs_parens {
            write!(f, "(")?;
        }
        match self {
            ConditionExpr::Atom(atom) => write!(f, "{atom}")?,
            ConditionExpr::Not(inner) => {
                write!(f, "!")?;
                inner.fmt_with_prec(f, Prec::Not)?;
            }
            ConditionExpr::And(lhs, rhs) => {
                lhs.fmt_with_prec(f, Prec::And)?;
                write!(f, " && ")?;
                rhs.fmt_with_prec(f, Prec::And)?;
            }
            ConditionExpr::Or(lhs, rhs) => {
                lhs.fmt_with_prec(f, Prec::Or)?;
                write!(f, " || ")?;
                rhs.fmt_with_prec(f, Prec::Or)?;
            }
        }
        if needs_parens {
            write!(f, ")")?;
        }
        Ok(())
    }
}

impl fmt::Display for ConditionExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_with_prec(f, Prec::Or)
    }
}

// ---------------------------------------------------------------------------
// Serde
// ---------------------------------------------------------------------------

impl Serialize for ConditionExpr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ConditionExpr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        ConditionExpr::parse(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn ctx(
        cursor: usize,
        width: usize,
        byte_len: usize,
        selected: usize,
        len: usize,
    ) -> EvalContext {
        ctx_with_original(cursor, width, byte_len, selected, len, false)
    }

    fn ctx_with_original(
        cursor: usize,
        width: usize,
        byte_len: usize,
        selected: usize,
        len: usize,
        original_input_empty: bool,
    ) -> EvalContext {
        EvalContext {
            cursor_position: cursor,
            input_width: width,
            input_byte_len: byte_len,
            selected_index: selected,
            results_len: len,
            original_input_empty,
            has_context: false,
        }
    }

    // -- Atom evaluation (carried over from Phase 0) --

    #[test]
    fn atom_cursor_at_start() {
        assert!(ConditionAtom::CursorAtStart.evaluate(&ctx(0, 5, 5, 0, 10)));
        assert!(!ConditionAtom::CursorAtStart.evaluate(&ctx(3, 5, 5, 0, 10)));
    }

    #[test]
    fn atom_cursor_at_end() {
        assert!(ConditionAtom::CursorAtEnd.evaluate(&ctx(5, 5, 5, 0, 10)));
        assert!(!ConditionAtom::CursorAtEnd.evaluate(&ctx(3, 5, 5, 0, 10)));
        assert!(ConditionAtom::CursorAtEnd.evaluate(&ctx(0, 0, 0, 0, 10)));
    }

    #[test]
    fn atom_input_empty() {
        assert!(ConditionAtom::InputEmpty.evaluate(&ctx(0, 0, 0, 0, 10)));
        assert!(!ConditionAtom::InputEmpty.evaluate(&ctx(0, 5, 5, 0, 10)));
    }

    #[test]
    fn atom_original_input_empty() {
        // original_input_empty = true
        assert!(
            ConditionAtom::OriginalInputEmpty.evaluate(&ctx_with_original(0, 0, 0, 0, 10, true))
        );
        // original_input_empty = false
        assert!(
            !ConditionAtom::OriginalInputEmpty.evaluate(&ctx_with_original(0, 0, 0, 0, 10, false))
        );
        // original_input_empty is independent of current input state
        assert!(
            ConditionAtom::OriginalInputEmpty.evaluate(&ctx_with_original(0, 5, 5, 0, 10, true))
        );
    }

    #[test]
    fn atom_list_at_end() {
        assert!(ConditionAtom::ListAtEnd.evaluate(&ctx(0, 0, 0, 99, 100)));
        assert!(!ConditionAtom::ListAtEnd.evaluate(&ctx(0, 0, 0, 50, 100)));
        assert!(ConditionAtom::ListAtEnd.evaluate(&ctx(0, 0, 0, 0, 0)));
    }

    #[test]
    fn atom_list_at_start() {
        assert!(ConditionAtom::ListAtStart.evaluate(&ctx(0, 0, 0, 0, 100)));
        assert!(!ConditionAtom::ListAtStart.evaluate(&ctx(0, 0, 0, 50, 100)));
        assert!(ConditionAtom::ListAtStart.evaluate(&ctx(0, 0, 0, 0, 0)));
    }

    #[test]
    fn atom_no_results_and_has_results() {
        assert!(ConditionAtom::NoResults.evaluate(&ctx(0, 0, 0, 0, 0)));
        assert!(!ConditionAtom::NoResults.evaluate(&ctx(0, 0, 0, 0, 5)));
        assert!(ConditionAtom::HasResults.evaluate(&ctx(0, 0, 0, 0, 5)));
        assert!(!ConditionAtom::HasResults.evaluate(&ctx(0, 0, 0, 0, 0)));
    }

    #[test]
    fn atom_has_context() {
        let mut context = ctx(0, 0, 0, 0, 0);
        assert!(!ConditionAtom::HasContext.evaluate(&context));
        context.has_context = true;
        assert!(ConditionAtom::HasContext.evaluate(&context));
    }

    #[rstest]
    #[case::cursor_at_start("cursor-at-start")]
    #[case::cursor_at_end("cursor-at-end")]
    #[case::input_empty("input-empty")]
    #[case::original_input_empty("original-input-empty")]
    #[case::list_at_end("list-at-end")]
    #[case::list_at_start("list-at-start")]
    #[case::no_results("no-results")]
    #[case::has_results("has-results")]
    fn atom_parse_round_trip(#[case] s: &str) {
        let c = ConditionAtom::from_str(s).unwrap();
        assert_eq!(c.as_str(), s);
    }

    #[test]
    fn atom_parse_unknown() {
        assert!(ConditionAtom::from_str("unknown-condition").is_err());
    }

    // -- Parser tests --

    #[rstest]
    #[case::bare_atom("cursor-at-start", ConditionExpr::Atom(ConditionAtom::CursorAtStart))]
    #[case::negation(
        "!no-results",
        ConditionExpr::Not(Box::new(ConditionExpr::Atom(ConditionAtom::NoResults)))
    )]
    #[case::double_negation(
        "!!no-results",
        ConditionExpr::Not(Box::new(ConditionExpr::Not(Box::new(ConditionExpr::Atom(
            ConditionAtom::NoResults
        )))))
    )]
    #[case::and(
        "cursor-at-start && input-empty",
        ConditionExpr::And(
            Box::new(ConditionExpr::Atom(ConditionAtom::CursorAtStart)),
            Box::new(ConditionExpr::Atom(ConditionAtom::InputEmpty)),
        )
    )]
    #[case::or(
        "list-at-start || no-results",
        ConditionExpr::Or(
            Box::new(ConditionExpr::Atom(ConditionAtom::ListAtStart)),
            Box::new(ConditionExpr::Atom(ConditionAtom::NoResults)),
        )
    )]
    // "a || b && c" should parse as "a || (b && c)"
    #[case::precedence_and_binds_tighter_than_or(
        "cursor-at-start || input-empty && no-results",
        ConditionExpr::Or(
            Box::new(ConditionExpr::Atom(ConditionAtom::CursorAtStart)),
            Box::new(ConditionExpr::And(
                Box::new(ConditionExpr::Atom(ConditionAtom::InputEmpty)),
                Box::new(ConditionExpr::Atom(ConditionAtom::NoResults)),
            )),
        )
    )]
    // "(a || b) && c"
    #[case::parens_override_precedence(
        "(cursor-at-start || input-empty) && no-results",
        ConditionExpr::And(
            Box::new(ConditionExpr::Or(
                Box::new(ConditionExpr::Atom(ConditionAtom::CursorAtStart)),
                Box::new(ConditionExpr::Atom(ConditionAtom::InputEmpty)),
            )),
            Box::new(ConditionExpr::Atom(ConditionAtom::NoResults)),
        )
    )]
    // "(a && !b) || c"
    #[case::complex_nested(
        "(cursor-at-start && !input-empty) || no-results",
        ConditionExpr::Or(
            Box::new(ConditionExpr::And(
                Box::new(ConditionExpr::Atom(ConditionAtom::CursorAtStart)),
                Box::new(ConditionExpr::Not(Box::new(ConditionExpr::Atom(
                    ConditionAtom::InputEmpty
                )))),
            )),
            Box::new(ConditionExpr::Atom(ConditionAtom::NoResults)),
        )
    )]
    fn parses_condition(#[case] input: &str, #[case] expected: ConditionExpr) {
        let expr = ConditionExpr::parse(input).unwrap();
        assert_eq!(expr, expected);
    }

    #[test]
    fn parse_whitespace_tolerance() {
        let a = ConditionExpr::parse("cursor-at-start||input-empty").unwrap();
        let b = ConditionExpr::parse("cursor-at-start || input-empty").unwrap();
        let c = ConditionExpr::parse("  cursor-at-start  ||  input-empty  ").unwrap();
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[rstest]
    #[case::unknown_atom("unknown-thing")]
    #[case::trailing_input("cursor-at-start blah")]
    #[case::unmatched_paren("(cursor-at-start")]
    #[case::empty("")]
    fn rejects_invalid_condition(#[case] input: &str) {
        assert!(ConditionExpr::parse(input).is_err());
    }

    // -- Expression evaluation --

    #[test]
    fn eval_not() {
        let expr = ConditionExpr::parse("!no-results").unwrap();
        // Has results → !no-results is true
        assert!(expr.evaluate(&ctx(0, 0, 0, 0, 5)));
        // No results → !no-results is false
        assert!(!expr.evaluate(&ctx(0, 0, 0, 0, 0)));
    }

    #[test]
    fn eval_and() {
        let expr = ConditionExpr::parse("cursor-at-start && input-empty").unwrap();
        // Both true
        assert!(expr.evaluate(&ctx(0, 0, 0, 0, 10)));
        // First true, second false (non-empty input)
        assert!(!expr.evaluate(&ctx(0, 5, 5, 0, 10)));
        // First false (cursor not at start)
        assert!(!expr.evaluate(&ctx(3, 5, 5, 0, 10)));
    }

    #[test]
    fn eval_or() {
        let expr = ConditionExpr::parse("list-at-start || no-results").unwrap();
        // list at bottom (selected=0)
        assert!(expr.evaluate(&ctx(0, 0, 0, 0, 10)));
        // no results
        assert!(expr.evaluate(&ctx(0, 0, 0, 0, 0)));
        // neither
        assert!(!expr.evaluate(&ctx(0, 0, 0, 5, 10)));
    }

    #[test]
    fn eval_complex_nested() {
        // (cursor-at-start && !input-empty) || no-results
        let expr = ConditionExpr::parse("(cursor-at-start && !input-empty) || no-results").unwrap();

        // cursor at start, input not empty → true (left branch)
        assert!(expr.evaluate(&ctx(0, 5, 5, 0, 10)));
        // no results → true (right branch)
        assert!(expr.evaluate(&ctx(3, 5, 5, 0, 0)));
        // cursor not at start, has results → false
        assert!(!expr.evaluate(&ctx(3, 5, 5, 0, 10)));
        // cursor at start, input empty → false (left: && fails; right: has results)
        assert!(!expr.evaluate(&ctx(0, 0, 0, 0, 10)));
    }

    // -- Display --

    #[rstest]
    #[case::atom(ConditionExpr::Atom(ConditionAtom::CursorAtStart), "cursor-at-start")]
    #[case::not(ConditionExpr::Atom(ConditionAtom::NoResults).not(), "!no-results")]
    #[case::and(
        ConditionExpr::Atom(ConditionAtom::CursorAtStart)
            .and(ConditionExpr::Atom(ConditionAtom::InputEmpty)),
        "cursor-at-start && input-empty"
    )]
    #[case::or(
        ConditionExpr::Atom(ConditionAtom::ListAtStart)
            .or(ConditionExpr::Atom(ConditionAtom::NoResults)),
        "list-at-start || no-results"
    )]
    fn displays_condition(#[case] expr: ConditionExpr, #[case] expected: &str) {
        assert_eq!(expr.to_string(), expected);
    }

    #[test]
    fn display_parens_when_needed() {
        // (a || b) && c — the Or inside And needs parens
        let expr = ConditionExpr::Atom(ConditionAtom::CursorAtStart)
            .or(ConditionExpr::Atom(ConditionAtom::InputEmpty))
            .and(ConditionExpr::Atom(ConditionAtom::NoResults));
        assert_eq!(
            expr.to_string(),
            "(cursor-at-start || input-empty) && no-results"
        );
    }

    #[test]
    fn display_no_parens_when_not_needed() {
        // a || b && c — no parens needed (and binds tighter)
        let inner_and = ConditionExpr::Atom(ConditionAtom::InputEmpty)
            .and(ConditionExpr::Atom(ConditionAtom::NoResults));
        let expr = ConditionExpr::Atom(ConditionAtom::CursorAtStart).or(inner_and);
        assert_eq!(
            expr.to_string(),
            "cursor-at-start || input-empty && no-results"
        );
    }

    // -- Display round-trip --

    #[rstest]
    #[case::atom("cursor-at-start")]
    #[case::not("!no-results")]
    #[case::and("cursor-at-start && input-empty")]
    #[case::or("list-at-start || no-results")]
    #[case::parenthesized_or_in_and("(cursor-at-start || input-empty) && no-results")]
    #[case::parenthesized_and_in_or("(cursor-at-start && !input-empty) || no-results")]
    fn display_round_trip(#[case] s: &str) {
        let expr = ConditionExpr::parse(s).unwrap();
        let displayed = expr.to_string();
        let reparsed = ConditionExpr::parse(&displayed).unwrap();
        assert_eq!(expr, reparsed, "round-trip failed for: {s}");
    }

    // -- Serde --

    #[test]
    fn serde_simple_atom() {
        let expr = ConditionExpr::Atom(ConditionAtom::CursorAtStart);
        let json = serde_json::to_string(&expr).unwrap();
        assert_eq!(json, "\"cursor-at-start\"");
        let parsed: ConditionExpr = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, expr);
    }

    #[test]
    fn serde_compound_expression() {
        let json = "\"cursor-at-start && !input-empty\"";
        let parsed: ConditionExpr = serde_json::from_str(json).unwrap();
        let expected = ConditionExpr::And(
            Box::new(ConditionExpr::Atom(ConditionAtom::CursorAtStart)),
            Box::new(ConditionExpr::Not(Box::new(ConditionExpr::Atom(
                ConditionAtom::InputEmpty,
            )))),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn serde_round_trip() {
        let expr = ConditionExpr::parse("(cursor-at-start && !input-empty) || no-results").unwrap();
        let json = serde_json::to_string(&expr).unwrap();
        let parsed: ConditionExpr = serde_json::from_str(&json).unwrap();
        assert_eq!(expr, parsed);
    }

    // -- From<ConditionAtom> --

    #[test]
    fn from_atom_into_expr() {
        let expr: ConditionExpr = ConditionAtom::CursorAtStart.into();
        assert_eq!(expr, ConditionExpr::Atom(ConditionAtom::CursorAtStart));
    }

    // -- Builder helpers --

    #[test]
    fn builder_chain() {
        let expr = ConditionExpr::from(ConditionAtom::CursorAtStart)
            .and(ConditionExpr::from(ConditionAtom::InputEmpty).not())
            .or(ConditionExpr::from(ConditionAtom::NoResults));
        // And binds tighter than Or, so no parens needed around the And
        assert_eq!(
            expr.to_string(),
            "cursor-at-start && !input-empty || no-results"
        );
    }
}
