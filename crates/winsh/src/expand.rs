use crate::{
    Shell, ShellIo,
    syntax::{Word, WordPart, WordPartKind},
};
use std::{
    ffi::{OsStr, OsString},
    fmt,
    ops::Range,
};

#[derive(Debug)]
pub(crate) struct ExpansionError {
    pub message: String,
    pub span: Range<usize>,
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
pub(crate) fn error(word: &Word, message: impl Into<String>) -> ExpansionError {
    ExpansionError {
        message: message.into(),
        span: word.span.clone(),
    }
}

#[derive(Default)]
struct Field {
    value: OsString,
    pattern: String,
    wildcard: bool,
    keep: bool,
}
struct Builder {
    fields: Vec<Field>,
    split: bool,
}
impl Builder {
    fn current(&mut self) -> &mut Field {
        self.fields.last_mut().unwrap()
    }
    fn literal(&mut self, value: &OsStr, glob: bool) {
        let field = self.current();
        field.keep = true;
        field.value.push(value);
        for ch in value.to_string_lossy().chars() {
            if glob && matches!(ch, '*' | '?' | '[') {
                field.wildcard = true;
            }
            if !glob && matches!(ch, '*' | '?' | '[' | ']' | '\\') {
                field.pattern.push('\\');
            }
            field.pattern.push(ch);
        }
    }
    fn expanded(&mut self, value: &OsStr, quoted: bool, shell: &Shell) {
        if quoted || !self.split {
            self.literal(value, false);
            return;
        }
        let ifs = shell
            .var("IFS")
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| " \t\n".into());
        if ifs.is_empty() {
            self.literal(value, true);
            return;
        }
        for part in value.to_string_lossy().split_inclusive(|c| ifs.contains(c)) {
            let last = part.chars().last().unwrap();
            let delimiter = ifs.contains(last);
            let content = if delimiter {
                &part[..part.len() - last.len_utf8()]
            } else {
                part
            };
            if !content.is_empty() {
                self.literal(OsStr::new(content), true);
            }
            if delimiter && (self.current().keep || !last.is_ascii_whitespace()) {
                if !last.is_ascii_whitespace() {
                    self.current().keep = true;
                }
                self.fields.push(Field::default());
            }
        }
    }
}

