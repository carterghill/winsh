//! Script grammar, words, redirections, and foreground pipelines.
//!
//! Words retain quote boundaries, escapes, and UTF-8 byte ranges into the input.
//! Parsing never expands words, reads the environment, or executes commands.

use std::{fmt, ops::Range};

mod parser;

pub(crate) use parser::completion::{Target, completion_word};
pub use parser::parse_script;
pub use parser::valid_name;
pub(crate) use parser::{parse_document_body, parse_fragment};
pub use parser::{parse_pipeline, parse_simple_command};

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Script {
    pub lists: Vec<AndOr>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct AndOr {
    /// Original command text when the list was terminated by an asynchronous '&'.
    pub background: Option<String>,
    pub pipelines: Vec<ScriptPipeline>,
    pub operators: Vec<ListOperator>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListOperator {
    And,
    Or,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ScriptPipeline {
    /// Display text for a pipeline or subshell that can run as a foreground job.
    pub label: Option<String>,
    pub commands: Vec<Command>,
    pub connections: Vec<PipeKind>,
    pub negated: bool,
}

impl ScriptPipeline {
    pub(crate) fn uses_subshell(&self) -> bool {
        self.commands.len() > 1
            || self
                .commands
                .first()
                .is_some_and(|c| matches!(c.kind, CommandKind::Group { subshell: true, .. }))
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub kind: CommandKind,
    pub redirections: Vec<Redirection>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum CommandKind {
    Simple(SimpleCommand),
    Group {
        body: Script,
        subshell: bool,
    },
    If {
        branches: Vec<(Script, Script)>,
        otherwise: Option<Script>,
    },
    Case {
        word: Word,
        arms: Vec<CaseArm>,
    },
    For {
        name: String,
        words: Option<Vec<Word>>,
        body: Script,
    },
    While {
        condition: Script,
        body: Script,
        until: bool,
    },
    Function {
        name: String,
        body: Box<Command>,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CaseArm {
    pub patterns: Vec<Word>,
    pub body: Script,
    pub terminator: CaseTerminator,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseTerminator {
    Break,
    FallThrough,
    Retest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<SimpleCommand>,
    /// One connection between each adjacent pair of commands.
    pub connections: Vec<PipeKind>,
    pub span: Range<usize>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeKind {
    Stdout,
    StdoutAndStderr,
}

/// A command's words before expansion or command lookup.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SimpleCommand {
    pub words: Vec<Word>,
    /// Kept in source order, independently of intervening command words.
    pub redirections: Vec<Redirection>,
    /// Source range excluding leading/trailing whitespace and comments.
    pub span: Range<usize>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Redirection {
    /// None selects the operator's default descriptor. Only 0, 1, 2 are supported.
    pub descriptor: Option<u8>,
    pub kind: RedirectionKind,
    pub target: Word,
    pub here_document: Option<HereDocument>,
    pub span: Range<usize>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct HereDocument {
    pub body: String,
    pub quoted: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectionKind {
    Input,
    HereDocument,
    HereDocumentStripTabs,
    Output,
    Append,
    ReadWrite,
    Clobber,
    DuplicateInput,
    DuplicateOutput,
    OutputAndError,
    AppendAndError,
}

/// Adjacent quoted and unquoted pieces form one shell word.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub parts: Vec<WordPart>,
    pub span: Range<usize>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct WordPart {
    pub kind: WordPartKind,
    /// Original source range, including quote delimiters or escape backslashes.
    /// A text range may also include removed backslash-newline continuations.
    pub span: Range<usize>,
}

/// Quote context is structural, so escaped `$` and `*` remain distinguishable
/// from characters that may trigger expansion later. Empty quotes are retained.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum WordPartKind {
    /// Unquoted text at word level; quoted text inside `DoubleQuoted`.
    /// Simple references such as `$HOME`, `~`, and `*.rs` remain unevaluated.
    Text(String),
    SingleQuoted(String),
    /// Children retain nested expansions and escapes; single quotes are text.
    DoubleQuoted(Vec<WordPart>),
    Escaped(char),
    Parameter(String),
    CommandSubstitution(String),
    Arithmetic(String),
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
    MissingCommand,
    MissingRedirectionTarget,
    UnclosedHereDocument(String),
    NullByte,
    Expected(&'static str),
    Unexpected(&'static str),
}

impl ParseError {
    /// Whether more input could complete the current quote or escape.
    pub fn is_incomplete(&self) -> bool {
        matches!(
            self.kind,
            ParseErrorKind::UnclosedQuote(_)
                | ParseErrorKind::TrailingEscape
                | ParseErrorKind::UnclosedHereDocument(_)
                | ParseErrorKind::MissingCommand
                | ParseErrorKind::Expected(_)
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
            ParseErrorKind::MultipleCommands => write!(f, "expected only one foreground pipeline"),
            ParseErrorKind::MissingCommand => write!(f, "expected a command after pipe"),
            ParseErrorKind::MissingRedirectionTarget => write!(f, "expected a redirection target"),
            ParseErrorKind::UnclosedHereDocument(ref delimiter) => {
                write!(f, "expected here-document delimiter {delimiter:?}")
            }
            ParseErrorKind::NullByte => write!(f, "shell input cannot contain a NUL byte"),
            ParseErrorKind::Expected(token) => write!(f, "expected {token}"),
            ParseErrorKind::Unexpected(token) => write!(f, "unexpected {token}"),
        }?;
        write!(f, " at bytes {}..{}", self.span.start, self.span.end)
    }
}

impl std::error::Error for ParseError {}
