use crate::{Shell, ShellIo, platform};
use std::{ffi::OsString, fs, io, path::Path};

pub(super) fn run(shell: &Shell, mut args: &[OsString], io: &mut ShellIo) -> io::Result<i32> {
    let mut show_hidden = false;
    let mut show_dot_dirs = false;
    let mut list_dirs = false;
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
                '1' => {}
                'a' => {
                    show_hidden = true;
                    show_dot_dirs = true;
                }
                'A' => show_hidden = true,
                'd' => list_dirs = true,
                _ => {
                    io.err(format_args!("ls: unknown option -{flag}\n"))?;
                    return Ok(2);
                }
            }
        }
        args = rest;
    }

    let current = [OsString::from(".")];
    let operands = if args.is_empty() { &current[..] } else { args };
    let mut files = Vec::new();
    let mut directories = Vec::new();
    let mut status = 0;
    for name in operands {
        let path = platform::absolute(shell.cwd(), Path::new(name));
        match path.and_then(|path| Ok((fs::symlink_metadata(&path)?, path))) {
            Ok((meta, path))
                if !list_dirs && (meta.is_dir() || (meta.is_symlink() && path.is_dir())) =>
            {
                directories.push((name, path));
            }
            Ok(_) => files.push(name),
            Err(error) => {
                io.err(format_args!("ls: {}: {error}\n", name.to_string_lossy()))?;
                status = 2;
            }
        }
    }

    files.sort_unstable();
    directories.sort_unstable();
    let mut printed = !files.is_empty();
    for name in files {
        writeln!(io.output(), "{}", name.to_string_lossy())?;
    }
    for (name, path) in directories {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) => {
                io.err(format_args!("ls: {}: {error}\n", name.to_string_lossy()))?;
                status = 2;
                continue;
            }
        };
        if operands.len() > 1 {
            if printed {
                writeln!(io.output())?;
            }
            writeln!(io.output(), "{}:", name.to_string_lossy())?;
            printed = true;
        }
        let mut names = Vec::new();
        if show_dot_dirs {
            names.extend([OsString::from("."), OsString::from("..")]);
        }
        for entry in entries {
            match entry {
                Ok(entry) => {
                    let name = entry.file_name();
                    if show_hidden || !name.as_encoded_bytes().starts_with(b".") {
                        names.push(name);
                    }
                }
                Err(error) => {
                    io.err(format_args!("ls: {}: {error}\n", name.to_string_lossy()))?;
                    status = status.max(1);
                }
            }
        }
        names.sort_unstable();
        for name in names {
            writeln!(io.output(), "{}", name.to_string_lossy())?;
        }
    }
    io.output().flush()?;
    Ok(status)
}
