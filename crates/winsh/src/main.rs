use std::{
    env,
    ffi::OsString,
    io::{self, IsTerminal, Write},
};

use winsh::{Outcome, Shell, ShellIo, Utility, syntax::parse_script};

mod console;

const HELP: &str = "A native Windows shell with Bash-compatible syntax.

Usage: winsh [--help | --version]
       winsh -c <command> [name [arguments...]]
       winsh [--] <script> [arguments...]
       winsh --utility <cat|grep|ls> [arguments...]

With no arguments, read commands from the terminal or standard input.

Options:
  -c            Execute a command string and exit
  -h, --help    Show this help
  -V, --version Show the version

Supports variables, expansions, lists, if/for/while/until, functions,
redirections, pipelines, and native executables. See docs/shell.md for limits.";

enum Mode {
    Input,
    Command(String, Vec<OsString>),
    Utility(Utility, Vec<OsString>),
    Script(OsString, Vec<OsString>),
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
        Some("--utility") => {
            let utility = args
                .next()
                .as_deref()
                .and_then(Utility::from_name)
                .ok_or("--utility expects cat, grep, or ls")?;
            return Ok(Mode::Utility(utility, args.collect()));
        }
        Some("-c") => {
            let command = args.next().ok_or("-c requires a command string")?;
            return Ok(Mode::Command(
                command
                    .into_string()
                    .map_err(|_| "command text must be valid UTF-8")?,
                args.collect(),
            ));
        }
        Some("--") => {
            return Ok(match args.next() {
                Some(file) => Mode::Script(file, args.collect()),
                None => Mode::Input,
            });
        }
        Some(arg) if arg.starts_with('-') => return Err("unsupported option; see --help"),
        _ => return Ok(Mode::Script(first, args.collect())),
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
    if let Mode::Utility(utility, args) = mode {
        return utility.run(&shell, &args, &mut streams);
    }
    let interactive = io::stdin().is_terminal();
    let _handler = if interactive {
        shell.set_utility_executable(env::current_exe()?);
        Some(console::Handler::install()?)
    } else {
        None
    };
    match mode {
        Mode::Command(source, mut arguments) => {
            let arg0 = if arguments.is_empty() {
                "winsh".into()
            } else {
                arguments.remove(0)
            };
            shell.set_arguments(arg0, arguments);
            Ok(shell.run(&source, &mut streams)?.status())
        }
        Mode::Script(file, arguments) => {
            let source = match std::fs::read_to_string(&file) {
                Ok(source) => source,
                Err(error) => {
                    writeln!(io::stderr(), "winsh: {}: {error}", file.to_string_lossy())?;
                    return Ok(127);
                }
            };
            shell.set_arguments(file, arguments);
            let source = source.trim_start_matches('\u{feff}').replace("\r\n", "\n");
            Ok(shell.run(&source, &mut streams)?.status())
        }
        Mode::Input => {
            if interactive && let Ok(label) = env::var("WINSH_DEV_BUILD") {
                writeln!(io::stderr(), "winsh dev build {label}")?;
            }
            input_loop(&mut shell, &mut streams, interactive)
        }
        Mode::Utility(..) | Mode::Help | Mode::Version => unreachable!(),
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
        match parse_script(&pending) {
            Err(error) if error.is_incomplete() && newline => {
                pending.push('\n');
                continue;
            }
            Ok(script) => {
                let outcome = shell.execute_script(&script, streams)?;
                if let Outcome::Exit(status) = outcome {
                    return Ok(status);
                }
            }
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
