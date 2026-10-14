//! The tool calls a Codex `exec` script makes. Since 0.160 Codex runs every tool the model asks
//! for through one (codex-rs `tools/code_mode`): the model writes JavaScript calling them as
//! `tools.<name>(<arguments>)`, as in `await tools.exec_command({cmd: "ls", workdir: "/w"})` or
//! `const patch = "*** Begin Patch ..."; await tools.apply_patch(patch)`.
//!
//! No JavaScript is run or parsed whole: the script is split into tokens, and the arguments of
//! each `tools.<name>(...)` read where they are literals (strings in any quotes, numbers, objects
//! and lists of them) or a name an earlier `const` gave a string, number or `true`/`false`/`null`.
//! Where that may have missed something (a string, regex or comment left open, a name escaped,
//! more uses of `tools` in the text than calls read, or a name read that the script mentions
//! anywhere else, as it may then hold something else), the script is not read
//! [`whole`](Script::whole).

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

/// One tool call a script makes, and its arguments as JSON.
#[derive(Debug, PartialEq)]
pub(super) struct Call {
    pub name: String,
    pub input: Value,
}

/// What [`calls`] makes of a script.
#[derive(Debug, PartialEq)]
pub(super) struct Script {
    /// The calls it makes, with their arguments as far as they are literals.
    pub calls: Vec<Call>,
    /// Whether that is all of it: every call's arguments were literals, every use of `tools`
    /// such a call, and nothing in the script was left open.
    pub whole: bool,
}

