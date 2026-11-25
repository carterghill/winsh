use super::*;
use crate::syntax::{Command, CommandKind, HereDocument, Script};
use std::collections::HashMap;

struct Pending {
    header: std::ops::Range<usize>,
    delimiter: String,
    quoted: bool,
    strip_tabs: bool,
}

/// Several redirects, even on different commands, can share a header line.
/// The next grammar newline consumes their bodies in declaration order.
#[derive(Default)]
pub(super) struct Documents {
    pending: Vec<Pending>,
    bodies: HashMap<usize, HereDocument>,
}

impl Documents {
    pub(super) fn queue(&mut self, source: &str, word: &Word, kind: RedirectionKind, start: usize) {
        fn unquote(source: &str, parts: &[WordPart], delimiter: &mut String) {
            for part in parts {
                match &part.kind {
                    WordPartKind::Text(s) | WordPartKind::SingleQuoted(s) => delimiter.push_str(s),
                    WordPartKind::DoubleQuoted(parts) => unquote(source, parts, delimiter),
                    WordPartKind::Escaped(ch) => delimiter.push(*ch),
                    _ => delimiter.push_str(&source[part.span.clone()]),
                }
            }
        }
        let mut delimiter = String::new();
        unquote(source, &word.parts, &mut delimiter);
        let quoted = word.parts.iter().any(|p| {
            matches!(
                p.kind,
                WordPartKind::DoubleQuoted(_)
                    | WordPartKind::SingleQuoted(_)
                    | WordPartKind::Escaped(_)
            )
        });
        self.pending.push(Pending {
            header: start..word.span.end,
            delimiter,
            quoted,
            strip_tabs: kind == RedirectionKind::HereDocumentStripTabs,
        });
    }

    pub(super) fn check_complete(&self) -> Result<(), ParseError> {
        match self.pending.first() {
            Some(doc) => Err(doc.missing()),
            None => Ok(()),
        }
    }

    pub(super) fn collect(&mut self, source: &str, pos: &mut usize) -> Result<(), ParseError> {
        for doc in self.pending.drain(..) {
            let mut body = String::new();
            let mut line = String::new();
            loop {
                if *pos == source.len() {
                    return Err(doc.missing());
                }
                let rest = &source[*pos..];
                let end = rest.find('\n').unwrap_or(rest.len());
                let newline = end < rest.len();
                let physical = &rest[..end];
                *pos += end + usize::from(newline);
                line.push_str(physical);
                if !doc.quoted
                    && newline
                    && physical.bytes().rev().take_while(|b| *b == b'\\').count() % 2 == 1
                {
                    line.pop();
                    continue;
                }
                let content = if doc.strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    &line
                };
                if content == doc.delimiter {
                    break;
                }
                body.push_str(content);
                if newline {
                    body.push('\n');
                }
                line.clear();
            }
            self.bodies.insert(
                doc.header.start,
                HereDocument {
                    body,
                    quoted: doc.quoted,
                },
            );
        }
        Ok(())
    }

    pub(super) fn attach(&mut self, redirects: &mut [Redirection]) {
        for redirect in redirects {
            redirect.here_document = self.bodies.remove(&redirect.span.start);
        }
    }

    pub(super) fn attach_script(&mut self, script: &mut Script) {
        if self.bodies.is_empty() {
            return;
        }
        for list in &mut script.lists {
            for pipeline in &mut list.pipelines {
                for command in &mut pipeline.commands {
                    self.attach_command(command);
                }
            }
        }
    }

    fn attach_command(&mut self, command: &mut Command) {
        self.attach(&mut command.redirections);
        match &mut command.kind {
            CommandKind::Simple(simple) => self.attach(&mut simple.redirections),
            CommandKind::Group { body, .. } | CommandKind::For { body, .. } => {
                self.attach_script(body)
            }
            CommandKind::While {
                condition, body, ..
            } => {
                self.attach_script(condition);
                self.attach_script(body);
            }
            CommandKind::If {
                branches,
                otherwise,
            } => {
                for (condition, body) in branches {
                    self.attach_script(condition);
                    self.attach_script(body);
                }
                if let Some(body) = otherwise {
                    self.attach_script(body);
                }
            }
            CommandKind::Case { arms, .. } => {
                for arm in arms {
                    self.attach_script(&mut arm.body);
                }
            }
            CommandKind::Function { body, .. } => self.attach_command(body),
        }
    }
}

impl Pending {
    fn missing(&self) -> ParseError {
        ParseError {
            kind: ParseErrorKind::UnclosedHereDocument(self.delimiter.clone()),
            span: self.header.clone(),
        }
    }
}

/// Quotes are ordinary body text. Only dollar, backtick, backslash and newline
/// can be escaped, and the expanded result remains one string.
pub(crate) fn parse_document_body(source: &str) -> Result<Word, ParseError> {
    let mut parser = Parser::new(source, 0, 0);
    let mut parts = Vec::new();
    while let Some(ch) = parser.peek() {
        let start = parser.pos;
        match ch {
            '$' => parser.dollar(&mut parts, true)?,
            '`' => return Err(parser.backtick_error()),
            '\\' if matches!(source[start + 1..].chars().next(), Some('$' | '`' | '\\')) => {
                parser.escape(&mut parts, true)?;
            }
            _ => {
                parser.pos += ch.len_utf8();
                parser.text(&mut parts, ch, start);
            }
        }
    }
    let span = 0..source.len();
    Ok(Word {
        parts: vec![WordPart {
            kind: WordPartKind::DoubleQuoted(parts),
            span: span.clone(),
        }],
        span,
    })
}
