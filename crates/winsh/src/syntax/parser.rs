use super::{
    ParseError, ParseErrorKind, PipeKind, Pipeline, Quote, Redirection, RedirectionKind,
    SimpleCommand, Word, WordPart, WordPartKind,
};

/// Parse one simple command, or `None` for blank/comment-only input.
///
/// Supports words and redirections. Use parse_pipeline for pipe operators.
pub fn parse_simple_command(source: &str) -> Result<Option<SimpleCommand>, ParseError> {
    let Some(mut pipeline) = parse_pipeline(source)? else {
        return Ok(None);
    };
    if pipeline.commands.len() != 1 {
        return Err(ParseError {
            kind: ParseErrorKind::UnsupportedSyntax("pipeline in simple-command parser"),
            span: pipeline.span,
        });
    }
    Ok(pipeline.commands.pop())
}

/// Parse one foreground pipeline, retaining quote contexts and source ranges.
pub fn parse_pipeline(source: &str) -> Result<Option<Pipeline>, ParseError> {
    if let Some(start) = source.find('\0') {
        return Err(ParseError {
            kind: ParseErrorKind::NullByte,
            span: start..start + 1,
        });
    }

    Parser {
        source,
        pos: 0,
        nesting: 0,
    }
    .parse()
}

#[derive(Clone, Copy)]
struct Parser<'a> {
    source: &'a str,
    pos: usize,
    nesting: usize,
}

#[path = "completion.rs"]
pub(crate) mod completion;
#[path = "language_parser.rs"]
mod language;
pub(crate) use language::parse_fragment;
pub use language::{parse_script, valid_name};

