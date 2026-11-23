//! Inspect syntax without adding parser diagnostics to the shell's CLI.
use std::{env, process::ExitCode};

use winsh::syntax::parse_pipeline;

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let source = args.next();
    let Some(source) = source.as_deref().and_then(|source| source.to_str()) else {
        eprintln!("usage: cargo run --example parse -- '<UTF-8 command>'");
        return ExitCode::from(2);
    };
    if args.next().is_some() {
        eprintln!("pass the complete command as one quoted argument");
        return ExitCode::from(2);
    }

    match parse_pipeline(source) {
        Ok(command) => {
            println!("{command:#?}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("winsh: {error}");
            ExitCode::from(2)
        }
    }
}
