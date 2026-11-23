use std::{
    env,
    ffi::OsString,
    io::{self, IsTerminal, Write},
};

use winsh::{Outcome, Shell, ShellIo, syntax::parse_simple_command};

mod console;

const HELP: &str = "A native Windows shell with Bash-compatible syntax.

Usage: winsh [--help | --version]
       winsh -c <command>

With no arguments, read commands from the terminal or standard input.

Options:
  -c            Execute one simple command and exit
  -h, --help    Show this help
  -V, --version Show the version

Built-ins: cd [-L|-P] [--] [directory|-], pwd [-L|-P], exit [status]
Supports quoted words, escapes, ~/ paths, and native executables.
Pipelines, redirection, variables, wildcards, and script-file arguments are not implemented yet.";

enum Mode {
    Input,
    Command(String),
    Help,
    Version,
}

fn options(mut args: impl Iterator<Item = OsString>) -> Result<Mode, &'static str> {
    let Some(first) = args.next() else {
        return Ok(Mode::Input);
    };
    let mode = match first.to_str() {
        Some("-h" | "--help") => Mode::Help,
        Some("-V" | "--version") => Mode::Version,
        Some("-c") => {
            let command = args.next().ok_or("-c requires a command string")?;
            Mode::Command(
                command
                    .into_string()
                    .map_err(|_| "command text must be valid UTF-8")?,
            )
        }
        _ => return Err("unsupported argument; see --help"),
    };
    if args.next().is_some() {
        return Err("unexpected extra argument; see --help");
    }
    Ok(mode)
}

fn main() {
    // Only this process boundary exits. The engine and built-ins return outcomes.
    let status = match run() {
        Ok(status) => status,
        Err(error) => {
            let _ = writeln!(io::stderr(), "winsh: {error}");
            1
        }
    };
    std::process::exit(status);
}

fn run() -> io::Result<i32> {
    let mode = match options(env::args_os().skip(1)) {
        Ok(mode) => mode,
        Err(message) => {
            writeln!(io::stderr(), "winsh: {message}")?;
            return Ok(2);
        }
    };
    match mode {
        Mode::Help => {
            writeln!(io::stdout(), "winsh {}\n{HELP}", env!("CARGO_PKG_VERSION"))?;
            return Ok(0);
        }
        Mode::Version => {
            writeln!(io::stdout(), "winsh {}", env!("CARGO_PKG_VERSION"))?;
            return Ok(0);
        }
        _ => {}
    }
    let mut shell = Shell::new()?;
    let mut streams = ShellIo::inherited();
    let interactive = io::stdin().is_terminal();
    let _handler = if interactive {
        Some(console::Handler::install()?)
    } else {
        None
    };
    match mode {
        Mode::Command(source) => Ok(shell.run(&source, &mut streams)?.status()),
        Mode::Input => input_loop(&mut shell, &mut streams, interactive),
        Mode::Help | Mode::Version => unreachable!(),
    }
}

fn input_loop(shell: &mut Shell, streams: &mut ShellIo, interactive: bool) -> io::Result<i32> {
    let mut redirected = if interactive {
        None
    } else {
        Some(console::redirected_stdin()?)
    };
    let mut pending = String::new();
    loop {
        if interactive {
            if pending.is_empty() {
                write!(io::stderr(), "{} $ ", shell.cwd().display())?;
            } else {
                write!(io::stderr(), "> ")?;
            }
            io::stderr().flush()?;
        }
        let mut line = String::new();
        let read = match &mut redirected {
            Some(input) => console::read_line_unbuffered(input, &mut line),
            None => console::read_line(&mut line),
        };
        if matches!(&read, Err(error) if error.kind() == io::ErrorKind::Interrupted) {
            pending.clear();
            shell.interrupt();
            writeln!(io::stderr())?;
            continue;
        }
        if read? == 0 {
            if interactive {
                writeln!(io::stderr())?;
            }
            return if pending.is_empty() {
                Ok(shell.last_status())
            } else {
                Ok(shell.run(&pending, streams)?.status())
            };
        }
        let newline = line.ends_with('\n');
        if newline {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        pending.push_str(&line);
        match parse_simple_command(&pending) {
            Err(error) if error.is_incomplete() && newline => {
                pending.push('\n');
                continue;
            }
            Ok(Some(command)) => {
                let outcome = shell.execute(&command, streams)?;
                if let Outcome::Exit(status) = outcome {
                    return Ok(status);
                }
            }
            Ok(None) => {}
            Err(_) => {
                shell.run(&pending, streams)?;
                if !interactive {
                    return Ok(shell.last_status());
                }
            }
        }
        pending.clear();
    }
}
