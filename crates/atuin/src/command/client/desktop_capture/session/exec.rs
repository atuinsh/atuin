//! Finds the statement of a `sh` login file that replaces the shell with `exec`, such as the
//! `exec sway` that ends many console logins.
//!
//! The scan reads the file as bytes. It knows enough of the shell's grammar to tell the nesting
//! of compound commands, function definitions, and the words that a command starts with:
//! comments, quotes that span lines, `$(…)`, arithmetic, redirections, here-documents, and
//! continued lines. It doesn't parse the shell fully.

/// Where setup's block goes in a file so that the block runs before the first `exec` of the file
/// that replaces the shell.
///
/// An `exec` with only redirections, such as `exec >log 2>&1`, keeps the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecPlacement {
    /// No `exec` of the file replaces the shell.
    NoExec,
    /// The top-level statement that holds the `exec` starts at this offset, or the comment lines
    /// directly above that statement start there.
    Before(usize),
    /// The scan couldn't tell where the top-level statement that holds the `exec` starts.
    ///
    /// For example, the scan doesn't follow Zsh's `repeat` loop.
    Unknown {
        /// The start of the line that holds the `exec`.
        ///
        /// A scan that misreads the file can miss the first `exec`, so this is the first line that
        /// holds a word `exec` that may start a command.
        exec_line: usize,
    },
}

/// Finds where setup's block goes in `text` so that the block runs before the first `exec` of
/// `text` that replaces the shell.
///
/// The result is [`ExecPlacement::Before`] only when the scan read the nesting of the whole file
/// without a doubt, because a block inside a compound command can break the file. For example,
/// a block before the line `then exec sway` joins the condition of the `if`.
pub fn statement_before_exec(text: &[u8]) -> ExecPlacement {
    let mut scanner = Scanner {
        command_position: true,
        ..Scanner::default()
    };
    let mut lexer = Lexer::new(text, 0);
    let mut found = None;
    while let Some(token) = lexer.next_token() {
        if scanner.follow(token, text) == Followed::ExecProgram && found.is_none() {
            found = Some(Found {
                statement: scanner.statement,
                line: scanner.line.start,
            });
        }
    }
    let misread = scanner.reading == Reading::Doubtful
        || lexer.reading == Reading::Doubtful
        || !scanner.open.is_empty()
        || scanner.function_header;
    if !misread {
        return match found {
            None => ExecPlacement::NoExec,
            Some(Found {
                statement: Some(statement),
                ..
            }) => ExecPlacement::Before(statement),
            Some(Found { line, .. }) => ExecPlacement::Unknown { exec_line: line },
        };
    }
    // A misread can hide an `exec` from the scan, as when the scan takes the rest of the file
    // for the head of a `case`.
    let lines = found.map(|found| found.line).into_iter().chain(line_of_exec_word(text));
    lines.min().map_or(ExecPlacement::NoExec, |exec_line| ExecPlacement::Unknown { exec_line })
}

/// Finds the first `exec` of `text`, a file of a shell whose grammar the scan doesn't follow,
/// such as Nushell.
///
/// The result is [`ExecPlacement::Unknown`] for a file that has a word `exec` before a program,
/// and [`ExecPlacement::NoExec`] for any other file.
pub fn exec_without_grammar(text: &[u8]) -> ExecPlacement {
    line_of_exec_word(text)
        .map_or(ExecPlacement::NoExec, |exec_line| ExecPlacement::Unknown { exec_line })
}

/// The start of the first line of `text` that holds the word `exec`, outside a comment, before a
/// word that isn't a redirection.
///
/// The search doesn't follow the grammar, so it finds every `exec` that the scan can miss when
/// the scan misreads the file. It also finds an `exec` in a quote or a here-document.
fn line_of_exec_word(text: &[u8]) -> Option<usize> {
    let mut start = 0;
    for line in text.split_inclusive(|&byte| byte == b'\n') {
        if holds_exec_word(line) {
            return Some(start);
        }
        start += line.len();
    }
    None
}

/// Whether `line` holds the word `exec`, outside a comment, before a word that isn't a
/// redirection.
fn holds_exec_word(line: &[u8]) -> bool {
    let starts_word = |at: usize| {
        at == 0
            || line[at - 1].is_ascii_whitespace()
            || matches!(
                line[at - 1],
                b';' | b'&' | b'|' | b'(' | b')' | b'{' | b'}' | b'`' | b'\'' | b'"'
            )
    };
    for at in (0..line.len()).filter(|&at| starts_word(at)) {
        let rest = &line[at..];
        if rest.starts_with(b"#") {
            return false;
        }
        let word = rest.strip_prefix(b"\\").unwrap_or(rest);
        if let Some(arguments) = word.strip_prefix(b"exec")
            && arguments.first().is_none_or(u8::is_ascii_whitespace)
            && names_program(arguments)
        {
            return true;
        }
    }
    false
}

/// Whether the words of `arguments`, which follow an `exec` in a line, name a program before
/// the command ends.
fn names_program(mut arguments: &[u8]) -> bool {
    loop {
        arguments = arguments.trim_ascii_start();
        let descriptor = match arguments.first() {
            None | Some(b';' | b'|' | b')' | b'#') => return false,
            Some(b'&') if !arguments.starts_with(b"&>") => return false,
            Some(b'{') => arguments.iter().position(|&byte| byte == b'}').map_or(0, |end| end + 1),
            Some(_) => arguments.iter().take_while(|byte| byte.is_ascii_digit()).count(),
        };
        let redirection = &arguments[descriptor..];
        if !matches!(redirection.first(), Some(b'<' | b'>')) && !redirection.starts_with(b"&>") {
            return true;
        }
        let operator = redirection
            .iter()
            .take_while(|byte| matches!(byte, b'<' | b'>' | b'&' | b'|' | b'-'))
            .count();
        let target = redirection[operator..].trim_ascii_start();
        let length = target
            .iter()
            .take_while(|&&byte| {
                !byte.is_ascii_whitespace() && !matches!(byte, b';' | b'&' | b'|' | b')')
            })
            .count();
        arguments = &target[length..];
    }
}

/// Where the first `exec` that replaces the shell is.
struct Found {
    /// The start of its top-level statement, with the comment lines above it.
    statement: Option<usize>,
    /// The start of its line.
    line: usize,
}

