use super::{ParseError, ParseErrorKind, Quote, SimpleCommand, Word, WordPart, WordPartKind};

/// Parse one simple command, or `None` for blank/comment-only input.
///
/// Supports ordinary words, single/double quotes, escapes, line continuations,
/// and comments. Leading/trailing blank lines are accepted, but a second command
/// is rejected. Operators, compound substitutions, special dollar quoting,
/// assignment prefixes, and reserved words in command position are not supported.
/// Unexpanded words are syntax, not a ready-to-execute argument vector.
pub fn parse_simple_command(source: &str) -> Result<Option<SimpleCommand>, ParseError> {
    if let Some(start) = source.find('\0') {
        return Err(ParseError {
            kind: ParseErrorKind::NullByte,
            span: start..start + 1,
        });
    }

    Parser { source, pos: 0 }.parse()
}

struct Parser<'a> {
    source: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn parse(mut self) -> Result<Option<SimpleCommand>, ParseError> {
        self.skip_trivia();
        let start = self.pos;
        let mut words = Vec::new();

        while let Some(ch) = self.peek() {
            match ch {
                ' ' | '\t' => self.pos += 1,
                '\n' | '#' => break,
                _ => words.push(self.word()?),
            }
        }

        let Some(last) = words.last() else {
            return Ok(None);
        };
        let end = last.span.end;

        self.skip_trivia();
        if let Some(ch) = self.peek() {
            return Err(self.error(ParseErrorKind::MultipleCommands, self.pos, ch.len_utf8()));
        }

        check_command_word(&words[0])?;
        Ok(Some(SimpleCommand {
            words,
            span: start..end,
        }))
    }

    fn word(&mut self) -> Result<Word, ParseError> {
        let start = self.pos;
        let mut parts = Vec::new();

        while let Some(ch) = self.peek() {
            let part_start = self.pos;
            match ch {
                ' ' | '\t' | '\n' => break,
                ';' | '&' | '|' | '<' | '>' | '(' | ')' => {
                    return Err(self.error(
                        ParseErrorKind::UnsupportedSyntax("shell operator"),
                        self.pos,
                        1,
                    ));
                }
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
        let feature = match self.peek() {
            Some('(') => Some("command or arithmetic substitution"),
            Some('{') => Some("braced parameter expansion"),
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

fn check_command_word(word: &Word) -> Result<(), ParseError> {
    const RESERVED_WORDS: &[&str] = &[
        "!", "{", "}", "[[", "]]", "if", "then", "elif", "else", "fi", "time", "for", "in",
        "until", "while", "do", "done", "case", "esac", "select", "function", "coproc",
    ];

    let WordPartKind::Text(first) = &word.parts[0].kind else {
        return Ok(());
    };
    let feature = if word.parts.len() == 1 && RESERVED_WORDS.contains(&first.as_str()) {
        Some("reserved word in command position")
    } else if first.split_once('=').is_some_and(|(name, _)| {
        let name = name.strip_suffix('+').unwrap_or(name);
        let name = if let Some(indexed) = name.strip_suffix(']') {
            indexed.split_once('[').map_or(name, |(name, _)| name)
        } else {
            name
        };
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
            && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    }) {
        Some("assignment prefix")
    } else {
        None
    };

    match feature {
        Some(feature) => Err(ParseError {
            kind: ParseErrorKind::UnsupportedSyntax(feature),
            span: word.span.clone(),
        }),
        None => Ok(()),
    }
}
