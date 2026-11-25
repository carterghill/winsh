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
    pub cause: ExpansionFailure,
    span: Range<usize>,
}
#[derive(Debug)]
pub(crate) enum ExpansionFailure {
    Invalid(String),
    Unbound(String),
}
impl From<String> for ExpansionFailure {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}
impl From<&str> for ExpansionFailure {
    fn from(message: &str) -> Self {
        Self::Invalid(message.into())
    }
}
impl ExpansionError {
    fn at(part: &WordPart, cause: impl Into<ExpansionFailure>) -> Self {
        Self {
            cause: cause.into(),
            span: part.span.clone(),
        }
    }
}
impl fmt::Display for ExpansionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.cause {
            ExpansionFailure::Invalid(message) => f.write_str(message)?,
            ExpansionFailure::Unbound(name) => write!(f, "{name}: unbound variable")?,
        }
        write!(f, " at bytes {}..{}", self.span.start, self.span.end)
    }
}
pub(crate) fn error(word: &Word, message: impl Into<String>) -> ExpansionError {
    ExpansionError {
        cause: ExpansionFailure::Invalid(message.into()),
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
    // Literal text inside an unquoted parameter operand is subject to IFS.
    parameter_operand: bool,
    document: Option<DocumentFields>,
}
#[derive(Default)]
struct DocumentFields {
    raw: OsString,
    expanded_at: bool,
}
impl Builder {
    fn new(split: bool) -> Self {
        Self {
            fields: vec![Field::default()],
            split,
            parameter_operand: false,
            document: None,
        }
    }
    fn current(&mut self) -> &mut Field {
        self.fields.last_mut().unwrap()
    }
    fn literal(&mut self, value: &OsStr, glob: bool) {
        if let Some(doc) = &mut self.document {
            doc.raw.push(value);
        }
        self.field_literal(value, glob);
    }
    fn field_literal(&mut self, value: &OsStr, glob: bool) {
        let field = self.current();
        field.keep = true;
        field.value.push(value);
        for ch in value.to_string_lossy().chars() {
            if glob && matches!(ch, '*' | '?' | '[') {
                field.wildcard = true;
            }
            if !glob && matches!(ch, '*' | '?' | '[' | ']' | '\\' | '-' | '!' | '^') {
                field.pattern.push('\\');
            }
            field.pattern.push(ch);
        }
    }
    fn expanded(&mut self, value: &OsStr, quoted: bool, shell: &Shell) {
        if quoted || !self.split {
            self.literal(value, !quoted);
            return;
        }
        if let Some(doc) = &mut self.document {
            doc.raw.push(value);
        }
        let ifs = shell
            .var("IFS")
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| " \t\n".into());
        if ifs.is_empty() {
            if !value.is_empty() {
                self.field_literal(value, true);
            }
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
                self.field_literal(OsStr::new(content), true);
            }
            if delimiter && (self.current().keep || !last.is_ascii_whitespace()) {
                if !last.is_ascii_whitespace() {
                    self.current().keep = true;
                }
                self.fields.push(Field::default());
            }
        }
    }
    fn text(&mut self, text: &str, quoted: bool, shell: &Shell) {
        if self.parameter_operand {
            self.expanded(text.as_ref(), quoted, shell);
        } else {
            self.literal(text.as_ref(), !quoted);
        }
    }
    fn parameter_value(&mut self, name: &str, value: &OsStr, quoted: bool, shell: &Shell) {
        self.expanded(
            value,
            quoted && !(name == "*" && self.document.is_some()),
            shell,
        );
    }
}

pub(crate) fn document(
    source: &str,
    target: &Word,
    shell: &mut Shell,
    io: &mut ShellIo,
) -> Result<OsString, ExpansionError> {
    let word =
        crate::syntax::parse_document_body(source).map_err(|e| error(target, e.to_string()))?;
    if !(source.contains("$*") || source.contains("${*"))
        || !(source.contains("$@") || source.contains("${@"))
    {
        return scalar(&word, shell, io);
    }
    // In a here-document Bash joins the fields of $@ with spaces. When the
    // same body also expands $*, its unquoted IFS boundaries participate in
    // that join. Keep both renderings so substitutions run only once.
    let mut builder = Builder::new(true);
    builder.document = Some(DocumentFields::default());
    append_word(&word, true, shell, io, &mut builder)?;
    let doc = builder.document.unwrap();
    if !doc.expanded_at {
        return Ok(doc.raw);
    }
    let values: Vec<_> = builder
        .fields
        .into_iter()
        .filter(|f| f.keep)
        .map(|f| f.value)
        .collect();
    Ok(values.join(OsStr::new(" ")))
}