/// What a token was to the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Followed {
    /// The token names the program of an `exec`.
    ExecProgram,
    Other,
}

/// What the scan knows between tokens.
#[derive(Default)]
struct Scanner {
    /// The compound commands that are open, innermost last.
    open: Vec<Compound>,
    /// Where the top-level statement that the scan is in starts, with its comment lines.
    statement: Option<usize>,
    /// Where the comment lines that directly precede the next statement start.
    comments: Option<usize>,
    line: Line,
    reading: Reading,
    /// Whether the next word starts a command.
    command_position: bool,
    /// Whether the last token joins the next line to the statement, as `&&` does.
    continues: bool,
    /// Whether a function's name came, and its body is still to come.
    function_header: bool,
    /// What the next tokens mean.
    expecting: Expecting,
}

/// Whether the scan's model of the nesting explains everything that the scan met.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Reading {
    #[default]
    Followed,
    /// The scan met something that its model doesn't explain, such as a `then` outside an `if`,
    /// or a compound command that it doesn't follow, such as Zsh's `repeat`.
    Doubtful,
}

/// What the scan knows about the current line.
#[derive(Default)]
struct Line {
    start: usize,
    has_code: bool,
    has_comment: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Expecting {
    #[default]
    Nothing,
    /// The last token was the first word of a simple command, which `()` makes a function name.
    CommandName,
    /// `(` after a function name, before `)`.
    FunctionParens,
    /// The name after `function`.
    FunctionName,
    /// The words after `exec`. A word that isn't a redirection names the program.
    ExecArguments,
    /// The word after the body in braces of Zsh's short form of `if`, which can be `else` or
    /// `elif`, as in `if [[ $x ]] { a } else { b }`.
    ShortElse,
}

/// Whether a compound command with a grammar of its own took a token, from
/// [`Scanner::follow_own_grammar`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claimed {
    Taken,
    /// The token means what it means outside the compound command.
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compound {
    If,
    /// A `case` before its `in`.
    CaseHead,
    /// A `case` where its next pattern, or its `esac`, starts.
    CasePattern,
    /// A pattern of a `case`, after its first word or its `(`, before its `)`. `esac` doesn't end
    /// the `case` here, as in the pattern `(esac)`.
    CasePatternWords,
    /// The commands of a pattern of a `case`, which end with `;;` or with `esac`.
    CaseArm,
    Loop,
    /// A conditional expression of Bash and Zsh, `[[ … ]]`, whose words don't start commands
    /// and can span lines.
    Test,
    Brace,
    Subshell,
    /// The words in parentheses of Zsh's short form of `for`.
    WordList,
    /// The body in braces of Zsh's short form of `if` or a loop.
    ShortBody,
}

impl Scanner {
    /// Follows `token` of `text`.
    fn follow(&mut self, token: Token<'_>, text: &[u8]) -> Followed {
        match token {
            Token::Comment => {
                self.line.has_comment = true;
                return Followed::Other;
            }
            Token::Newline { next_line } => {
                self.end_line(text);
                self.line.start = next_line;
                return Followed::Other;
            }
            _ => {}
        }
        self.line.has_code = true;
        if self.statement.is_none() {
            self.statement = Some(self.comments.take().unwrap_or(self.line.start));
        }
        let expecting = std::mem::take(&mut self.expecting);
        match (expecting, token) {
            (Expecting::ExecArguments, Token::Word(_)) => return Followed::ExecProgram,
            (Expecting::ExecArguments, Token::Redirect)
            | (Expecting::FunctionParens, Token::Open) => self.expecting = expecting,
            (Expecting::CommandName, Token::Open) => self.expecting = Expecting::FunctionParens,
            // The body of `else` follows in braces, and `elif` takes a condition first.
            (Expecting::ShortElse, Token::Word(word @ (b"else" | b"elif"))) => {
                self.push(Compound::If);
                self.command_position = word == b"elif";
            }
            (Expecting::FunctionParens, Token::Close)
            | (Expecting::FunctionName, Token::Word(_)) => {
                self.function_header = true;
                self.command_position = true;
                self.expecting = if expecting == Expecting::FunctionName {
                    Expecting::FunctionParens
                } else {
                    Expecting::Nothing
                };
            }
            (_, token) => self.follow_command(token),
        }
        self.continues = matches!(token, Token::Separator {
            continues: true,
            ..
        });
        Followed::Other
    }

    /// Follows a token outside the special cases of `follow`.
    fn follow_command(&mut self, token: Token<'_>) {
        if self.follow_own_grammar(token) == Claimed::Taken {
            return;
        }
        match token {
            Token::Open if self.command_position => self.push(Compound::Subshell),
            // Zsh's short form of `for` takes its words in parentheses, as in
            // `for x (a b) { … }`.
            Token::Open if self.open.last() == Some(&Compound::Loop) => {
                self.push(Compound::WordList);
            }
            Token::Separator { .. } | Token::Open => self.command_position = true,
            Token::Close => match self.open.last() {
                Some(Compound::Subshell) => {
                    self.open.pop();
                    self.command_position = true;
                }
                Some(Compound::WordList) => {
                    self.open.pop();
                    self.command_position = false;
                }
                // A `)` that the scan doesn't pair, such as the end of Zsh's `(a|b)` in `[[ … ]]`.
                _ => self.command_position = true,
            },
            Token::Word(word) if self.command_position => self.command_word(word),
            // Zsh's short forms give `if` and the loops a body in braces, as in
            // `if [[ -o login ]] { exec sway }`.
            Token::Word(b"{")
                if matches!(self.open.last(), Some(Compound::If | Compound::Loop)) =>
            {
                self.push(Compound::ShortBody);
                self.command_position = true;
            }
            // Zsh closes a body with `}` after a command, as in `f() { echo hi }`.
            Token::Word(b"}") if self.open.last() == Some(&Compound::Brace) => {
                self.open.pop();
            }
            Token::Word(b"}") if self.open.last() == Some(&Compound::ShortBody) => {
                self.close_short_body();
            }
            // A redirection can come anywhere in a command, and takes its target with it.
            Token::Redirect | Token::Comment | Token::Newline { .. } | Token::Word(_) => {}
        }
    }

