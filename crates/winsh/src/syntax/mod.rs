//! Parsing for the first supported subset of Bash: one simple command.
//!
//! Words retain quote boundaries, escapes, and UTF-8 byte ranges into the input.
//! Parsing never expands words, reads the environment, or executes commands.

use std::{fmt, ops::Range};

mod parser;

pub use parser::parse_simple_command;

/// A command's words before expansion or command lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCommand {
    pub words: Vec<Word>,
    /// Source range excluding leading/trailing whitespace and comments.
    pub span: Range<usize>,
}

/// Adjacent quoted and unquoted pieces form one shell word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub parts: Vec<WordPart>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordPart {
    pub kind: WordPartKind,
    /// Original source range, including quote delimiters or escape backslashes.
    /// A text range may also include removed backslash-newline continuations.
    pub span: Range<usize>,
}

/// Quote context is structural, so escaped `$` and `*` remain distinguishable
/// from characters that may trigger expansion later. Empty quotes are retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordPartKind {
    /// Unquoted text at word level; quoted text inside `DoubleQuoted`.
    /// Expansion syntax such as `$HOME`, `~`, and `*.rs` is left untouched.
    Text(String),
    SingleQuoted(String),
    /// Children are text or escapes; single quotes here are ordinary text.
    DoubleQuoted(Vec<WordPart>),
    Escaped(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quote {
    Single,
    Double,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    /// UTF-8 byte range into the original input, not a character or column range.
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    UnclosedQuote(Quote),
    TrailingEscape,
    UnsupportedSyntax(&'static str),
    MultipleCommands,
    NullByte,
}

impl ParseError {
    /// Whether more input could complete the current quote or escape.
    pub fn is_incomplete(&self) -> bool {
        matches!(
            self.kind,
            ParseErrorKind::UnclosedQuote(_) | ParseErrorKind::TrailingEscape
        )
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ParseErrorKind::UnclosedQuote(Quote::Single) => write!(f, "unclosed single quote"),
            ParseErrorKind::UnclosedQuote(Quote::Double) => write!(f, "unclosed double quote"),
            ParseErrorKind::TrailingEscape => write!(f, "expected a character after backslash"),
            ParseErrorKind::UnsupportedSyntax(feature) => {
                write!(f, "{feature} is not supported yet")
            }
            ParseErrorKind::MultipleCommands => write!(f, "expected only one simple command"),
            ParseErrorKind::NullByte => write!(f, "shell input cannot contain a NUL byte"),
        }?;
        write!(f, " at bytes {}..{}", self.span.start, self.span.end)
    }
}

impl std::error::Error for ParseError {}
