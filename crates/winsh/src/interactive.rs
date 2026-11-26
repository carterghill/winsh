//! Line editing and per-user history for the interactive CLI.
use std::{
    borrow::Cow,
    env, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

use rustyline::{
    CompletionType, Config, Context, Editor, Helper,
    completion::{Completer, Pair},
    error::ReadlineError,
    highlight::Highlighter,
    hint::Hinter,
    history::DefaultHistory,
    validate::Validator,
};
#[cfg(windows)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use winsh::{CompletionContext, Shell, ShellIo};

use crate::console;

struct ShellCompleter {
    session: CompletionContext,
    pending: String,
    styled_prompt: String,
}

impl Completer for ShellCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let before = format!("{}{}", self.pending, &line[..pos]);
        let candidates = self
            .session
            .candidates(&before, &line[pos..])
            .into_iter()
            .map(|(display, replacement)| Pair {
                display,
                replacement,
            })
            .collect();
        Ok((pos, candidates))
    }
}

impl Hinter for ShellCompleter {
    type Hint = String;
}
impl Validator for ShellCompleter {}
impl Highlighter for ShellCompleter {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        default: bool,
    ) -> Cow<'b, str> {
        Cow::Borrowed(if default { &self.styled_prompt } else { prompt })
    }
}
impl Helper for ShellCompleter {}

type LineEditor = Editor<ShellCompleter, DefaultHistory>;

pub enum Input {
    Redirected(fs::File),
    Console,
    Edited(Box<CommandEditor>),
}

impl Input {
    pub fn new(interactive: bool) -> io::Result<Self> {
        if !interactive {
            return console::redirected_stdin().map(Self::Redirected);
        }
        // Screen editing needs a visible prompt and must not write into a pipe.
        // Rustyline falls back to buffered stdin for these terminal types.
        // Keep our cooked reader there so Ctrl+C still cancels a pending read.
        let basic_terminal = env::var("TERM").is_ok_and(|term| {
            ["dumb", "cons25", "emacs"]
                .iter()
                .any(|name| term.eq_ignore_ascii_case(name))
        });
        if !io::stdout().is_terminal() || !io::stderr().is_terminal() || basic_terminal {
            return Ok(Self::Console);
        }
        Ok(Self::Edited(Box::new(CommandEditor::new()?)))
    }

    pub fn read_line(
        &mut self,
        shell: &Shell,
        streams: &mut ShellIo,
        pending: &str,
        line: &mut String,
    ) -> io::Result<usize> {
        if let Self::Redirected(file) = self {
            return console::read_line_unbuffered(file, line);
        }
        let prompt = shell.render_prompt(!pending.is_empty(), streams)?;
        if let Self::Edited(editor) = self {
            editor.lines.set_helper(Some(ShellCompleter {
                session: shell.completion_context(),
                pending: pending.to_owned(),
                styled_prompt: prompt.display,
            }));
            return match editor.lines.readline(&prompt.visible) {
                Ok(source) => {
                    *line = source;
                    #[cfg(windows)]
                    if editor.eof.0.swap(false, Ordering::Relaxed) {
                        return Ok(line.len());
                    }
                    line.push('\n');
                    Ok(line.len())
                }
                Err(ReadlineError::Interrupted) => Err(io::ErrorKind::Interrupted.into()),
                Err(ReadlineError::Eof) => Ok(0),
                Err(error) => Err(io::Error::other(error)),
            };
        }
        write!(io::stderr(), "{}", prompt.display)?;
        io::stderr().flush()?;
        let read = console::read_line(line);
        if matches!(&read, Ok(0))
            || matches!(&read, Err(e) if e.kind() == io::ErrorKind::Interrupted)
        {
            writeln!(io::stderr())?;
        }
        read
    }

    pub fn remember(&mut self, source: &str) {
        if let Self::Edited(editor) = self {
            editor.remember(source);
        }
    }
}

pub struct CommandEditor {
    lines: LineEditor,
    path: Option<PathBuf>,
    #[cfg(windows)]
    eof: ConsoleEof,
}

// ReadConsoleW delivers any pending text without a newline on Ctrl+Z.
// Rustyline's usual EOF binding only works on an empty buffer.
#[cfg(windows)]
#[derive(Clone, Default)]
struct ConsoleEof(Arc<AtomicBool>);

#[cfg(windows)]
impl rustyline::ConditionalEventHandler for ConsoleEof {
    fn handle(
        &self,
        _: &rustyline::Event,
        _: rustyline::RepeatCount,
        _: bool,
        _: &rustyline::EventContext<'_>,
    ) -> Option<rustyline::Cmd> {
        self.0.store(true, Ordering::Relaxed);
        Some(rustyline::Cmd::AcceptLine)
    }
}

impl CommandEditor {
    fn new() -> io::Result<Self> {
        let config = Config::builder()
            .completion_type(CompletionType::List)
            .max_history_size(1000)
            .map_err(io::Error::other)?
            .build();
        let mut lines = LineEditor::with_config(config).map_err(io::Error::other)?;
        #[cfg(windows)]
        let eof = {
            let eof = ConsoleEof::default();
            lines.bind_sequence(
                rustyline::KeyEvent::ctrl('Z'),
                rustyline::EventHandler::Conditional(Box::new(eof.clone())),
            );
            eof
        };
        let path = history_path().filter(|path| match load_history(&mut lines, path) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("winsh: cannot load history {}: {error}", path.display());
                false
            }
        });
        Ok(Self {
            lines,
            path,
            #[cfg(windows)]
            eof,
        })
    }

    fn remember(&mut self, source: &str) {
        if source.trim().is_empty() {
            return;
        }
        if let Ok(true) = self.lines.add_history_entry(source)
            && let Some(path) = &self.path
            && let Err(error) = self.lines.append_history(path)
        {
            eprintln!("winsh: cannot save history {}: {error}", path.display());
            self.path = None;
        }
    }
}

fn load_history(lines: &mut LineEditor, path: &Path) -> rustyline::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Create without truncation so simultaneous first sessions both use the
    // history backend's locked append, even when the file starts out empty.
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?;
    lines.load_history(path)
}

fn history_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("WINSH_HISTORY_FILE") {
        return (!path.is_empty()).then(|| PathBuf::from(path));
    }
    #[cfg(windows)]
    let root = env::var_os("LOCALAPPDATA")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    #[cfg(unix)]
    let root = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::home_dir().map(|home| home.join(".local/state")));
    root.map(|root| root.join("winsh").join("history"))
}
