//! File utilities bundled with the shell. Paths and streams belong to the session.
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read},
    path::Path,
};

use same_file::Handle;

use crate::{Shell, ShellIo, platform};

mod files;
mod grep;
mod ls;

/// Utilities bundled in winsh. Embedders can run them with explicit session I/O.
#[derive(Clone, Copy)]
pub enum Utility {
    Cat,
    Cp,
    Grep,
    Ls,
    Mkdir,
    Mv,
    Rm,
    Touch,
}

impl Utility {
    pub(crate) const COMMANDS: [(&'static str, Self); 8] = [
        ("cat", Self::Cat),
        ("cp", Self::Cp),
        ("grep", Self::Grep),
        ("ls", Self::Ls),
        ("mkdir", Self::Mkdir),
        ("mv", Self::Mv),
        ("rm", Self::Rm),
        ("touch", Self::Touch),
    ];

    pub fn from_name(name: &OsStr) -> Option<Self> {
        let name = name.to_str()?;
        Self::COMMANDS
            .into_iter()
            .find(|(command, _)| *command == name)
            .map(|(_, utility)| utility)
    }

    pub(crate) fn reads_stdin(self, args: &[OsString]) -> bool {
        let files = match self {
            Self::Cat => cat_options(args).ok().map(|(_, _, files)| files),
            Self::Grep => grep::files(args),
            _ => None,
        };
        files.is_some_and(|files| operands(files).any(|f| f == "-"))
    }

    pub fn run(self, shell: &Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<i32> {
        let (name, failure, result) = match self {
            Self::Cat => ("cat", 1, cat(shell, args, io)),
            Self::Grep => ("grep", 2, grep::run(shell, args, io)),
            Self::Ls => ("ls", 2, ls::run(shell, args, io)),
            Self::Mkdir => return files::run(self, "mkdir", "p", shell, args, io),
            Self::Touch => return files::run(self, "touch", "acm", shell, args, io),
            Self::Cp => return files::run(self, "cp", "rR", shell, args, io),
            Self::Mv => return files::run(self, "mv", "", shell, args, io),
            Self::Rm => return files::run(self, "rm", "frR", shell, args, io),
        };
        match result {
            Ok(status) => Ok(status),
            Err(error) => {
                io.err(format_args!("{name}: {error}\n"))?;
                Ok(failure)
            }
        }
    }
}

fn operands(files: &[OsString]) -> impl Iterator<Item = &OsStr> {
    files
        .iter()
        .map(OsString::as_os_str)
        .chain(files.is_empty().then_some(OsStr::new("-")))
}

fn output_id(io: &ShellIo) -> io::Result<Option<Handle>> {
    match io.output_file()? {
        Some(file) if file.metadata().is_ok_and(|m| m.is_file()) => {
            Handle::from_file(file).map(Some)
        }
        _ => Ok(None),
    }
}

fn open_input(
    shell: &Shell,
    io: &ShellIo,
    name: &OsStr,
    output: Option<&Handle>,
) -> io::Result<Box<dyn Read>> {
    if name == "-" && io.inherits_console_input() {
        return Ok(Box::new(io::stdin()));
    }
    let file = if name == "-" {
        io.input_file()?
    } else {
        File::open(platform::absolute(shell.cwd(), Path::new(name))?)?
    };
    if let Some(output) = output
        && file.metadata().is_ok_and(|m| m.is_file())
        && Handle::from_file(file.try_clone()?)? == *output
    {
        return Err(io::Error::other("input file is also the output file"));
    }
    Ok(Box::new(file))
}

fn cat_options(mut args: &[OsString]) -> Result<(bool, bool, &[OsString]), char> {
    let mut number = false;
    let mut nonblank = false;
    while let Some((arg, rest)) = args.split_first() {
        if arg == "--" {
            args = rest;
            break;
        }
        let Some(flags) = arg
            .to_str()
            .and_then(|s| s.strip_prefix('-'))
            .filter(|s| !s.is_empty())
        else {
            break;
        };
        for flag in flags.chars() {
            match flag {
                'n' => number = true,
                'b' => nonblank = true,
                'u' => {}
                _ => return Err(flag),
            }
        }
        args = rest;
    }
    Ok((number, nonblank, args))
}

fn cat(shell: &Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<i32> {
    let (number, nonblank, args) = match cat_options(args) {
        Ok(parsed) => parsed,
        Err(flag) => {
            io.err(format_args!("cat: unknown option -{flag}\n"))?;
            return Ok(1);
        }
    };
    let output = output_id(io)?;
    let mut status = 0;
    let mut at_line_start = true;
    let mut next_number = 1u64;
    let mut buffer = [0; 32 * 1024];
    for name in operands(args) {
        let mut input = match open_input(shell, io, name, output.as_ref()) {
            Ok(file) => file,
            Err(error) => {
                io.err(format_args!("cat: {}: {error}\n", name.to_string_lossy()))?;
                status = 1;
                continue;
            }
        };
        loop {
            let used = match input.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(error) => {
                    io.err(format_args!("cat: {}: {error}\n", name.to_string_lossy()))?;
                    status = 1;
                    break;
                }
            };
            if number || nonblank {
                for part in buffer[..used].split_inclusive(|&b| b == b'\n') {
                    if at_line_start && !(nonblank && part == b"\n") {
                        write!(io.output(), "{next_number:6}\t")?;
                        next_number += 1;
                    }
                    io.output().write_all(part)?;
                    at_line_start = part.ends_with(b"\n");
                }
            } else {
                io.output().write_all(&buffer[..used])?;
            }
            io.output().flush()?;
        }
    }
    Ok(status)
}