/// Where a script uses `tools`, however it reads (`tools.x`, `tools [`), in strings and
/// comments too.
static TOOLS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\btools\s*[.\[]").expect("a valid regex"));

/// The tool calls `script` makes.
pub(super) fn calls(script: &str) -> Script {
    let (tokens, closed) = tokens(script);
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
        bindings: Map::new(),
        reads: Vec::new(),
        whole: closed,
    };
    let mut calls = Vec::new();
    while let Some(token) = tokens.get(parser.at) {
        let after_dot = parser.at > 0 && tokens[parser.at - 1] == Token::Punct('.');
        match token {
            Token::Ident(word) if word == "const" => {
                parser.at += 1;
                parser.binding();
            }
            Token::Ident(word) if word == "tools" && !after_dot => {
                parser.at += 1;
                match parser.call() {
                    Some(call) => calls.push(call),
                    None => parser.whole = false,
                }
            }
            _ => parser.at += 1,
        }
    }
    // A misread `/` (division taken for a regex, or the other way about) can hide calls in what
    // it takes for a string: then the text has more of them than were read.
    let whole = parser.whole
        && TOOLS.find_iter(script).count() <= calls.len()
        && parser.reads_are_all_mentions();
    Script { calls, whole }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A string, and whether it is one (`false` for a template interpolating `${...}`, kept as
    /// written).
    Str(String, bool),
    Ident(String),
    Num(String),
    Punct(char),
}

/// `script`'s tokens, and whether every string, regex and comment in it was closed and no name
/// in it escaped.
fn tokens(script: &str) -> (Vec<Token>, bool) {
    let chars: Vec<char> = script.chars().collect();
    let mut tokens = Vec::new();
    let mut closed = true;
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && next == Some('/') {
            while chars.get(i).is_some_and(|&c| c != '\n') {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            closed &= i < chars.len();
            i += 2;
        } else if c == '/' && regex_may_start(tokens.last()) {
            let (end, ended) = past_regex(&chars, i + 1);
            closed &= ended;
            i = end;
        } else if matches!(c, '"' | '\'' | '`') {
            let (token, ended, end) = string(&chars, i + 1, c);
            closed &= ended;
            tokens.push(token);
            i = end;
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while chars.get(i).is_some_and(|&c| c.is_alphanumeric() || c == '_' || c == '$') {
                i += 1;
            }
            tokens.push(Token::Ident(chars[start..i].iter().collect()));
        } else if c.is_ascii_digit() {
            let start = i;
            while chars.get(i).is_some_and(|&c| c.is_ascii_alphanumeric() || c == '.' || c == '_') {
                i += 1;
            }
            tokens.push(Token::Num(chars[start..i].iter().collect()));
        } else {
            // Outside a string a `\` escapes a name (`\u0074ools` is `tools`): not read.
            closed &= c != '\\';
            tokens.push(Token::Punct(c));
            i += 1;
        }
    }
    (tokens, closed)
}

/// Whether a `/` after `previous` starts a regex rather than dividing what came before it.
fn regex_may_start(previous: Option<&Token>) -> bool {
    match previous {
        None => true,
        Some(Token::Punct(c)) => !matches!(c, ')' | ']' | '}'),
        Some(Token::Ident(word)) => matches!(word.as_str(), "return" | "typeof" | "in" | "of"),
        Some(Token::Str(..) | Token::Num(_)) => false,
    }
}

/// Where the regex whose body starts at `i` ends, its flags and all, and whether it was closed.
fn past_regex(chars: &[char], mut i: usize) -> (usize, bool) {
    let mut class = false;
    let mut ended = false;
    while let Some(&c) = chars.get(i) {
        i += 1;
        match c {
            '\\' => i += 1,
            '[' => class = true,
            ']' => class = false,
            '/' if !class => {
                ended = true;
                break;
            }
            '\n' => break,
            _ => {}
        }
    }
    while chars.get(i).is_some_and(|c| c.is_alphanumeric()) {
        i += 1;
    }
    (i, ended)
}

/// The string whose text starts at `i`, closed by `quote`: it (see [`Token::Str`]), whether it
/// was closed (a quoted string can't hold a line break), and where it ends.
fn string(chars: &[char], mut i: usize, quote: char) -> (Token, bool, usize) {
    let mut text = String::new();
    let mut literal = true;
    let mut ended = false;
    while let Some(&c) = chars.get(i) {
        i += 1;
        match c {
            c if c == quote => {
                ended = true;
                break;
            }
            '\\' => i = escape(chars, i, &mut text),
            '$' if quote == '`' && chars.get(i) == Some(&'{') => {
                literal = false;
                text.push('$');
                let mut depth = 0usize;
                while let Some(&c) = chars.get(i) {
                    text.push(c);
                    i += 1;
                    match c {
                        '{' => depth += 1,
                        '}' if depth <= 1 => break,
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
            }
            '\n' if quote != '`' => break,
            c => text.push(c),
        }
    }
    (Token::Str(text, literal), ended, i)
}

/// Decode the escape whose character is at `i` into `text`; where it ends.
fn escape(chars: &[char], i: usize, text: &mut String) -> usize {
    let Some(&c) = chars.get(i) else {
        return i;
    };
    let hex = |from: usize, len: usize| -> Option<char> {
        let digits: String = chars.get(from..from + len)?.iter().collect();
        char::from_u32(u32::from_str_radix(&digits, 16).ok()?)
    };
    match c {
        'n' => text.push('\n'),
        't' => text.push('\t'),
        'r' => text.push('\r'),
        'b' => text.push('\u{8}'),
        'f' => text.push('\u{c}'),
        'v' => text.push('\u{b}'),
        '0' => text.push('\0'),
        // A line continuation.
        '\n' => {}
        'x' => {
            if let Some(c) = hex(i + 1, 2) {
                text.push(c);
                return i + 3;
            }
            text.push('x');
        }
        'u' if chars.get(i + 1) == Some(&'{') => {
            let close = chars[i..].iter().position(|&c| c == '}').map(|at| i + at);
            if let Some(c) = close.and_then(|close| hex(i + 2, close - i - 2)) {
                text.push(c);
                return close.unwrap_or(i) + 1;
            }
            text.push('u');
        }
        'u' => {
            if let Some(c) = hex(i + 1, 4) {
                text.push(c);
                return i + 5;
            }
            text.push('u');
        }
        c => text.push(c),
    }
    i + 1
}

struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
    /// The literals the script named so far (`const patch = "..."`).
    bindings: Map<String, Value>,
    /// The names read for their values, once each time.
    reads: Vec<String>,
    /// Whether every use of `tools` so far was a call whose arguments were literals.
    whole: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, punct: char) -> bool {
        let found = self.peek() == Some(&Token::Punct(punct));
        if found {
            self.at += 1;
        }
        found
    }

    /// Whether every name read is mentioned only where it is given its value and where it was
    /// read: anywhere else (declared again in a block, a parameter, assigned to) it may hold
    /// something else by the time it is read. A key (`{cmd: cmd}`) is not a mention.
    fn reads_are_all_mentions(&self) -> bool {
        let mut reads: HashMap<&str, usize> = HashMap::new();
        for name in &self.reads {
            *reads.entry(name).or_default() += 1;
        }
        let mut mentions: HashMap<&str, usize> = HashMap::new();
        let next = self.tokens.iter().skip(1).map(Some).chain([None]);
        for (token, next) in self.tokens.iter().zip(next) {
            if let Token::Ident(word) = token
                && reads.contains_key(word.as_str())
                && next != Some(&Token::Punct(':'))
            {
                *mentions.entry(word).or_default() += 1;
            }
        }
        reads.iter().all(|(name, read)| mentions.get(name) == Some(&(1 + read)))
    }

    /// `<name> = <literal>`, after a `const`. Objects and lists are not kept, as what they hold
    /// can be changed without naming them again (in a `${...}`).
    fn binding(&mut self) {
        let Some(Token::Ident(name)) = self.peek().cloned() else {
            return;
        };
        self.at += 1;
        if !self.eat('=') {
            return;
        }
        // Whether the value is all literal says nothing of the calls, unless one is given it.
        let (at, reads) = (self.at, self.reads.len());
        let whole = std::mem::replace(&mut self.whole, true);
        match self.value() {
            Some(value @ (Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null))
                if self.whole && self.ends_value() =>
            {
                self.bindings.insert(name, value);
            }
            // Not a literal: the next token is looked at again (it may be a call of `tools`).
            _ => {
                self.at = at;
                self.reads.truncate(reads);
            }
        }
        self.whole = whole;
    }

    /// `.<name>(<arguments>)`, after `tools`; `None` when that isn't what follows, or the
    /// arguments are not all literals.
    fn call(&mut self) -> Option<Call> {
        if !self.eat('.') {
            return None;
        }
        let Some(Token::Ident(name)) = self.peek().cloned() else {
            return None;
        };
        self.at += 1;
        if !self.eat('(') {
            return None;
        }
        let input = if self.peek() == Some(&Token::Punct(')')) {
            Value::Null
        } else {
            match self.value() {
                Some(value) if self.ends_value() => value,
                _ => {
                    self.whole = false;
                    Value::Null
                }
            }
        };
        Some(Call { name, input })
    }

    /// Whether the value just read is all of the expression it is in.
    fn ends_value(&self) -> bool {
        matches!(
            self.peek(),
            None | Some(Token::Punct(',' | ')' | '}' | ']' | ';') | Token::Ident(_))
        )
    }

    /// A literal value, or a name bound to one, strings joined by `+` joined. An object or list
    /// holding something else is read as far as it is literal, the rest `null`, and the script no
    /// longer `whole`.
    fn value(&mut self) -> Option<Value> {
        let mut value = self.operand()?;
        while let Value::String(text) = &mut value
            && self.peek() == Some(&Token::Punct('+'))
        {
            self.at += 1;
            match self.operand()? {
                Value::String(more) => text.push_str(&more),
                _ => return None,
            }
        }
        Some(value)
    }

    fn operand(&mut self) -> Option<Value> {
        let token = self.peek()?.clone();
        self.at += 1;
        Some(match token {
            Token::Str(text, literal) => {
                self.whole &= literal;
                Value::String(text)
            }
            Token::Num(digits) => serde_json::from_str(&digits).unwrap_or(Value::String(digits)),
            Token::Ident(word) => match word.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                "null" | "undefined" => Value::Null,
                _ => {
                    let value = self.bindings.get(&word).cloned()?;
                    self.reads.push(word);
                    value
                }
            },
            Token::Punct('{') => self.object(),
            Token::Punct('[') => self.list(),
            Token::Punct(_) => return None,
        })
    }

    /// The rest of an object, after its `{`.
    fn object(&mut self) -> Value {
        let mut object = Map::new();
        loop {
            if self.eat('}') {
                return Value::Object(object);
            }
            let key = match self.peek().cloned() {
                Some(Token::Ident(key) | Token::Str(key, true) | Token::Num(key)) => key,
                // A bracket closing something else ends it: there is nothing more to skip.
                Some(_) => {
                    let at = self.at;
                    self.skip_member();
                    if self.at == at {
                        return Value::Object(object);
                    }
                    continue;
                }
                None => return Value::Object(object),
            };
            self.at += 1;
            let value = if self.eat(':') {
                self.member_value()
            } else {
                // `{cmd}`: the value of the name.
                if let Some(value) = self.bindings.get(&key).cloned() {
                    self.reads.push(key.clone());
                    value
                } else {
                    self.whole = false;
                    Value::Null
                }
            };
            object.insert(key, value);
            self.eat(',');
        }
    }

    /// The rest of a list, after its `[`.
    fn list(&mut self) -> Value {
        let mut items = Vec::new();
        loop {
            if self.eat(']') || self.peek().is_none() {
                return Value::Array(items);
            }
            let at = self.at;
            items.push(self.member_value());
            self.eat(',');
            // A bracket closing something else ends it (and `whole`, skipped as not a literal).
            if self.at == at {
                return Value::Array(items);
            }
        }
    }

    /// A value inside an object or list: `null` where it is not a literal, with the rest of the
    /// expression skipped.
    fn member_value(&mut self) -> Value {
        let (at, reads) = (self.at, self.reads.len());
        match self.value() {
            Some(value) if self.ends_value() && !matches!(self.peek(), Some(Token::Ident(_))) => {
                value
            }
            _ => {
                self.at = at;
                self.reads.truncate(reads);
                self.skip_member();
                Value::Null
            }
        }
    }

    /// Skip what is left of a member that is not a literal, to the `,` after it or the bracket
    /// closing its object or list; the script is no longer `whole`.
    fn skip_member(&mut self) {
        self.whole = false;
        let mut depth = 0usize;
        while let Some(token) = self.peek() {
            match token {
                Token::Punct('(' | '[' | '{') => depth += 1,
                Token::Punct(')' | ']' | '}') if depth == 0 => return,
                Token::Punct(')' | ']' | '}') => depth -= 1,
                Token::Punct(',') if depth == 0 => {
                    self.at += 1;
                    return;
                }
                _ => {}
            }
            self.at += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    fn call(name: &str, input: Value) -> Call {
        Call {
            name: name.to_owned(),
            input,
        }
    }

    #[rstest]
    #[case::single_quotes(r"tools.exec_command({cmd: 'it\'s'})", json!({"cmd": "it's"}))]
    #[case::hex_and_unicode(r#"tools.exec_command({cmd: "\x41B\u{43}"})"#, json!({"cmd": "ABC"}))]
    #[case::continuation("tools.exec_command({cmd: \"a\\\nb\"})", json!({"cmd": "ab"}))]
    #[case::nested("tools.exec_command({cmd: \"ls\", env: {A: [1, true, null]}})", json!({"cmd": "ls", "env": {"A": [1, true, null]}}))]
    #[case::shorthand("const cmd = 'ls'; tools.exec_command({cmd})", json!({"cmd": "ls"}))]
    #[case::named("const cmd = 'ls'; tools.exec_command({cmd: cmd})", json!({"cmd": "ls"}))]
    #[case::joined("const dir = '/w'; tools.exec_command({cmd: 'cd ' + dir + \" && ls\"})", json!({"cmd": "cd /w && ls"}))]
    #[case::comments("/* read */ tools.exec_command({cmd: 'ls' /* , cmd: 'rm' */}) // done", json!({"cmd": "ls"}))]
    fn arguments_are_read_as_literals(#[case] script: &str, #[case] input: Value) {
        let found = calls(script);
        assert!(found.whole);
        assert_eq!(found.calls, vec![call("exec_command", input)]);
    }

    /// What isn't a literal is `null`, and the script no longer read whole.
    #[rstest]
    #[case::computed_member("tools.exec_command({cmd: \"echo \" + x, workdir: \"/w\"})", vec![call("exec_command", json!({"cmd": null, "workdir": "/w"}))])]
    #[case::unknown_name("tools.apply_patch(patch)", vec![call("apply_patch", Value::Null)])]
    #[case::interpolation("tools.exec_command({cmd: `cat ${f}`})", vec![call("exec_command", json!({"cmd": "cat ${f}"}))])]
    #[case::reassigned("let c = 'ls'; c = 'psql'; tools.exec_command({cmd: c})", vec![call("exec_command", json!({"cmd": null}))])]
    #[case::joined_with_other("tools.exec_command({cmd: 'sleep ' + 1})", vec![call("exec_command", json!({"cmd": null}))])]
    #[case::indexed("tools['exec_command']({cmd: 'ls'})", vec![])]
    #[case::passed_on("const run = tools.exec_command; run({cmd: 'ls'})", vec![])]
    fn what_is_not_literal_is_not_whole(#[case] script: &str, #[case] want: Vec<Call>) {
        let found = calls(script);
        assert!(!found.whole);
        assert_eq!(found.calls, want);
    }

    /// What may have been misread is not read whole: a `/` taken for a regex or a division,
    /// swallowing calls into a string or a regex left open; a string, regex or comment left open;
    /// `tools` used more often than calls were read. The first four from review.
    #[rstest]
    #[case::regex_after_if(
        "if (ok) /\"/.test(x); await tools.exec_command({cmd:\"psql -c 1\"}); const y = \"z\";"
    )]
    #[case::regex_after_block(
        "const f = () => {}\n/\"/.test(s); await tools.exec_command({cmd:\"psql\"}); \"\""
    )]
    #[case::division_after_increment(
        "let i = 0; i++ / 2; await tools.exec_command({cmd:\"psql\"});"
    )]
    #[case::regex_after_empty_block("if(1){} /\"/.test(\"x\"); tools.exec_command({cmd:\"psql\"})")]
    #[case::open_string("tools.exec_command({cmd: 'ls'}); 'psql")]
    #[case::open_comment("tools.exec_command({cmd: 'ls'}); /* psql")]
    #[case::tools_in_a_string("tools.exec_command({cmd: 'ls'}); text('tools.exec_command')")]
    #[case::escaped_tools(r"await \u0074ools.exec_command({cmd:'cat .env'})")]
    #[case::escaped_tools_braced(r"await tool\u{73}.exec_command({cmd:'cat .env'})")]
    #[case::escaped_name(r"tools.\u0065xec_command({cmd:'cat .env'})")]
    #[case::escaped_shadow(
        r"const a='ls'; { let \u0061 = 'cat .env'; tools.exec_command({cmd:a}) }"
    )]
    fn what_may_be_misread_is_not_whole(#[case] script: &str) {
        assert!(!calls(script).whole);
    }

    /// A script with a stray closing bracket is read to its end, not forever.
    #[rstest]
    #[case::paren_in_object("tools.exec_command({cmd:'ls')})")]
    #[case::bracket_in_object("tools.exec_command({cmd:'ls']})")]
    #[case::paren_in_list("tools.exec_command([1)])")]
    #[case::brace_in_list("tools.exec_command({a: [1}]})")]
    fn a_stray_closer_ends_the_read(#[case] script: &'static str) {
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || send.send(calls(script).whole));
        let whole = receive.recv_timeout(std::time::Duration::from_secs(5)).expect("the read ends");
        assert!(!whole);
    }

    /// A name is only trusted where nothing else can change what it holds at the call: the
    /// first two from review.
    #[rstest]
    #[case::member_assigned("const a={cmd:'ls'}; a.cmd='cat .env'; await tools.exec_command(a);")]
    #[case::shadowed_in_block("const a='ls'; { let a = 'cat .env'; } tools.exec_command({cmd:a})")]
    #[case::shadowed_by_parameter(
        "const a='ls'; const f = a => tools.exec_command({cmd:a}); f('cat .env')"
    )]
    #[case::passed_elsewhere("const a={cmd:'ls'}; change(a); tools.exec_command(a)")]
    #[case::pushed("const a=['ls']; a.push('cat .env'); tools.exec_command({cmd:a})")]
    #[case::incremented("let n=1; n++; tools.exec_command({cmd:'ls', n})")]
    #[case::changed_in_template("const a={cmd:'ls'}; `${a.cmd='cat .env'}`; tools.exec_command(a)")]
    #[case::reassigned_in_template("let a='ls'; `${a='cat .env'}`; tools.exec_command({cmd:a})")]
    fn a_name_that_may_change_is_not_whole(#[case] script: &str) {
        assert!(!calls(script).whole);
    }

    /// A name read many times is checked once, not once per read: a script of thousands of
    /// calls reads in linear time.
    #[rstest]
    fn a_name_read_many_times_reads_whole() {
        let script =
            format!("const cmd = 'ls';{}", "tools.exec_command({cmd: cmd});".repeat(20_000));
        let found = calls(&script);
        assert!(found.whole);
        assert_eq!(found.calls.len(), 20_000);
    }

    proptest! {
        /// Any command, written as a JSON string (which is a JavaScript one), reads back as it is.
        #[test]
        fn a_quoted_command_reads_back(command in any::<String>()) {
            let script = format!("await tools.exec_command({{cmd:{}}});", json!(command));
            let found = calls(&script);
            prop_assert!(found.whole);
            prop_assert_eq!(found.calls, vec![call("exec_command", json!({"cmd": command}))]);
        }
    }
}