impl Parser<'_> {
    fn parse(mut self) -> Result<Option<Pipeline>, ParseError> {
        self.skip_trivia();
        let start = self.pos;
        if self.peek().is_none() {
            return Ok(None);
        }
        let mut commands = vec![self.command()?];
        let mut connections = Vec::new();
        while self.peek() == Some('|') {
            let pipe_start = self.pos;
            self.pos += 1;
            let kind = match self.peek() {
                Some('|') => {
                    return Err(self.error(
                        ParseErrorKind::UnsupportedSyntax("command list"),
                        pipe_start,
                        self.pos + 1 - pipe_start,
                    ));
                }
                Some('&') => {
                    self.pos += 1;
                    PipeKind::StdoutAndStderr
                }
                _ => PipeKind::Stdout,
            };
            let pipe_end = self.pos;
            self.skip_trivia();
            if self.peek().is_none() {
                return Err(self.error(
                    ParseErrorKind::MissingCommand,
                    pipe_start,
                    pipe_end - pipe_start,
                ));
            }
            connections.push(kind);
            commands.push(self.command()?);
        }
        let end = commands.last().unwrap().span.end;
        self.skip_trivia();
        if let Some(ch) = self.peek() {
            return Err(self.error(ParseErrorKind::MultipleCommands, self.pos, ch.len_utf8()));
        }
        Ok(Some(Pipeline {
            commands,
            connections,
            span: start..end,
        }))
    }

    fn command(&mut self) -> Result<SimpleCommand, ParseError> {
        self.skip_spaces();
        let start = self.pos;
        let mut words = Vec::new();
        let mut redirections = Vec::new();
        let mut end = start;

        while let Some(ch) = self.peek() {
            match ch {
                ' ' | '\t' => self.pos += 1,
                '\n' | '#' | '|' | ';' | ')' => break,
                '<' | '>' => redirections.push(self.redirection(None, self.pos)?),
                '&' if self.following() == Some('>') => {
                    redirections.push(self.redirection(None, self.pos)?)
                }
                '&' if self.following() == Some('&') => break,
                '&' | '(' => {
                    return Err(self.error(
                        ParseErrorKind::UnsupportedSyntax("shell operator"),
                        self.pos,
                        1,
                    ));
                }
                _ => {
                    let word = self.word()?;
                    let descriptor = match &word.parts[..] {
                        [
                            WordPart {
                                kind: WordPartKind::Text(text),
                                ..
                            },
                        ] if !text.is_empty()
                            && text.bytes().all(|b| b.is_ascii_digit())
                            && matches!(self.peek(), Some('<' | '>')) =>
                        {
                            Some(text.parse::<u8>().ok().filter(|fd| *fd <= 2).ok_or_else(
                                || {
                                    self.error(
                                        ParseErrorKind::UnsupportedSyntax(
                                            "descriptors other than 0, 1, and 2",
                                        ),
                                        word.span.start,
                                        word.span.len(),
                                    )
                                },
                            )?)
                        }
                        _ => None,
                    };
                    if let Some(descriptor) = descriptor {
                        redirections.push(self.redirection(Some(descriptor), word.span.start)?);
                    } else {
                        words.push(word);
                    }
                }
            }
            if !matches!(ch, ' ' | '\t') {
                end = self.pos;
            }
        }
        if words.is_empty() && redirections.is_empty() {
            let length = self.peek().map_or(1, char::len_utf8);
            return Err(self.error(
                ParseErrorKind::UnsupportedSyntax("empty pipeline stage"),
                self.pos,
                length,
            ));
        }
        Ok(SimpleCommand {
            words,
            redirections,
            span: start..end,
        })
    }

    fn redirection(
        &mut self,
        descriptor: Option<u8>,
        start: usize,
    ) -> Result<Redirection, ParseError> {
        let kind = self.redirection_kind(start)?;
        let operator_end = self.pos;
        self.skip_spaces();
        if self.peek() == Some('(') {
            return Err(self.error(
                ParseErrorKind::UnsupportedSyntax("process substitution"),
                start,
                self.pos + 1 - start,
            ));
        }
        if matches!(
            self.peek(),
            None | Some('\n' | '#' | '|' | '<' | '>' | '&' | ';' | '(' | ')')
        ) {
            return Err(self.error(
                ParseErrorKind::MissingRedirectionTarget,
                start,
                operator_end - start,
            ));
        }
        let target = self.word()?;
        let end = target.span.end;
        Ok(Redirection {
            descriptor,
            kind,
            target,
            span: start..end,
        })
    }

    fn redirection_kind(&mut self, start: usize) -> Result<RedirectionKind, ParseError> {
        let first = self.peek().unwrap();
        self.pos += 1;
        let kind = match first {
            '&' => {
                self.peek(); // remove continuations between & and >
                self.pos += 1; // the > in &>
                if self.peek() == Some('>') {
                    self.pos += 1;
                    RedirectionKind::AppendAndError
                } else {
                    RedirectionKind::OutputAndError
                }
            }
            '<' => match self.peek() {
                Some('<') => {
                    return Err(self.error(
                        ParseErrorKind::UnsupportedSyntax("here-document or here-string"),
                        start,
                        self.pos + 1 - start,
                    ));
                }
                Some('>') => {
                    self.pos += 1;
                    RedirectionKind::ReadWrite
                }
                Some('&') => {
                    self.pos += 1;
                    RedirectionKind::DuplicateInput
                }
                _ => RedirectionKind::Input,
            },
            '>' => match self.peek() {
                Some('>') => {
                    self.pos += 1;
                    RedirectionKind::Append
                }
                Some('&') => {
                    self.pos += 1;
                    RedirectionKind::DuplicateOutput
                }
                Some('|') => {
                    self.pos += 1;
                    RedirectionKind::Clobber
                }
                _ => RedirectionKind::Output,
            },
            _ => unreachable!(),
        };
        Ok(kind)
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.pos += 1;
        }
    }

    fn following(&self) -> Option<char> {
        Parser {
            source: self.source,
            pos: self.pos + 1,
            nesting: self.nesting,
        }
        .peek()
    }

    fn word(&mut self) -> Result<Word, ParseError> {
        let start = self.pos;
        let mut parts = Vec::new();

        while let Some(ch) = self.peek() {
            let part_start = self.pos;
            match ch {
                ' ' | '\t' | '\n' | '&' | '|' | '<' | '>' | ';' | '(' | ')' => break,
                '\'' => parts.push(self.single_quoted()?),
                '"' => parts.push(self.double_quoted()?),
                '\\' => self.escape(&mut parts, false)?,
                '$' => self.dollar(&mut parts, false)?,
                '`' => return Err(self.backtick_error()),
                _ => {
                    self.pos += ch.len_utf8();
                    self.text(&mut parts, ch, part_start);
                }
            }
        }

        Ok(Word {
            parts,
            span: start..self.pos,
        })
    }

    fn single_quoted(&mut self) -> Result<WordPart, ParseError> {
        let start = self.pos;
        self.pos += 1;
        let content_start = self.pos;
        // No escaping or continuation removal takes place inside single quotes.
        let Some(length) = self.source[content_start..].find('\'') else {
            return Err(self.unclosed_quote(Quote::Single, start));
        };
        self.pos += length + 1;
        Ok(WordPart {
            kind: WordPartKind::SingleQuoted(self.source[content_start..self.pos - 1].to_owned()),
            span: start..self.pos,
        })
    }

    fn double_quoted(&mut self) -> Result<WordPart, ParseError> {
        let start = self.pos;
        self.pos += 1;
        let mut parts = Vec::new();

        while let Some(ch) = self.peek() {
            let part_start = self.pos;
            match ch {
                '"' => {
                    self.pos += 1;
                    return Ok(WordPart {
                        kind: WordPartKind::DoubleQuoted(parts),
                        span: start..self.pos,
                    });
                }
                '\\' => self.escape(&mut parts, true)?,
                '$' => self.dollar(&mut parts, true)?,
                '`' => return Err(self.backtick_error()),
                _ => {
                    self.pos += ch.len_utf8();
                    self.text(&mut parts, ch, part_start);
                }
            }
        }

        Err(self.unclosed_quote(Quote::Double, start))
    }

    fn escape(&mut self, parts: &mut Vec<WordPart>, double: bool) -> Result<(), ParseError> {
        let start = self.pos;
        self.pos += 1;
        // Read the raw next character: an escaped backslash must not introduce
        // a continuation itself. peek() already removed unescaped continuations.
        let Some(ch) = self.source[self.pos..].chars().next() else {
            return Err(self.error(ParseErrorKind::TrailingEscape, start, 1));
        };

        if double && !matches!(ch, '$' | '`' | '"' | '\\') {
            self.text(parts, '\\', start);
        } else {
            self.pos += ch.len_utf8();
            parts.push(WordPart {
                kind: WordPartKind::Escaped(ch),
                span: start..self.pos,
            });
        }
        Ok(())
    }

    fn dollar(&mut self, parts: &mut Vec<WordPart>, double: bool) -> Result<(), ParseError> {
        let start = self.pos;
        self.pos += 1;
        if matches!(self.peek(), Some('(' | '{')) {
            let open = self.peek().unwrap();
            self.pos += 1;
            let arithmetic = open == '(' && self.peek() == Some('(');
            if arithmetic {
                self.pos += 1;
            }
            let content = self.pos;
            let close = if open == '(' { ')' } else { '}' };
            let mut depth = 1;
            let mut quote = None;
            while let Some(ch) = self.source[self.pos..].chars().next() {
                if ch == '\\' && quote != Some('\'') {
                    self.pos += 1;
                    if let Some(next) = self.source[self.pos..].chars().next() {
                        self.pos += next.len_utf8();
                    }
                    continue;
                }
                if let Some(q) = quote {
                    if ch == q {
                        quote = None;
                    }
                } else if matches!(ch, '\'' | '"') {
                    quote = Some(ch);
                } else if ch == open {
                    depth += 1;
                    if depth > 64 {
                        return Err(self.error(
                            ParseErrorKind::UnsupportedSyntax("expansion nesting beyond 64 levels"),
                            start,
                            self.pos - start,
                        ));
                    }
                } else if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        let value = self.source[content..self.pos].to_owned();
                        self.pos += 1;
                        if arithmetic {
                            if self.peek() != Some(')') {
                                return Err(self.error(
                                    ParseErrorKind::Expected("))"),
                                    start,
                                    self.pos - start,
                                ));
                            }
                            self.pos += 1;
                        }
                        if open == '(' && !arithmetic {
                            language::parse_nested(&value, self.nesting + 1).map_err(|mut e| {
                                e.span.start += content;
                                e.span.end += content;
                                e
                            })?;
                        }
                        parts.push(WordPart {
                            kind: if arithmetic {
                                WordPartKind::Arithmetic(value)
                            } else if open == '(' {
                                WordPartKind::CommandSubstitution(value)
                            } else {
                                WordPartKind::Parameter(value)
                            },
                            span: start..self.pos,
                        });
                        return Ok(());
                    }
                }
                self.pos += ch.len_utf8();
            }
            return Err(self.error(
                ParseErrorKind::Expected(if open == '(' { ")" } else { "}" }),
                start,
                self.pos - start,
            ));
        }
        let feature = match self.peek() {
            Some('[') => Some("legacy arithmetic expansion"),
            Some('\'' | '"') if !double => Some("dollar-prefixed quoting"),
            _ => None,
        };
        if let Some(feature) = feature {
            return Err(self.error(
                ParseErrorKind::UnsupportedSyntax(feature),
                start,
                self.pos + 1 - start,
            ));
        }
        self.text(parts, '$', start);
        Ok(())
    }

    fn text(&self, parts: &mut Vec<WordPart>, ch: char, start: usize) {
        if let Some(WordPart {
            kind: WordPartKind::Text(text),
            span,
        }) = parts.last_mut()
        {
            text.push(ch);
            span.end = self.pos;
        } else {
            parts.push(WordPart {
                kind: WordPartKind::Text(ch.to_string()),
                span: start..self.pos,
            });
        }
    }

    /// Remove continuations before recognizing the next token. This also handles
    /// comments and multi-character syntax introduced across physical lines.
    /// Single quotes, comment bodies, and escaped characters bypass this method.
    fn peek(&mut self) -> Option<char> {
        while self.source[self.pos..].starts_with("\\\n") {
            self.pos += 2;
        }
        self.source[self.pos..].chars().next()
    }

    fn skip_trivia(&mut self) {
        while let Some(ch) = self.peek() {
            match ch {
                ' ' | '\t' | '\n' => self.pos += 1,
                '#' => {
                    let rest = &self.source[self.pos..];
                    self.pos += rest.find('\n').unwrap_or(rest.len());
                }
                _ => break,
            }
        }
    }

    fn error(&self, kind: ParseErrorKind, start: usize, length: usize) -> ParseError {
        ParseError {
            kind,
            span: start..start + length,
        }
    }

    fn unclosed_quote(&self, quote: Quote, start: usize) -> ParseError {
        self.error(ParseErrorKind::UnclosedQuote(quote), start, 1)
    }

    fn backtick_error(&self) -> ParseError {
        self.error(
            ParseErrorKind::UnsupportedSyntax("backtick command substitution"),
            self.pos,
            1,
        )
    }
}
