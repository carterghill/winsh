use super::*;
use crate::syntax::{AndOr, Command, CommandKind, ListOperator, Script, ScriptPipeline};

/// Parse a complete script before executing any part of it.
pub fn parse_script(source: &str) -> Result<Script, ParseError> {
    parse_nested(source, 0)
}
pub(super) fn parse_nested(source: &str, nesting: usize) -> Result<Script, ParseError> {
    if nesting > 32 {
        return Err(ParseError {
            kind: ParseErrorKind::UnsupportedSyntax(
                "command substitution nesting beyond 32 levels",
            ),
            span: 0..source.len(),
        });
    }
    if let Some(start) = source.find('\0') {
        return Err(ParseError {
            kind: ParseErrorKind::NullByte,
            span: start..start + 1,
        });
    }
    let mut parser = Parser {
        source,
        pos: 0,
        nesting,
    };
    let script = parser.list(&[], 0)?;
    parser.skip_trivia();
    if parser.peek().is_some() {
        return Err(parser.error(ParseErrorKind::Unexpected("token"), parser.pos, 1));
    }
    Ok(script)
}

impl Parser<'_> {
    fn keyword_end(&mut self, text: &str) -> Option<usize> {
        self.skip_spaces();
        let mut scan = *self;
        for ch in text.chars() {
            if scan.peek() != Some(ch) {
                return None;
            }
            scan.pos += ch.len_utf8();
        }
        scan.peek()
            .is_none_or(|ch| {
                matches!(
                    ch,
                    ' ' | '\t' | '\n' | ';' | '&' | '|' | '(' | ')' | '<' | '>'
                )
            })
            .then_some(scan.pos)
    }
    fn keyword(&mut self, text: &str) -> bool {
        self.keyword_end(text).is_some()
    }

    fn take_keyword(&mut self, text: &str) -> bool {
        if let Some(end) = self.keyword_end(text) {
            self.pos = end;
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, text: &'static str) -> Result<(), ParseError> {
        if self.take_keyword(text) {
            Ok(())
        } else {
            let end = self.peek().is_none();
            Err(self.error(
                if end {
                    ParseErrorKind::Expected(text)
                } else {
                    ParseErrorKind::Unexpected(text)
                },
                self.pos,
                0,
            ))
        }
    }

    fn list(&mut self, stops: &[&str], depth: usize) -> Result<Script, ParseError> {
        if depth > 64 {
            return Err(self.error(
                ParseErrorKind::UnsupportedSyntax("nesting beyond 64 levels"),
                self.pos,
                0,
            ));
        }
        let mut script = Script::default();
        self.skip_trivia();
        while self.peek().is_some() && !stops.iter().any(|stop| self.keyword(stop)) {
            let mut list = AndOr {
                pipelines: vec![self.language_pipeline(depth)?],
                operators: Vec::new(),
            };
            loop {
                self.skip_spaces();
                let op = if self.source[self.pos..].starts_with("&&") {
                    Some(ListOperator::And)
                } else if self.source[self.pos..].starts_with("||") {
                    Some(ListOperator::Or)
                } else {
                    None
                };
                let Some(op) = op else {
                    break;
                };
                self.pos += 2;
                self.skip_trivia();
                if self.peek().is_none() {
                    return Err(self.error(ParseErrorKind::MissingCommand, self.pos, 0));
                }
                list.operators.push(op);
                list.pipelines.push(self.language_pipeline(depth)?);
            }
            script.lists.push(list);
            self.skip_spaces();
            if self.peek() == Some('#') {
                self.skip_comment();
            }
            match self.peek() {
                Some(';') => {
                    self.pos += 1;
                    self.skip_trivia();
                }
                Some('\n') => self.skip_trivia(),
                None => break,
                Some(')') if stops.contains(&")") => break,
                Some('&') => {
                    return Err(self.error(
                        ParseErrorKind::UnsupportedSyntax("background commands"),
                        self.pos,
                        1,
                    ));
                }
                Some(ch) => {
                    return Err(self.error(
                        ParseErrorKind::Unexpected("token; expected a command separator"),
                        self.pos,
                        ch.len_utf8(),
                    ));
                }
            }
        }
        Ok(script)
    }

    fn skip_comment(&mut self) {
        let rest = &self.source[self.pos..];
        self.pos += rest.find('\n').unwrap_or(rest.len());
    }

    fn language_pipeline(&mut self, depth: usize) -> Result<ScriptPipeline, ParseError> {
        let negated = self.take_keyword("!");
        self.skip_trivia();
        let mut pipeline = ScriptPipeline {
            commands: vec![self.language_command(depth)?],
            connections: Vec::new(),
            negated,
        };
        loop {
            self.skip_spaces();
            if self.peek() != Some('|') || self.following() == Some('|') {
                break;
            }
            self.pos += 1;
            let connection = if self.peek() == Some('&') {
                self.pos += 1;
                PipeKind::StdoutAndStderr
            } else {
                PipeKind::Stdout
            };
            self.skip_trivia();
            if self.peek().is_none() {
                return Err(self.error(ParseErrorKind::MissingCommand, self.pos, 0));
            }
            pipeline.connections.push(connection);
            pipeline.commands.push(self.language_command(depth)?);
        }
        Ok(pipeline)
    }

    fn nonempty_list(&mut self, stops: &[&str], depth: usize) -> Result<Script, ParseError> {
        let script = self.list(stops, depth + 1)?;
        if script.lists.is_empty() {
            let end = self.peek().is_none();
            return Err(self.error(
                if end {
                    ParseErrorKind::Expected("command")
                } else {
                    ParseErrorKind::Unexpected("empty command list")
                },
                self.pos,
                0,
            ));
        }
        Ok(script)
    }

    fn language_command(&mut self, depth: usize) -> Result<Command, ParseError> {
        self.skip_spaces();
        let kind = if self.take_keyword("if") {
            let mut branches = Vec::new();
            loop {
                let condition = self.nonempty_list(&["then"], depth)?;
                self.expect_keyword("then")?;
                let body = self.nonempty_list(&["elif", "else", "fi"], depth)?;
                branches.push((condition, body));
                if !self.take_keyword("elif") {
                    break;
                }
            }
            let otherwise = if self.take_keyword("else") {
                Some(self.nonempty_list(&["fi"], depth)?)
            } else {
                None
            };
            self.expect_keyword("fi")?;
            CommandKind::If {
                branches,
                otherwise,
            }
        } else if self.keyword("while") || self.keyword("until") {
            let until = self.take_keyword("until");
            if !until {
                self.expect_keyword("while")?;
            }
            let condition = self.nonempty_list(&["do"], depth)?;
            self.expect_keyword("do")?;
            let body = self.nonempty_list(&["done"], depth)?;
            self.expect_keyword("done")?;
            CommandKind::While {
                condition,
                body,
                until,
            }
        } else if self.take_keyword("for") {
            self.skip_spaces();
            let name_word = self.word()?;
            let name = plain(&name_word)
                .filter(|name| valid_name(name))
                .ok_or_else(|| {
                    self.error(
                        ParseErrorKind::Unexpected("for variable name"),
                        name_word.span.start,
                        name_word.span.len(),
                    )
                })?
                .to_owned();
            self.skip_spaces();
            // Newlines may precede `in`, but a semicolon starts the `do` form.
            let mut separator = false;
            while self.peek() == Some('\n') {
                separator = true;
                self.pos += 1;
                self.skip_spaces();
            }
            let words = if self.take_keyword("in") {
                let mut words = Vec::new();
                self.skip_spaces();
                while !matches!(self.peek(), None | Some(';' | '\n' | '#')) {
                    if matches!(self.peek(), Some('&' | '|' | '<' | '>' | '(' | ')')) {
                        return Err(self.error(
                            ParseErrorKind::Unexpected("for word"),
                            self.pos,
                            1,
                        ));
                    }
                    words.push(self.word()?);
                    self.skip_spaces();
                }
                Some(words)
            } else {
                None
            };
            if self.peek() == Some(';') {
                self.pos += 1;
                separator = true;
            } else if words.is_some() && !matches!(self.peek(), Some('\n' | '#') | None) {
                return Err(self.error(ParseErrorKind::Unexpected("for separator"), self.pos, 0));
            }
            if words.is_none() && !separator && self.peek().is_some() {
                return Err(self.error(ParseErrorKind::Unexpected("for separator"), self.pos, 0));
            }
            self.skip_trivia();
            self.expect_keyword("do")?;
            let body = self.nonempty_list(&["done"], depth)?;
            self.expect_keyword("done")?;
            CommandKind::For { name, words, body }
        } else if self.take_keyword("{") {
            let body = self.nonempty_list(&["}"], depth)?;
            self.expect_keyword("}")?;
            CommandKind::Group {
                body,
                subshell: false,
            }
        } else if self.peek() == Some('(') {
            self.pos += 1;
            let body = self.nonempty_list(&[")"], depth)?;
            self.expect_keyword(")")?;
            CommandKind::Group {
                body,
                subshell: true,
            }
        } else {
            for reserved in ["then", "elif", "else", "fi", "do", "done", "}", "in"] {
                if self.keyword(reserved) {
                    return Err(self.error(
                        ParseErrorKind::Unexpected("reserved word"),
                        self.pos,
                        reserved.len(),
                    ));
                }
            }
            for unsupported in ["case", "esac", "select", "function", "coproc", "[[", "time"] {
                if self.keyword(unsupported) {
                    return Err(self.error(
                        ParseErrorKind::UnsupportedSyntax(unsupported),
                        self.pos,
                        unsupported.len(),
                    ));
                }
            }
            let start = self.pos;
            let possible_name = self.word()?;
            if plain(&possible_name).is_some_and(|name| {
                name.split_once('=')
                    .is_some_and(|(key, _)| key.contains('['))
            }) {
                return Err(self.error(
                    ParseErrorKind::UnsupportedSyntax("array assignments"),
                    possible_name.span.start,
                    possible_name.span.len(),
                ));
            }
            self.skip_spaces();
            if plain(&possible_name).is_some_and(valid_name) && self.peek() == Some('(') {
                self.pos += 1;
                self.skip_spaces();
                self.expect_keyword(")")?;
                self.skip_trivia();
                if !self.keyword("{") && self.peek() != Some('(') {
                    return Err(self.error(ParseErrorKind::Expected("function body"), self.pos, 0));
                }
                if depth >= 64 {
                    return Err(self.error(
                        ParseErrorKind::UnsupportedSyntax("nesting beyond 64 levels"),
                        self.pos,
                        0,
                    ));
                }
                let body = self.language_command(depth + 1)?;
                CommandKind::Function {
                    name: plain(&possible_name).unwrap().to_owned(),
                    body: Box::new(body),
                }
            } else {
                self.pos = start;
                return Ok(Command {
                    kind: CommandKind::Simple(self.command()?),
                    redirections: Vec::new(),
                });
            }
        };
        let mut redirections = Vec::new();
        loop {
            self.skip_spaces();
            let start = self.pos;
            let mut descriptor = None;
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                let word = self.word()?;
                if matches!(self.peek(), Some('<' | '>')) {
                    descriptor = plain(&word)
                        .and_then(|s| s.parse::<u8>().ok())
                        .filter(|fd| *fd <= 2);
                    if descriptor.is_none() {
                        return Err(self.error(
                            ParseErrorKind::UnsupportedSyntax("descriptor"),
                            start,
                            word.span.len(),
                        ));
                    }
                } else {
                    self.pos = start;
                    break;
                }
            }
            if matches!(self.peek(), Some('<' | '>'))
                || (self.peek() == Some('&') && self.following() == Some('>'))
            {
                redirections.push(self.redirection(descriptor, start)?);
            } else {
                break;
            }
        }
        Ok(Command { kind, redirections })
    }
}

pub(super) fn plain(word: &Word) -> Option<&str> {
    match word.parts.as_slice() {
        [
            WordPart {
                kind: WordPartKind::Text(text),
                ..
            },
        ] => Some(text),
        _ => None,
    }
}

/// Parameter default words are fragments: spaces and operators are text here.
pub(crate) fn parse_fragment(source: &str) -> Result<Word, ParseError> {
    let mut parser = Parser {
        source,
        pos: 0,
        nesting: 0,
    };
    let mut parts = Vec::new();
    while let Some(ch) = parser.peek() {
        let start = parser.pos;
        match ch {
            '\'' => parts.push(parser.single_quoted()?),
            '"' => parts.push(parser.double_quoted()?),
            '\\' => parser.escape(&mut parts, false)?,
            '$' => parser.dollar(&mut parts, false)?,
            '`' => return Err(parser.backtick_error()),
            _ => {
                parser.pos += ch.len_utf8();
                parser.text(&mut parts, ch, start);
            }
        }
    }
    Ok(Word {
        parts,
        span: 0..source.len(),
    })
}

pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