    /// Follows `token` where the innermost compound command gives the token its meaning: in the
    /// head or the patterns of a `case`, and in `[[ … ]]`.
    fn follow_own_grammar(&mut self, token: Token<'_>) -> Claimed {
        let Some(top) = self.open.last_mut() else {
            return Claimed::Left;
        };
        match (*top, token) {
            // `in` ends the head, and `;;` or the like ends an arm.
            (Compound::CaseHead, Token::Word(b"in"))
            | (
                Compound::CaseArm,
                Token::Separator {
                    ends_case_arm: true,
                    ..
                },
            ) => *top = Compound::CasePattern,
            (Compound::CasePattern, Token::Word(b"esac")) => {
                self.open.pop();
                self.command_position = true;
            }
            (Compound::CasePattern, Token::Open | Token::Word(_)) => {
                *top = Compound::CasePatternWords;
            }
            (Compound::CasePattern | Compound::CasePatternWords, Token::Close) => {
                *top = Compound::CaseArm;
                self.command_position = true;
            }
            (Compound::Test, Token::Word(b"]]")) => {
                self.open.pop();
                self.command_position = false;
            }
            // The words of the head, the words and `|` of a pattern, and the words, parentheses,
            // and operators of a test don't start commands.
            (
                Compound::CaseHead
                | Compound::CasePattern
                | Compound::CasePatternWords
                | Compound::Test,
                _,
            ) => {}
            _ => return Claimed::Left,
        }
        Claimed::Taken
    }

    /// Follows `word` at the start of a command.
    fn command_word(&mut self, word: &[u8]) {
        self.command_position = true;
        match word {
            b"exec" | b"\\exec" => {
                self.command_position = false;
                self.expecting = Expecting::ExecArguments;
            }
            b"function" => {
                self.command_position = false;
                self.expecting = Expecting::FunctionName;
            }
            b"if" => self.push(Compound::If),
            b"case" => {
                self.push(Compound::CaseHead);
                self.command_position = false;
            }
            b"for" | b"select" => {
                self.push(Compound::Loop);
                self.command_position = false;
            }
            b"while" | b"until" => self.push(Compound::Loop),
            b"[[" => {
                self.push(Compound::Test);
                self.command_position = false;
            }
            b"{" => self.push(Compound::Brace),
            b"fi" => self.close(Compound::If),
            b"esac" => self.close(Compound::CaseArm),
            b"done" => self.close(Compound::Loop),
            b"}" if self.open.last() == Some(&Compound::ShortBody) => self.close_short_body(),
            b"}" => self.close(Compound::Brace),
            b"then" | b"else" | b"elif" => self.expect_inside(Compound::If),
            b"do" => self.expect_inside(Compound::Loop),
            // Zsh's `repeat` and `foreach`, and Bash's `coproc`, start compound commands that
            // the scan doesn't follow.
            b"repeat" | b"foreach" | b"coproc" => self.reading = Reading::Doubtful,
            b"!" | b"time" | b"builtin" | b"command" => {}
            _ if is_assignment(word) => {}
            _ => {
                self.command_position = false;
                self.expecting = Expecting::CommandName;
                // Zsh takes a simple command as the body of a function, as in `f() echo hi`.
                self.function_header = false;
            }
        }
    }

    /// Closes a body of Zsh's short forms, and the `if` or loop that it belongs to.
    fn close_short_body(&mut self) {
        self.open.pop();
        if self.open.pop() == Some(Compound::If) {
            self.expecting = Expecting::ShortElse;
        }
    }

    /// Closes `compound`, which has to be the innermost compound command.
    fn close(&mut self, compound: Compound) {
        self.expect_inside(compound);
        if self.open.last() == Some(&compound) {
            self.open.pop();
        }
    }

    /// Notes a doubt unless `compound` is the innermost compound command.
    fn expect_inside(&mut self, compound: Compound) {
        if self.open.last() != Some(&compound) {
            self.reading = Reading::Doubtful;
        }
    }

    fn push(&mut self, compound: Compound) {
        self.open.push(compound);
        self.function_header = false;
    }

    /// Whether a `)` that comes next ends the command substitution that the scan reads, because
    /// nothing that is open in the substitution takes it.
    fn ends_substitution_at_close(&self) -> bool {
        let taken = matches!(
            self.open.last(),
            Some(
                Compound::Subshell
                    | Compound::WordList
                    | Compound::CasePattern
                    | Compound::CasePatternWords
                    | Compound::Test
            )
        );
        !taken && self.expecting != Expecting::FunctionParens
    }

    /// Ends the current line: a statement ends with it unless something continues the statement,
    /// and a line of comments joins the comments above the next statement.
    fn end_line(&mut self, text: &[u8]) {
        if self.expecting != Expecting::FunctionName {
            self.expecting = Expecting::Nothing;
        }
        self.command_position = true;
        if self.line.has_code {
            if self.open.is_empty() && !self.continues && !self.function_header {
                self.statement = None;
            }
        } else if self.statement.is_none() {
            // A shebang belongs to the file and stays its first line.
            let shebang = self.line.start == 0 && text.starts_with(b"#!");
            if self.line.has_comment && !shebang {
                self.comments.get_or_insert(self.line.start);
            } else {
                self.comments = None;
            }
        }
        self.line.has_code = false;
        self.line.has_comment = false;
    }
}

/// Whether `word` assigns a variable, such as `WLR_NO_HARDWARE_CURSORS=1`.
fn is_assignment(word: &[u8]) -> bool {
    let Some(equals) = word.iter().position(|&byte| byte == b'=') else {
        return false;
    };
    is_name(word[..equals].strip_suffix(b"+").unwrap_or(&word[..equals]))
}

/// Whether `name` is the name of a variable.
fn is_name(name: &[u8]) -> bool {
    name.first().is_some_and(|&byte| byte.is_ascii_alphabetic() || byte == b'_')
        && name.iter().all(|&byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// A piece of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Word(&'a [u8]),
    /// `;`, `&`, `|`, `&&`, `||`, `;;`, or the like.
    ///
    /// `continues` is true for the ones after which a statement goes on in the next line, and
    /// `ends_case_arm` for the ones that end the commands of a pattern of `case`, such as `;;`.
    Separator {
        continues: bool,
        ends_case_arm: bool,
    },
    Open,
    Close,
    /// A redirection with its target, such as `2>&1` or `<<EOF`.
    Redirect,
    Comment,
    /// The end of a line, and where the next line that holds code starts, after the bodies of
    /// here-documents.
    Newline {
        next_line: usize,
    },
}

/// A here-document whose body comes after the current line.
struct HereDocument {
    delimiter: Vec<u8>,
    tabs: Tabs,
}

/// What a here-document does with the leading tabs of its body's lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tabs {
    Kept,
    /// The lines lose their leading tabs, as with `<<-`.
    Stripped,
}

/// Splits a file into tokens.
struct Lexer<'a> {
    text: &'a [u8],
    at: usize,
    here_documents: Vec<HereDocument>,
    /// Where the last `(` that the lexer split off ends.
    open_end: Option<usize>,
    /// Whether the scans of the words, and of the command substitutions in them, explain what
    /// they met.
    reading: Reading,
}

