use std::{ffi::OsString, fs, io, path::Path};

use crate::{Outcome, Shell, ShellIo, platform};

pub(crate) fn names() -> impl Iterator<Item = &'static str> {
    ["cd", "pwd", "exit", "alias", "unalias"]
        .into_iter()
        .chain(crate::jobs::BUILTINS.iter().copied())
        .chain(crate::language_builtins::NAMES.iter().copied())
}

pub(crate) fn recognizes(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| names().any(|builtin| builtin == name))
}

pub(crate) fn dispatch(
    shell: &mut Shell,
    arguments: &[OsString],
    io: &mut ShellIo,
) -> io::Result<Option<Outcome>> {
    let args = &arguments[1..];
    let outcome = match arguments[0].to_str() {
        Some("pwd") => pwd(shell, args, io)?,
        Some("cd") => cd(shell, args, io)?,
        Some("exit") => exit(shell, args, io)?,
        Some("alias") => Outcome::Continue(crate::aliases::define(shell, args, io)?),
        Some("unalias") => Outcome::Continue(crate::aliases::remove(shell, args, io)?),
        Some(name) if crate::jobs::BUILTINS.contains(&name) => {
            Outcome::Continue(shell.jobs.builtin(name, args, io)?)
        }
        Some(name) if crate::language_builtins::NAMES.contains(&name) => {
            crate::language_builtins::dispatch(shell, name, args, io)?
        }
        _ => return Ok(None),
    };
    Ok(Some(outcome))
}

fn fail(
    io: &mut ShellIo,
    command: &str,
    message: impl std::fmt::Display,
    status: i32,
) -> io::Result<Outcome> {
    io.err(format_args!("winsh: {command}: {message}\n"))?;
    Ok(Outcome::Continue(status))
}

// Shared -L/-P/-- parsing for the two directory built-ins.
fn directory_options(args: &[OsString]) -> Result<(bool, &[OsString]), &OsString> {
    let mut physical = false;
    for (index, arg) in args.iter().enumerate() {
        if arg == "--" {
            return Ok((physical, &args[index + 1..]));
        }
        let Some(option) = arg
            .to_str()
            .filter(|arg| arg.starts_with('-') && *arg != "-")
        else {
            return Ok((physical, &args[index..]));
        };
        for flag in option[1..].chars() {
            match flag {
                'L' => physical = false,
                'P' => physical = true,
                _ => return Err(arg),
            }
        }
    }
    Ok((physical, &[]))
}

fn pwd(shell: &Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<Outcome> {
    let (physical, _) = match directory_options(args) {
        Ok(options) => options,
        Err(arg) => {
            return fail(
                io,
                "pwd",
                format_args!("invalid option: {}", arg.to_string_lossy()),
                2,
            );
        }
    };
    let path = if physical {
        match fs::canonicalize(&shell.cwd) {
            Ok(path) => path,
            Err(error) => return fail(io, "pwd", error, 1),
        }
    } else {
        shell.cwd.clone()
    };
    io.out(format_args!("{}\n", path.display()))?;
    Ok(Outcome::Continue(0))
}

fn cd(shell: &mut Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<Outcome> {
    let (physical, args) = match directory_options(args) {
        Ok(options) => options,
        Err(arg) => {
            return fail(
                io,
                "cd",
                format_args!("invalid option: {}", arg.to_string_lossy()),
                2,
            );
        }
    };
    if args.len() > 1 {
        return fail(io, "cd", "too many arguments", 1);
    }
    let print = args.first().is_some_and(|arg| arg == "-");
    let target = match args.first() {
        Some(_) if print => {
            let Some(previous) = shell.var("OLDPWD") else {
                return fail(io, "cd", "OLDPWD is not set", 1);
            };
            previous.to_owned()
        }
        Some(arg) => arg.clone(),
        None => {
            let Some(home) = shell.home() else {
                return fail(io, "cd", "HOME is not set", 1);
            };
            home.to_owned()
        }
    };
    let changed = (|| {
        let path = platform::absolute(&shell.cwd, Path::new(&target))?;
        platform::directory(&path, physical)
    })();
    let destination = match changed {
        Ok(path) => path,
        Err(error) => {
            return fail(
                io,
                "cd",
                format_args!("{}: {error}", target.to_string_lossy()),
                1,
            );
        }
    };
    shell.set_env("OLDPWD", shell.cwd.clone().into_os_string());
    shell.cwd = destination;
    shell.set_env("PWD", shell.cwd.clone().into_os_string());
    if print {
        io.out(format_args!("{}\n", shell.cwd.display()))?;
    }
    Ok(Outcome::Continue(0))
}

fn exit(shell: &Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<Outcome> {
    let args = args.strip_prefix(&[OsString::from("--")]).unwrap_or(args);
    let Some(value) = args.first() else {
        return Ok(Outcome::Exit(shell.last_status()));
    };
    let Some(status) = value
        .to_str()
        .and_then(|value| value.trim().parse::<i64>().ok())
    else {
        io.err(format_args!(
            "winsh: exit: {}: numeric argument required\n",
            value.to_string_lossy()
        ))?;
        return Ok(Outcome::Exit(2));
    };
    if args.len() > 1 {
        return fail(io, "exit", "too many arguments", 1);
    }
    Ok(Outcome::Exit(status.rem_euclid(256) as i32))
}
