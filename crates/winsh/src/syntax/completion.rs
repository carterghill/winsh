use super::{language::plain, *};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Command,
    Path,
    Directory,
}

/// Read up to the cursor. The surrounding script can still be incomplete.
/// Use the shell word parser so quotes, escapes and substitutions consume
/// exactly the same source as they would during execution.
pub(crate) fn completion_word(source: &str) -> Option<(Target, Word, Option<Quote>)> {
    let mut parser = Parser::new(source, 0, 0);
    let mut target = Target::Command;
    let mut redirect = None;
    loop {
        parser.skip_spaces();
        let next = parser.peek();
        match next {
            Some('#') => {
                parser.pos += source[parser.pos..].find('\n')?;
            }
            Some('<' | '>') => redirect = Some(parser.redirection_kind(parser.pos).ok()?),
            Some('&') if parser.following() == Some('>') => {
                redirect = Some(parser.redirection_kind(parser.pos).ok()?);
            }
            Some('\n' | ';' | '|' | '&' | '(' | ')') => {
                parser.pos += 1;
                target = Target::Command;
                redirect = None;
            }
            _ => {
                let start = parser.pos;
                let (word, quote) = match parser.word() {
                    Ok(word) => (word, None),
                    Err(ParseError {
                        kind: ParseErrorKind::UnclosedQuote(quote),
                        ..
                    }) => {
                        let close = match quote {
                            Quote::Single => '\'',
                            Quote::Double => '"',
                        };
                        let mut closed = source[start..].to_owned();
                        closed.push(close);
                        let word = language::parse_fragment(&closed).ok()?;
                        (word, Some(quote))
                    }
                    Err(_) => return None,
                };
                if quote.is_some() || parser.pos == source.len() {
                    let target = match redirect {
                        Some(
                            RedirectionKind::DuplicateInput
                            | RedirectionKind::DuplicateOutput
                            | RedirectionKind::HereDocument
                            | RedirectionKind::HereDocumentStripTabs,
                        ) => return None,
                        Some(_) => Target::Path,
                        None => target,
                    };
                    return Some((target, word, quote));
                }
                if redirect.take().is_some() || target != Target::Command {
                    continue;
                }
                let raw = &source[start..parser.pos];
                let descriptor = matches!(parser.peek(), Some('<' | '>'))
                    && raw.bytes().all(|b| b.is_ascii_digit());
                let assignment = raw.split_once('=').is_some_and(|(name, _)| {
                    language::valid_name(name.strip_suffix('+').unwrap_or(name))
                });
                if !assignment && !descriptor {
                    target = match plain(&word) {
                        Some(
                            "if" | "then" | "elif" | "else" | "while" | "until" | "do" | "!" | "{",
                        ) => Target::Command,
                        Some("cd") => Target::Directory,
                        _ => Target::Path,
                    };
                }
            }
        }
    }
}