pub(crate) fn fields(
    word: &Word,
    shell: &mut Shell,
    io: &mut ShellIo,
    split: bool,
) -> Result<Vec<OsString>, ExpansionError> {
    let builder = build_word(word, shell, io, split)?;
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

pub(crate) fn case_pattern(
    word: &Word,
    shell: &mut Shell,
    io: &mut ShellIo,
) -> Result<String, ExpansionError> {
    Ok(build_word(word, shell, io, false)?
        .fields
        .pop()
        .unwrap()
        .pattern)
}

fn build_word(
    word: &Word,
    shell: &mut Shell,
    io: &mut ShellIo,
    split: bool,
) -> Result<Builder, ExpansionError> {
    let mut builder = Builder::new(split);
    append_word(word, false, shell, io, &mut builder)?;
    Ok(builder)
}
fn append_word(
    word: &Word,
    quoted: bool,
    shell: &mut Shell,
    io: &mut ShellIo,
    builder: &mut Builder,
) -> Result<(), ExpansionError> {
    for (index, part) in word.parts.iter().enumerate() {
        if !quoted
            && index == 0
            && let WordPartKind::Text(text) = &part.kind
            && text.starts_with('~')
        {
            if (text == "~" && word.parts.len() == 1) || text.starts_with("~/") {
                let home = shell
                    .home()
                    .ok_or_else(|| error(word, "HOME is not set"))?
                    .to_owned();
                builder.literal(&home, false);
                text_part(&text[1..], false, part, shell, builder)?;
                continue;
            }
            if text != "~" {
                return Err(error(word, "named-user tilde expansion is not supported"));
            }
        }
        append(part, quoted, shell, io, builder)?;
    }
    Ok(())
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
            let (value, status) = shell
                .capture(source, io)
                .map_err(|e| ExpansionError::at(part, e.to_string()))?;
            shell.substitution_status = Some(status);
            builder.expanded(&value, quoted, shell);
        }
        WordPartKind::Arithmetic(expression) => {
            let word = crate::syntax::parse_fragment(expression)
                .map_err(|error| ExpansionError::at(part, error.to_string()))?;
            let expanded = scalar(&word, shell, io)?;
            let value = crate::arithmetic::evaluate(&expanded.to_string_lossy(), shell)
                .map_err(|cause| ExpansionError::at(part, cause))?;
            builder.expanded(value.to_string().as_ref(), quoted, shell);
        }
        WordPartKind::Parameter(expression) => {
            if expression == "@" {
                positional(quoted, shell, builder);
            } else {
                parameter(expression, part, quoted, shell, io, builder)?;
            }
        }
    }
    Ok(())
}
fn positional(quoted: bool, shell: &Shell, builder: &mut Builder) {
    if builder.split && (quoted || shell.var("IFS") == Some(OsStr::new(""))) {
        if let Some(doc) = &mut builder.document {
            doc.expanded_at = true;
        }
        for (index, arg) in shell.positional.iter().enumerate() {
            if index > 0 {
                builder.fields.push(Field::default());
                if let Some(doc) = &mut builder.document {
                    doc.raw.push(" ");
                }
            }
            builder.expanded(arg, quoted, shell);
        }
    } else {
        let name = if builder.split { "*" } else { "@" };
        builder.expanded(&shell.parameter(name), quoted, shell);
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
        return Err(ExpansionError::at(part, "brace expansion is not supported"));
    }
    let mut pos = 0;
    while let Some(offset) = text[pos..].find('$') {
        let start = pos + offset;
        if start > pos {
            builder.text(&text[pos..start], quoted, shell);
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
        } else {
            check_bound(shell, name, part)?;
            builder.parameter_value(name, &shell.parameter(name), quoted, shell);
        }
        pos += length;
    }
    if pos < text.len() {
        builder.text(&text[pos..], quoted, shell);
    }
    Ok(())
}
fn check_bound(shell: &Shell, name: &str, part: &WordPart) -> Result<(), ExpansionError> {
    if shell.options.nounset && !matches!(name, "@" | "*") && !shell.parameter_is_set(name) {
        return Err(ExpansionError::at(
            part,
            ExpansionFailure::Unbound(name.into()),
        ));
    }
    Ok(())
}
fn parameter(
    expression: &str,
    part: &WordPart,
    quoted: bool,
    shell: &mut Shell,
    io: &mut ShellIo,
    builder: &mut Builder,
) -> Result<(), ExpansionError> {
    let fail = |message: String| ExpansionError::at(part, message);
    if let Some(name) = expression.strip_prefix('#')
        && !name.is_empty()
    {
        check_bound(shell, name, part)?;
        let length = if matches!(name, "@" | "*") {
            shell.positional.len()
        } else {
            shell.parameter(name).to_string_lossy().chars().count()
        }
        .to_string();
        builder.expanded(length.as_ref(), quoted, shell);
        return Ok(());
    }
    let len = if expression.starts_with(|c: char| c.is_ascii_digit()) {
        expression.bytes().take_while(u8::is_ascii_digit).count()
    } else if expression.starts_with(|c: char| "@*#?$!-".contains(c)) {
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
        check_bound(shell, name, part)?;
        builder.parameter_value(name, &value, quoted, shell);
        return Ok(());
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
        if name == "@" && op != '+' {
            positional(quoted, shell, builder);
        } else {
            builder.expanded(
                if op == '+' { OsStr::new("") } else { &value },
                quoted,
                shell,
            );
        }
        return Ok(());
    }
    let fallback = &suffix[1..];
    let word =
        crate::syntax::parse_parameter_word(fallback, quoted).map_err(|e| fail(e.to_string()))?;
    if matches!(op, '-' | '+') {
        let previous = builder.parameter_operand;
        builder.parameter_operand = true;
        if quoted && word.parts.is_empty() {
            builder.literal("".as_ref(), false);
        }
        let result = append_word(&word, quoted, shell, io, builder);
        builder.parameter_operand = previous;
        return result;
    }
    let mut operand = Builder::new(false);
    append_word(&word, quoted, shell, io, &mut operand)?;
    let expanded = operand.fields.pop().unwrap().value;
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
    builder.expanded(&expanded, quoted, shell);
    Ok(())
}