impl<'a> Lexer<'a> {
    /// Splits `text` into tokens from the offset `at` on.
    const fn new(text: &'a [u8], at: usize) -> Self {
        Self {
            text,
            at,
            here_documents: Vec::new(),
            open_end: None,
            reading: Reading::Followed,
        }
    }

    fn next_token(&mut self) -> Option<Token<'a>> {
        let text = self.text;
        loop {
            match text.get(self.at)? {
                b' ' | b'\t' | b'\r' => self.at += 1,
                b'\\' if text.get(self.at + 1) == Some(&b'\n') => self.at += 2,
                _ => break,
            }
        }
        let start = self.at;
        let rest = &text[start..];
        let token = match rest[0] {
            b'\n' => {
                self.at += 1;
                self.skip_here_documents();
                Token::Newline { next_line: self.at }
            }
            b'#' => {
                // Zsh reads the `(#i)` of a word as glob flags where Bash reads a comment.
                if self.open_end == Some(start) {
                    self.reading = Reading::Doubtful;
                }
                self.at += rest.iter().position(|&byte| byte == b'\n').unwrap_or(rest.len());
                Token::Comment
            }
            b'&' if rest.get(1) == Some(&b'>') => self.redirect(),
            b'<' | b'>' => self.redirect(),
            b';' | b'&' | b'|' => {
                let length =
                    rest.iter().take_while(|byte| matches!(byte, b';' | b'&' | b'|')).count();
                self.at += length;
                let separator = &rest[..length];
                Token::Separator {
                    continues: matches!(separator, b"&&" | b"||" | b"|" | b"|&"),
                    ends_case_arm: matches!(separator, b";;" | b";&" | b";;&" | b";|"),
                }
            }
            b'(' if rest.starts_with(b"((") => {
                self.at = arithmetic_end(text, start, &mut self.reading);
                Token::Word(&text[start..self.at])
            }
            b'(' => {
                self.at += 1;
                self.open_end = Some(self.at);
                Token::Open
            }
            b')' => {
                self.at += 1;
                Token::Close
            }
            _ => {
                self.at = word_end(text, start, &mut self.reading);
                let word = &text[start..self.at];
                // A number, or a name in braces, right before `<` or `>` is the descriptor of a
                // redirection, as in `2>&1` and `{fd}>log`.
                let named = word
                    .strip_prefix(b"{")
                    .and_then(|word| word.strip_suffix(b"}"))
                    .is_some_and(|name| !name.is_empty() && is_name(name));
                let descriptor = (word.iter().all(u8::is_ascii_digit) || named)
                    && matches!(text.get(self.at), Some(b'<' | b'>'));
                if descriptor {
                    self.redirect()
                } else {
                    Token::Word(word)
                }
            }
        };
        Some(token)
    }

    /// Reads the redirection operator at the current offset, and its target.
    fn redirect(&mut self) -> Token<'a> {
        const OPERATORS: [&[u8]; 12] =
            [b"<<<", b"<<-", b"&>>", b"<<", b"<>", b"<&", b">>", b">&", b">|", b"&>", b"<", b">"];
        let rest = &self.text[self.at..];
        let operator = OPERATORS.iter().find(|operator| rest.starts_with(operator)).copied();
        let operator = operator.unwrap_or(b">");
        self.at += operator.len();
        while matches!(self.text.get(self.at), Some(b' ' | b'\t')) {
            self.at += 1;
        }
        let start = self.at;
        self.at = word_end(self.text, start, &mut self.reading);
        if let Some(tabs) = match operator {
            b"<<" => Some(Tabs::Kept),
            b"<<-" => Some(Tabs::Stripped),
            _ => None,
        } {
            self.here_documents.push(HereDocument::new(&self.text[start..self.at], tabs));
        }
        Token::Redirect
    }

    /// Skips the bodies of the here-documents that the last line started.
    fn skip_here_documents(&mut self) {
        self.at = skip_bodies(self.text, self.at, &std::mem::take(&mut self.here_documents));
    }
}

impl HereDocument {
    /// The here-document whose delimiter `word` names.
    fn new(word: &[u8], tabs: Tabs) -> Self {
        let delimiter =
            word.iter().copied().filter(|&byte| !matches!(byte, b'\'' | b'"' | b'\\')).collect();
        Self { delimiter, tabs }
    }
}

/// The offset after the bodies of `here_documents` in `text`, which start at `at`.
fn skip_bodies(text: &[u8], mut at: usize, here_documents: &[HereDocument]) -> usize {
    for here_document in here_documents {
        while at < text.len() {
            let rest = &text[at..];
            let length =
                rest.iter().position(|&byte| byte == b'\n').map_or(rest.len(), |end| end + 1);
            let line = &rest[..length];
            at += length;
            let mut line = line.strip_suffix(b"\n").unwrap_or(line);
            line = line.strip_suffix(b"\r").unwrap_or(line);
            if here_document.tabs == Tabs::Stripped {
                let tabs = line.iter().take_while(|&&byte| byte == b'\t').count();
                line = &line[tabs..];
            }
            if line == here_document.delimiter.as_slice() {
                break;
            }
        }
    }
    at
}

/// The offset after the arithmetic command `((…))` that starts at `start`. An arithmetic
/// command that doesn't end is a doubt, which goes to `reading`.
fn arithmetic_end(text: &[u8], start: usize, reading: &mut Reading) -> usize {
    let mut depth = 0_usize;
    for (at, &byte) in text.iter().enumerate().skip(start) {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
    }
    *reading = Reading::Doubtful;
    text.len()
}