pub(crate) fn fields(
    word: &Word,
    shell: &mut Shell,
    io: &mut ShellIo,
    split: bool,
) -> Result<Vec<OsString>, ExpansionError> {
    let mut builder = Builder {
        fields: vec![Field::default()],
        split,
    };
    for (index, part) in word.parts.iter().enumerate() {
        if index == 0
            && let WordPartKind::Text(text) = &part.kind
            && text.starts_with('~')
        {
            if (text == "~" && word.parts.len() == 1) || text.starts_with("~/") {
                let home = shell
                    .home()
                    .ok_or_else(|| error(word, "HOME is not set"))?
                    .to_owned();
                builder.literal(&home, false);
                text_part(&text[1..], false, part, shell, &mut builder)?;
                continue;
            }
            if text != "~" {
                return Err(error(word, "named-user tilde expansion is not supported"));
            }
        }
        append(part, false, shell, io, &mut builder)?;
    }
    let mut result = Vec::new();
    for field in builder.fields {
        if !field.keep {
            continue;
        }
        if split && field.wildcard {
            let matches = crate::glob::paths(shell.cwd(), &field.pattern);
            if !matches.is_empty() {
                result.extend(matches);
                continue;
            }
        }
        result.push(field.value);
    }
    if !split && result.is_empty() {
        result.push(OsString::new());
    }
    Ok(result)
}
pub(crate) fn scalar(
    word: &Word,
    shell: &mut Shell,
    io: &mut ShellIo,
) -> Result<OsString, ExpansionError> {
    Ok(fields(word, shell, io, false)?.pop().unwrap_or_default())
}
fn append(
    part: &WordPart,
    quoted: bool,
    shell: &mut Shell,
    io: &mut ShellIo,
    builder: &mut Builder,
) -> Result<(), ExpansionError> {
    match &part.kind {
        WordPartKind::Text(text) => text_part(text, quoted, part, shell, builder)?,
        WordPartKind::SingleQuoted(text) => builder.literal(text.as_ref(), false),
        WordPartKind::Escaped(ch) => builder.literal(ch.to_string().as_ref(), false),
        WordPartKind::DoubleQuoted(parts) => {
            if parts.is_empty() {
                builder.literal("".as_ref(), false);
            }
            for part in parts {
                append(part, true, shell, io, builder)?;
            }
        }
        WordPartKind::CommandSubstitution(source) => {
            let (value, status) = shell.capture(source, io).map_err(|e| ExpansionError {
                message: e.to_string(),
                span: part.span.clone(),
            })?;
            shell.substitution_status = Some(status);
            builder.expanded(&value, quoted, shell);
        }
        WordPartKind::Arithmetic(expression) => {
            let word =
                crate::syntax::parse_fragment(expression).map_err(|error| ExpansionError {
                    message: error.to_string(),
                    span: part.span.clone(),
                })?;
            let expanded = scalar(&word, shell, io)?;
            let value = crate::arithmetic::evaluate(&expanded.to_string_lossy(), shell).map_err(
                |message| ExpansionError {
                    message,
                    span: part.span.clone(),
                },
            )?;
            builder.expanded(value.to_string().as_ref(), quoted, shell);
        }
        WordPartKind::Parameter(expression) => {
            if expression == "@" {
                positional(quoted, shell, builder);
            } else {
                let value = parameter(expression, part, shell, io)?;
                builder.expanded(&value, quoted, shell);
            }
        }
    }
    Ok(())
}
fn positional(quoted: bool, shell: &Shell, builder: &mut Builder) {
    if quoted && builder.split {
        for (index, arg) in shell.positional.iter().enumerate() {
            if index > 0 {
                builder.fields.push(Field::default());
            }
            builder.literal(arg, false);
        }
    } else {
        builder.expanded(&shell.parameter("*"), quoted, shell);
    }
}
fn text_part(
    text: &str,
    quoted: bool,
    part: &WordPart,
    shell: &mut Shell,
    builder: &mut Builder,
) -> Result<(), ExpansionError> {
    if !quoted && text.contains('{') && (text.contains(',') || text.contains("..")) {
        return Err(ExpansionError {
            message: "brace expansion is not supported".into(),
            span: part.span.clone(),
        });
    }
    let mut pos = 0;
    while let Some(offset) = text[pos..].find('$') {
        let start = pos + offset;
        if start > pos {
            builder.literal(text[pos..start].as_ref(), !quoted);
        }
        pos = start + 1;
        let rest = &text[pos..];
        let length = if rest.starts_with(|ch: char| ch == '_' || ch.is_ascii_alphabetic()) {
            rest.bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count()
        } else if rest.starts_with(|ch: char| ch.is_ascii_digit() || "@*#?$!-".contains(ch)) {
            1
        } else {
            0
        };
        if length == 0 {
            builder.literal("$".as_ref(), false);
            continue;
        }
        let name = &rest[..length];
        if name == "@" {
            positional(quoted, shell, builder);
        } else if name == "-" {
            return Err(ExpansionError {
                message: format!("${name} is not supported"),
                span: part.span.clone(),
            });
        } else {
            builder.expanded(&shell.parameter(name), quoted, shell);
        }
        pos += length;
    }
    if pos < text.len() {
        builder.literal(text[pos..].as_ref(), !quoted);
    }
    Ok(())
}
fn parameter(
    expression: &str,
    part: &WordPart,
    shell: &mut Shell,
    io: &mut ShellIo,
) -> Result<OsString, ExpansionError> {
    let fail = |message: String| ExpansionError {
        message,
        span: part.span.clone(),
    };
    if let Some(name) = expression.strip_prefix('#')
        && !name.is_empty()
    {
        return Ok(if matches!(name, "@" | "*") {
            shell.positional.len()
        } else {
            shell.parameter(name).to_string_lossy().chars().count()
        }
        .to_string()
        .into());
    }
    let len = if expression.starts_with(|c: char| c.is_ascii_digit()) {
        expression.bytes().take_while(u8::is_ascii_digit).count()
    } else if expression.starts_with(|c: char| "@*#?$!".contains(c)) {
        1
    } else {
        expression
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count()
    };
    let name = &expression[..len];
    if name.is_empty() {
        return Err(fail("bad parameter expansion".into()));
    }
    let value = shell.parameter(name);
    let suffix = &expression[len..];
    if suffix.is_empty() {
        return Ok(value);
    }
    let (null, suffix) = suffix
        .strip_prefix(':')
        .map_or((false, suffix), |s| (true, s));
    let op = suffix.chars().next().unwrap_or('\0');
    if !matches!(op, '-' | '+' | '=' | '?') {
        return Err(fail("unsupported parameter operator".into()));
    }
    let set = shell.parameter_is_set(name) && (!null || !value.is_empty());
    let use_word = if op == '+' { set } else { !set };
    if !use_word {
        return Ok(if op == '+' { OsString::new() } else { value });
    }
    let fallback = &suffix[1..];
    let word = crate::syntax::parse_fragment(fallback).map_err(|e| fail(e.to_string()))?;
    let expanded = scalar(&word, shell, io)?;
    if op == '?' {
        return Err(fail(format!(
            "{name}: {}",
            if expanded.is_empty() {
                "parameter is unset or empty".into()
            } else {
                expanded.to_string_lossy().into_owned()
            }
        )));
    }
    if op == '=' {
        if !crate::syntax::valid_name(name) {
            return Err(fail("cannot assign to special parameter".into()));
        }
        shell.set_var(name, expanded.clone());
    }
    Ok(expanded)
}
