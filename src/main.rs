use std::{env, process::ExitCode};

const HELP: &str = "A native Windows shell with Bash-compatible syntax.

Usage: winsh [--help | --version]

Options:
  -h, --help     Show this help
  -V, --version  Show the version

Interactive commands and script execution are not implemented yet.";

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let option = args.next();

    if args.next().is_some() {
        eprintln!("winsh: expected at most one option; see --help");
        return ExitCode::from(2);
    }

    match option.as_deref().and_then(|value| value.to_str()) {
        None if option.is_none() => println!("winsh {}\n{HELP}", env!("CARGO_PKG_VERSION")),
        Some("-h" | "--help") => println!("winsh {}\n{HELP}", env!("CARGO_PKG_VERSION")),
        Some("-V" | "--version") => println!("winsh {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!("winsh: unsupported argument; see --help");
            return ExitCode::from(2);
        }
    }

    ExitCode::SUCCESS
}