/// The offset after the command substitution whose commands start at `start`, after its `$(`.
///
/// The commands go through the scan, which tells the `)` that ends the substitution from the
/// `)` of a pattern of `case` or of a subshell. A doubt of the scan goes to `reading`.
fn substitution_end(text: &[u8], start: usize, reading: &mut Reading) -> usize {
    let mut scanner = Scanner {
        command_position: true,
        line: Line {
            start,
            ..Line::default()
        },
        ..Scanner::default()
    };
    let mut lexer = Lexer::new(text, start);
    let end = loop {
        let Some(token) = lexer.next_token() else {
            // The substitution doesn't end.
            scanner.reading = Reading::Doubtful;
            break text.len();
        };
        if token == Token::Close && scanner.ends_substitution_at_close() {
            break lexer.at;
        }
        scanner.follow(token, text);
    };
    if scanner.reading == Reading::Doubtful || lexer.reading == Reading::Doubtful {
        *reading = Reading::Doubtful;
    }
    end
}

/// A quote that a word is inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quote {
    Single,
    Double,
    Back,
    /// `$'…'`, in which a backslash escapes the next byte.
    AnsiC,
}

impl Quote {
    /// The byte that ends the quote.
    const fn end(self) -> u8 {
        match self {
            Self::Single | Self::AnsiC => b'\'',
            Self::Double => b'"',
            Self::Back => b'`',
        }
    }
}

