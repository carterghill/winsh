use std::{ffi::OsString, fmt, ops::Range};

use crate::{
    Shell,
    syntax::{SimpleCommand, Word, WordPart, WordPartKind},
};

pub(crate) struct ExpansionError {
    message: &'static str,
    span: Range<usize>,
}

impl fmt::Display for ExpansionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at bytes {}..{}",
            self.message, self.span.start, self.span.end
        )
    }
}

/// The first expansion stage supports literals and a leading ~/ or bare ~.
/// Refuse pending expansion syntax rather than silently executing it literally.
pub(crate) fn arguments(
    command: &SimpleCommand,
    shell: &Shell,
) -> Result<Vec<OsString>, ExpansionError> {
    command.words.iter().map(|item| word(item, shell)).collect()
}

pub(crate) fn word(word: &Word, shell: &Shell) -> Result<OsString, ExpansionError> {
    let mut result = OsString::new();
    for (index, part) in word.parts.iter().enumerate() {
        if index == 0
            && let WordPartKind::Text(text) = &part.kind
            && text.starts_with('~')
        {
            let rest = if let Some(rest) = text.strip_prefix("~/") {
                Some(format!("/{rest}"))
            } else if text == "~" && word.parts.len() == 1 {
                Some(String::new())
            } else {
                None
            };
            if let Some(rest) = rest {
                let home = shell
                    .home()
                    .ok_or_else(|| unsupported(part, "HOME is not set"))?;
                check_text(&rest, false, part)?;
                result.push(home);
                result.push(rest);
                continue;
            }
            // Quoting a tilde prefix inhibits its expansion.
            if text != "~" {
                return Err(unsupported(
                    part,
                    "named-user tilde expansion is not supported yet",
                ));
            }
        }
        append(part, false, &mut result)?;
    }
    Ok(result)
}

fn append(part: &WordPart, quoted: bool, result: &mut OsString) -> Result<(), ExpansionError> {
    match &part.kind {
        WordPartKind::Text(text) => {
            check_text(text, quoted, part)?;
            result.push(text);
        }
        WordPartKind::SingleQuoted(text) => result.push(text),
        WordPartKind::Escaped(ch) => result.push(ch.to_string()),
        WordPartKind::DoubleQuoted(parts) => {
            for part in parts {
                append(part, true, result)?;
            }
        }
    }
    Ok(())
}

fn check_text(text: &str, quoted: bool, part: &WordPart) -> Result<(), ExpansionError> {
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$'
            && chars
                .peek()
                .is_some_and(|next| next.is_ascii_alphanumeric() || "_?$#!@*-{([".contains(*next))
        {
            return Err(unsupported(
                part,
                "parameter expansion is not supported yet",
            ));
        }
        if !quoted && matches!(ch, '*' | '?' | '[' | '{' | '}') {
            return Err(unsupported(
                part,
                "wildcard and brace expansion is not supported yet",
            ));
        }
    }
    Ok(())
}

fn unsupported(part: &WordPart, message: &'static str) -> ExpansionError {
    ExpansionError {
        message,
        span: part.span.clone(),
    }
}