/// The offset after the word that starts at `start` in `text`.
///
/// Quotes, `$(…)`, `${…}`, and an array after `=` can span lines, and stay in the word, and so do
/// here-documents inside `$(…)` and comments inside an array. A doubt of the scan of a `$(…)` goes
/// to `reading`, and so does a quote or a bracket that doesn't end.
fn word_end(text: &[u8], start: usize, reading: &mut Reading) -> usize {
    let mut at = start;
    let mut depth = 0_usize;
    // Whether the outermost bracket opens an array, as in `path=(…)`.
    let mut array = false;
    let mut quote = None;
    let mut here_documents = Vec::new();
    while let Some(&byte) = text.get(at) {
        let substitutes = matches!(quote, None | Some(Quote::Double)) && byte == b'$';
        // In an arithmetic expansion, `<<` is a shift and doesn't start a here-document.
        if substitutes && text[at + 1..].starts_with(b"((") {
            at = arithmetic_end(text, at + 1, reading);
            continue;
        }
        if substitutes && text.get(at + 1) == Some(&b'(') {
            at = substitution_end(text, at + 2, reading);
            continue;
        }
        let ends = match (quote, byte) {
            (Some(open), _) if byte == Quote::end(open) => {
                quote = None;
                false
            }
            // A backslash escapes the next byte, apart from inside single quotes.
            (_, b'\\') if quote != Some(Quote::Single) => {
                at += 1;
                false
            }
            (Some(_), _) => false,
            (None, b'\'') => {
                quote = Some(Quote::Single);
                false
            }
            (None, b'"') => {
                quote = Some(Quote::Double);
                false
            }
            (None, b'`') => {
                quote = Some(Quote::Back);
                false
            }
            (None, b'$') if text.get(at + 1) == Some(&b'\'') => {
                quote = Some(Quote::AnsiC);
                at += 1;
                false
            }
            (None, b'$') if text.get(at + 1) == Some(&b'{') => {
                depth += 1;
                at += 1;
                false
            }
            (None, b'(') if depth == 0 && at > start && text[at - 1] == b'=' => {
                depth = 1;
                array = true;
                false
            }
            // A comment in an array ends at the end of its line.
            (None, b'#') if array && text[at - 1].is_ascii_whitespace() => {
                at += text[at..].iter().position(|&byte| byte == b'\n').unwrap_or(text.len() - at);
                continue;
            }
            (None, b'(' | b'{') if depth > 0 => {
                depth += 1;
                false
            }
            (None, b')' | b'}') if depth > 0 => {
                depth -= 1;
                array &= depth > 0;
                false
            }
            (None, b'<') if depth > 0 && text[at..].starts_with(b"<<") => {
                at = after_here_document_operator(text, at, &mut here_documents);
                continue;
            }
            (None, b'\n') if depth > 0 && !here_documents.is_empty() => {
                at = skip_bodies(text, at + 1, &std::mem::take(&mut here_documents));
                continue;
            }
            (None, _) if depth > 0 => false,
            (None, _) => {
                matches!(byte, b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>')
                    || byte.is_ascii_whitespace()
            }
        };
        if ends {
            break;
        }
        at += 1;
    }
    if at >= text.len() && (quote.is_some() || depth > 0) {
        *reading = Reading::Doubtful;
    }
    at.min(text.len()).max(start + 1).min(text.len())
}

/// The offset after the operator `<<` at `at` in a word of `text`, and after the delimiter that
/// follows the operator. The here-document that the operator starts goes to `here_documents`.
fn after_here_document_operator(
    text: &[u8],
    mut at: usize,
    here_documents: &mut Vec<HereDocument>,
) -> usize {
    at += 2;
    if text.get(at) == Some(&b'<') {
        // `<<<` is a here-string.
        return at + 1;
    }
    let tabs = if text.get(at) == Some(&b'-') {
        at += 1;
        Tabs::Stripped
    } else {
        Tabs::Kept
    };
    while matches!(text.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    let delimiter = at;
    while text.get(at).is_some_and(|&byte| {
        !byte.is_ascii_whitespace() && !matches!(byte, b')' | b';' | b'&' | b'|')
    }) {
        at += 1;
    }
    here_documents.push(HereDocument::new(&text[delimiter..at], tabs));
    at
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// The offset of the statement is shown as the text that follows it.
    #[rstest]
    #[case::none("export A=1\n", None)]
    #[case::empty("", None)]
    #[case::alone("export A=1\nexec sway\n", Some("exec sway\n"))]
    #[case::arch(
        "[[ -f ~/.bashrc ]] && . ~/.bashrc\n[ -z \"$WAYLAND_DISPLAY\" ] && [ \"$XDG_VTNR\" = 1 ] \
         && exec sway\n",
        Some("[ -z \"$WAYLAND_DISPLAY\" ] && [ \"$XDG_VTNR\" = 1 ] && exec sway\n")
    )]
    #[case::if_with_a_comment(
        "export A=1\n\n# Start X on tty1.\nif [ \"$(tty)\" = /dev/tty1 ]; then\n  exec \
         startx\nfi\n",
        Some("# Start X on tty1.\nif [ \"$(tty)\" = /dev/tty1 ]; then\n  exec startx\nfi\n")
    )]
    #[case::case(
        "export A=1\ncase \"$(cat ~/session)\" in\n  sway) exec sway ;;\n  i3) exec startx \
         ;;\nesac\n",
        Some("case \"$(cat ~/session)\" in\n  sway) exec sway ;;\n  i3) exec startx ;;\nesac\n")
    )]
    #[case::after_a_closed_if("if true; then\n  :\nfi\nexec sway\n", Some("exec sway\n"))]
    #[case::in_a_function("f() {\n  exec sway\n}\nf\n", Some("f() {\n  exec sway\n}\nf\n"))]
    #[case::continued(
        "export A=1\n[ -n \"$X\" ] \\\n  && exec sway\n",
        Some("[ -n \"$X\" ] \\\n  && exec sway\n")
    )]
    #[case::only_in_a_comment("# exec sway\nexport A=1 # exec sway\n", None)]
    #[case::part_of_a_word("execute\nexport exec_path=1\necho $exec_path\nnoexec sway\n", None)]
    #[case::an_argument("echo exec sway\n", None)]
    #[case::quoted("echo 'a; exec sway'\necho \"$(echo exec)\"\n", None)]
    #[case::in_a_here_document("cat <<EOF\nexec sway\nEOF\nexport A=1\n", None)]
    #[case::after_a_here_document("cat <<'EOF'\nx\nEOF\nexec sway\n", Some("exec sway\n"))]
    #[case::after_a_shebang("#!/bin/sh\nexec sway\n", Some("exec sway\n"))]
    #[case::after_a_blank_comment_gap("# about A\n\nexec sway\n", Some("exec sway\n"))]
    #[case::after_and_and(
        "[ -n \"$X\" ] &&\n  exec sway\n",
        Some("[ -n \"$X\" ] &&\n  exec sway\n")
    )]
    #[case::redirected("echo a >/dev/null 2>&1\nexec sway <x\n", Some("exec sway <x\n"))]
    #[case::in_a_loop(
        "for s in sway; do\n  exec \"$s\"\ndone\n",
        Some("for s in sway; do\n  exec \"$s\"\ndone\n")
    )]
    #[case::after_an_assignment(
        "[ \"$(tty)\" = /dev/tty1 ] && WLR_NO_HARDWARE_CURSORS=1 exec sway\n",
        Some("[ \"$(tty)\" = /dev/tty1 ] && WLR_NO_HARDWARE_CURSORS=1 exec sway\n")
    )]
    #[case::after_an_assignment_in_an_if(
        "if true; then\n  XDG_CURRENT_DESKTOP=sway exec sway\nfi\n",
        Some("if true; then\n  XDG_CURRENT_DESKTOP=sway exec sway\nfi\n")
    )]
    #[case::escaped("export A=1\n\\exec sway\n", Some("\\exec sway\n"))]
    #[case::builtin("export A=1\nbuiltin exec sway\n", Some("builtin exec sway\n"))]
    #[case::command("export A=1\ncommand exec sway\n", Some("command exec sway\n"))]
    #[case::function_header_on_its_own_line(
        "export A=1\nstart_wm()\n{\n  exec sway\n}\n[ \"$XDG_VTNR\" = 1 ] && start_wm\n",
        Some("start_wm()\n{\n  exec sway\n}\n[ \"$XDG_VTNR\" = 1 ] && start_wm\n")
    )]
    #[case::function_header_with_a_space(
        "f ()\n{\n  exec sway\n}\n",
        Some("f ()\n{\n  exec sway\n}\n")
    )]
    #[case::function_keyword(
        "export A=1\nfunction start_wm {\n  exec sway\n}\nstart_wm\n",
        Some("function start_wm {\n  exec sway\n}\nstart_wm\n")
    )]
    #[case::function_keyword_on_its_own_line(
        "function f\n{\n  exec sway\n}\n",
        Some("function f\n{\n  exec sway\n}\n")
    )]
    #[case::function_keyword_with_parens(
        "function f() {\n  exec sway\n}\n",
        Some("function f() {\n  exec sway\n}\n")
    )]
    #[case::only_redirections("exec >\"$HOME/.xsession.log\" 2>&1\nexport A=1\n", None)]
    #[case::a_descriptor_then_exec("exec 3>&1\nexec sway\n", Some("exec sway\n"))]
    #[case::stdin_then_exec("exec </dev/null\nexec sway\n", Some("exec sway\n"))]
    #[case::a_redirection_before_the_program(
        "exec 2>/dev/null sway\n",
        Some("exec 2>/dev/null sway\n")
    )]
    #[case::in_a_multi_line_double_quote("export MSG=\"hello\nexec sway\"\n", None)]
    #[case::in_a_multi_line_single_quote("alias wm='\nexec sway\n'\n", None)]
    #[case::in_a_multi_line_array("arr=(\n  exec\n)\n", None)]
    #[case::after_a_shift("(( x = 1 << 2 ))\nexec sway\n", Some("exec sway\n"))]
    #[case::arithmetic_condition(
        "if (( XDG_VTNR == 1 )); then exec sway; fi\n",
        Some("if (( XDG_VTNR == 1 )); then exec sway; fi\n")
    )]
    #[case::an_indented_delimiter_in_a_here_document(
        "cat <<EOF\n  EOF\nexec sway\nEOF\nexport A=1\n",
        None
    )]
    #[case::a_here_document_without_tabs(
        "cat <<-EOF\n\texec sway\n\tEOF\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::crlf("export A=1\r\nexec sway\r\n", Some("exec sway\r\n"))]
    #[case::subshell_group("[ x ] && {\n  exec sway\n}\n", Some("[ x ] && {\n  exec sway\n}\n"))]
    #[case::exec_in_a_zsh_short_if(
        "export A=1\nif [[ $XDG_VTNR = 1 ]] { exec sway }\n",
        Some("if [[ $XDG_VTNR = 1 ]] { exec sway }\n")
    )]
    #[case::exec_in_a_zsh_short_for(
        "export A=1\nfor x (a) { exec sway }\n",
        Some("for x (a) { exec sway }\n")
    )]
    #[case::after_a_zsh_short_if_with_exec(
        "if [[ -o x ]] { echo hi }\nexport A=1\nif [[ -o login ]] { exec sway }\nexport B=1\n",
        Some("if [[ -o login ]] { exec sway }\nexport B=1\n")
    )]
    #[case::a_shift_in_a_multi_line_substitution(
        "x=$(\n  y=$(( 1 << 2 ))\n  echo $y\n)\nexport A=1\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::zsh_short_function("f() { echo hi }\nexport A=1\nexec sway\n", Some("exec sway\n"))]
    #[case::zsh_short_if(
        "if [[ -o login ]] { echo hi }\nexport A=1\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::zsh_short_for("for x (a b) { echo $x }\nexport A=1\nexec sway\n", Some("exec sway\n"))]
    #[case::zsh_function_without_braces(
        "f() echo hi\nexport A=1\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::a_named_descriptor("exec {fd}>/dev/null\nexport A=1\n", None)]
    #[case::an_escaped_quote_in_ansi_c("x=$'it\\'s'\nexec sway\n", Some("exec sway\n"))]
    #[case::a_here_document_in_a_substitution(
        "x=$(cat <<EOF\nit's\nEOF\n)\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::exec_in_a_here_document_in_a_substitution("x=$(cat <<EOF\nexec sway\nEOF\n)\n", None)]
    #[case::then_on_the_line_of_the_exec(
        "if [ \"$XDG_VTNR\" = 1 ]\nthen exec sway\nfi\n",
        Some("if [ \"$XDG_VTNR\" = 1 ]\nthen exec sway\nfi\n")
    )]
    #[case::a_case_arm(
        "export A=1\ncase \"$XDG_VTNR\" in\n  1) exec sway ;;\nesac\n",
        Some("case \"$XDG_VTNR\" in\n  1) exec sway ;;\nesac\n")
    )]
    #[case::else_on_the_line_of_the_exec(
        "if [ \"$XDG_VTNR\" != 1 ]\nthen :\nelse exec sway\nfi\n",
        Some("if [ \"$XDG_VTNR\" != 1 ]\nthen :\nelse exec sway\nfi\n")
    )]
    #[case::after_a_posix_case_pattern(
        "case \"$TERM\" in (for) echo ;; esac\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::after_a_case_in_a_substitution(
        "x=$( case a in a) echo b;; esac )\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::after_a_case_in_a_quoted_substitution(
        "x=\"$(case a in a) echo \")\";; esac)\"\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::after_a_subshell_in_a_substitution("x=$( (echo a) )\nexec sway\n", Some("exec sway\n"))]
    #[case::after_a_function_in_a_substitution(
        "x=$(f() { echo a; }; f)\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::exec_in_a_substitution("x=$(exec sway)\nexport A=1\n", None)]
    #[case::an_arm_after_reserved_word_patterns(
        "case \"$XDG_VTNR\" in\n  done|fi) echo odd ;;\n  (esac) : ;;\n  1) exec sway ;;\nesac\n",
        Some(
            "case \"$XDG_VTNR\" in\n  done|fi) echo odd ;;\n  (esac) : ;;\n  1) exec sway \
             ;;\nesac\n"
        )
    )]
    #[case::an_arm_after_a_fallthrough(
        "case \"$XDG_VTNR\" in\n  0) : ;&\n  1) exec sway ;;\nesac\nexport A=1\n",
        Some("case \"$XDG_VTNR\" in\n  0) : ;&\n  1) exec sway ;;\nesac\nexport A=1\n")
    )]
    #[case::a_last_arm_without_a_terminator(
        "case \"$XDG_VTNR\" in\n  1) exec sway\nesac\nexport A=1\n",
        Some("case \"$XDG_VTNR\" in\n  1) exec sway\nesac\nexport A=1\n")
    )]
    #[case::in_on_its_own_line(
        "case \"$XDG_VTNR\"\nin\n  1) exec sway ;;\nesac\n",
        Some("case \"$XDG_VTNR\"\nin\n  1) exec sway ;;\nesac\n")
    )]
    #[case::after_a_test_with_a_reserved_word(
        "[[ $x =~ (for) ]] && echo\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::after_a_test_with_case(
        "[[ $TERM =~ (case|linux) ]] && echo\nif [ \"$XDG_VTNR\" = 1 ]; then\n  exec sway\nfi\n",
        Some("if [ \"$XDG_VTNR\" = 1 ]; then\n  exec sway\nfi\n")
    )]
    #[case::a_test_on_two_lines(
        "export A=1\n[[ -z $DISPLAY\n   && $XDG_VTNR -eq 1 ]] && exec sway\n",
        Some("[[ -z $DISPLAY\n   && $XDG_VTNR -eq 1 ]] && exec sway\n")
    )]
    #[case::a_test_in_a_substitution(
        "x=$( [[ $TERM =~ (a|b) ]] && echo y )\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::exec_as_a_word_of_a_test("[[ exec == $x ]] && echo\n", None)]
    #[case::after_an_array_with_a_comment(
        "path=(\n  $path\n  # Homebrew's binaries\n  /usr/bin\n)\nexec sway\n",
        Some("exec sway\n")
    )]
    #[case::exec_in_a_comment_of_an_array("path=(\n  # exec sway\n  /usr/bin\n)\n", None)]
    #[case::exec_in_zsh_short_else(
        "export A=1\nif [[ $XDG_VTNR != 1 ]] { echo other } else { exec sway }\n",
        Some("if [[ $XDG_VTNR != 1 ]] { echo other } else { exec sway }\n")
    )]
    #[case::exec_in_zsh_short_elif(
        "if [[ $XDG_VTNR = 0 ]] { echo } elif [[ $XDG_VTNR = 1 ]] { exec sway }\nexport A=1\n",
        Some(
            "if [[ $XDG_VTNR = 0 ]] { echo } elif [[ $XDG_VTNR = 1 ]] { exec sway }\nexport A=1\n"
        )
    )]
    fn finds_the_statement_with_exec(#[case] text: &str, #[case] expected: Option<&str>) {
        let expected = expected.map_or(Shown::NoExec, Shown::Before);
        assert_eq!(shown(text), expected);
    }

    /// A scan that misreads the nesting, or that meets something that it doesn't follow, leaves
    /// the place of the statement unknown. The case shows the line that holds the `exec`.
    #[rstest]
    #[case::then_on_the_line_of_the_exec(
        "coproc true\nif [ \"$XDG_VTNR\" = 1 ]\nthen exec sway\nfi\n",
        "then exec sway\nfi\n"
    )]
    #[case::a_case_arm(
        "coproc true\ncase \"$XDG_VTNR\" in\n  1) exec sway ;;\nesac\n",
        "  1) exec sway ;;\nesac\n"
    )]
    #[case::do_on_the_line_of_the_exec(
        "coproc true\nwhile [ \"$XDG_VTNR\" = 1 ]\ndo exec sway\ndone\n",
        "do exec sway\ndone\n"
    )]
    #[case::after_and_and("coproc true\n[ \"$XDG_VTNR\" = 1 ] &&\n  exec sway\n", "  exec sway\n")]
    #[case::on_its_own_line("coproc true\nexport A=1\nexec sway\n", "exec sway\n")]
    #[case::then_outside_an_if("then\nexec sway\n", "exec sway\n")]
    #[case::fi_outside_an_if("fi\nexec sway\n", "exec sway\n")]
    #[case::a_closing_brace_outside_braces("}\nexec sway\n", "exec sway\n")]
    #[case::zsh_repeat("repeat 3\ndo exec sway\ndone\n", "do exec sway\ndone\n")]
    #[case::zsh_foreach("foreach x (a)\n  exec sway\nend\n", "  exec sway\nend\n")]
    #[case::a_doubt_after_the_exec(
        "exec sway\ncoproc { true; }\n",
        "exec sway\ncoproc { true; }\n"
    )]
    #[case::a_function_header_at_the_end("exec sway\nf()\n", "exec sway\nf()\n")]
    #[case::an_open_substitution("exec sway\nx=$(echo\n", "exec sway\nx=$(echo\n")]
    #[case::a_doubt_in_a_substitution("x=$(fi)\nexec sway\n", "exec sway\n")]
    #[case::zsh_case_with_braces(
        "case $TERM {\n  (linux) export A=1 ;;\n}\n[ \"$XDG_VTNR\" = 1 ] && exec sway\n",
        "[ \"$XDG_VTNR\" = 1 ] && exec sway\n"
    )]
    #[case::an_exec_that_the_misread_hides(
        "case $TERM {\n  (linux) exec startx ;;\n}\nexec sway\n",
        "  (linux) exec startx ;;\n}\nexec sway\n"
    )]
    #[case::zsh_glob_flags_in_a_substitution(
        "x=$(print -l (#i)/usr/BIN)\n[ \"$XDG_VTNR\" = 1 ] && exec sway\n",
        "[ \"$XDG_VTNR\" = 1 ] && exec sway\n"
    )]
    #[case::zsh_glob_flags_on_one_line(
        "x=$(print (#i)a) y=$(echo b)\n[ \"$XDG_VTNR\" = 1 ] && exec sway\n",
        "[ \"$XDG_VTNR\" = 1 ] && exec sway\n"
    )]
    #[case::a_parenthesis_in_a_parameter_expansion(
        "x=${PWD%(}\n[ \"$XDG_VTNR\" = 1 ] && exec sway\n",
        "[ \"$XDG_VTNR\" = 1 ] && exec sway\n"
    )]
    #[case::a_quote_that_doesnt_end("echo \"a\nexec sway\n", "exec sway\n")]
    #[case::an_arithmetic_command_that_doesnt_end("(( x = 1\nexec sway\n", "exec sway\n")]
    fn leaves_the_place_unknown_when_it_misreads(#[case] text: &str, #[case] exec_line: &str) {
        assert_eq!(shown(text), Shown::Unknown(exec_line));
    }

    /// In a file that the scan misreads, every line that holds a word `exec` before a program
    /// counts as the line of the `exec`.
    ///
    /// The case shows the line, after a coprocess that the scan doesn't follow.
    #[rstest]
    #[case::a_program("exec sway\n", Some("exec sway\n"))]
    #[case::after_a_separator("export A=1; exec sway\n", Some("export A=1; exec sway\n"))]
    #[case::after_and_and("[ x ] &&exec sway\n", Some("[ x ] &&exec sway\n"))]
    #[case::escaped("\\exec sway\n", Some("\\exec sway\n"))]
    #[case::in_a_quote("echo 'exec sway'\n", Some("echo 'exec sway'\n"))]
    #[case::after_redirections(
        "exec 2>/dev/null {fd}>x &> log sway\n",
        Some("exec 2>/dev/null {fd}>x &> log sway\n")
    )]
    #[case::the_second_line("export A=1\n  exec sway\n", Some("  exec sway\n"))]
    #[case::only_redirections("exec >log 2>&1\nexec 3<&- ; echo\n", None)]
    #[case::alone("exec\nexec &\n", None)]
    #[case::in_a_comment("# exec sway\necho a # exec sway\n", None)]
    #[case::in_a_word("noexec sway\nexec_path=1\necho $exec\n", None)]
    fn finds_an_exec_word_in_a_misread_file(#[case] text: &str, #[case] expected: Option<&str>) {
        let text = format!("coproc true\n{text}");
        let expected = expected.map_or(Shown::NoExec, Shown::Unknown);
        assert_eq!(shown(&text), expected);
    }

    /// What [`statement_before_exec`] found in a text, with offsets shown as the text that
    /// follows them.
    #[derive(Debug, PartialEq, Eq)]
    enum Shown<'a> {
        NoExec,
        Before(&'a str),
        Unknown(&'a str),
    }

    fn shown(text: &str) -> Shown<'_> {
        match statement_before_exec(text.as_bytes()) {
            ExecPlacement::NoExec => Shown::NoExec,
            ExecPlacement::Before(at) => Shown::Before(&text[at..]),
            ExecPlacement::Unknown { exec_line } => Shown::Unknown(&text[exec_line..]),
        }
    }
}
