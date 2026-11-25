use crate::{
    Outcome, Shell, ShellIo, Utility, aliases, builtins, exec, language_builtins::fail, syntax,
};
use std::{
    ffi::{OsStr, OsString},
    io,
};

pub(crate) fn command(
    shell: &mut Shell,
    mut args: &[OsString],
    io: &mut ShellIo,
) -> io::Result<Outcome> {
    loop {
        let describe = args.first().is_some_and(|a| a == "-v");
        if describe {
            args = &args[1..];
        }
        if args.first().is_some_and(|a| a == "--") {
            args = &args[1..];
        } else if args
            .first()
            .and_then(|a| a.to_str())
            .is_some_and(|s| s.starts_with('-') && s != "-")
        {
            return fail(io, "command", "expected [-v] [--] [name ...]", 2);
        }
        let Some((name, rest)) = args.split_first() else {
            return Ok(Outcome::Continue(0));
        };
        if describe {
            let mut found = false;
            for arg in args {
                if let Some(name) = arg.to_str()
                    && let Some(alias) = shell.aliases.get(name)
                {
                    aliases::display(name, alias, io)?;
                } else if builtins::recognizes(arg)
                    || Utility::from_name(arg).is_some()
                    || arg.to_str().is_some_and(|n| {
                        shell.functions.contains_key(n)
                            || matches!(
                                n,
                                "if" | "then"
                                    | "elif"
                                    | "else"
                                    | "fi"
                                    | "for"
                                    | "in"
                                    | "do"
                                    | "done"
                                    | "while"
                                    | "until"
                                    | "case"
                                    | "esac"
                                    | "{"
                                    | "}"
                                    | "!"
                            )
                    })
                {
                    io.out(format_args!("{}\n", arg.to_string_lossy()))?;
                } else if let Ok(path) = exec::command_path(shell, arg) {
                    #[cfg(windows)]
                    let path = path.to_string_lossy().replace('\\', "/");
                    #[cfg(unix)]
                    let path = path.display();
                    io.out(format_args!("{path}\n"))?;
                } else {
                    continue;
                }
                found = true;
            }
            return Ok(Outcome::Continue(i32::from(!found)));
        }
        // Skip shell function resolution, including a function named command.
        if name != "command" {
            return shell.invoke_command(args, io);
        }
        args = rest;
    }
}

pub(crate) fn getopts(
    shell: &mut Shell,
    args: &[OsString],
    io: &mut ShellIo,
) -> io::Result<Outcome> {
    let args = args.strip_prefix(&[OsString::from("--")]).unwrap_or(args);
    let [spec, name, explicit @ ..] = args else {
        return fail(io, "getopts", "expected optstring name [arguments...]", 2);
    };
    let Some(spec) = spec.to_str() else {
        return fail(io, "getopts", "optstring must be UTF-8", 2);
    };
    let silent = spec.starts_with(':');
    let spec = spec.strip_prefix(':').unwrap_or(spec);
    let argv = if explicit.is_empty() {
        &shell.positional[..]
    } else {
        explicit
    };
    let mut index = shell
        .var("OPTIND")
        .and_then(OsStr::to_str)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, argv.len() + 1);
    let mut offset = shell.opt_offset;
    let mut option = '?';
    let mut optarg = None;
    let mut done = true;
    if let Some(arg) = argv.get(index - 1).and_then(|a| a.to_str()) {
        if arg == "--" {
            index += 1;
        } else if let Some(flags) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            let tail = flags
                .get(offset..)
                .filter(|s| !s.is_empty())
                .unwrap_or(flags);
            let ch = tail.chars().next().unwrap();
            let rest = &tail[ch.len_utf8()..];
            done = false;
            offset = flags.len() - rest.len();
            if rest.is_empty() {
                index += 1;
                offset = 0;
            }
            if let Some((_, after)) = spec.split_once(ch).filter(|_| ch != ':') {
                option = ch;
                if after.starts_with(':') {
                    if !rest.is_empty() {
                        optarg = Some(rest.into());
                        index += 1;
                        offset = 0;
                    } else if let Some(next) = argv.get(index - 1) {
                        optarg = Some(next.clone());
                        index += 1;
                    } else if silent {
                        option = ':';
                        optarg = Some(ch.to_string().into());
                    } else {
                        option = '?';
                        if shell.var("OPTERR") != Some(OsStr::new("0")) {
                            io.err(format_args!(
                                "winsh: getopts: option requires an argument -- {ch}\n"
                            ))?;
                        }
                    }
                }
            } else if silent {
                optarg = Some(ch.to_string().into());
            } else if shell.var("OPTERR") != Some(OsStr::new("0")) {
                io.err(format_args!("winsh: getopts: illegal option -- {ch}\n"))?;
            }
        }
    }
    shell.set_var("OPTIND", index.to_string().into());
    if !done {
        shell.opt_offset = offset;
    }
    match optarg {
        Some(value) => shell.set_var("OPTARG", value),
        None => shell.unset("OPTARG"),
    }
    let Some(name) = name.to_str().filter(|s| syntax::valid_name(s)) else {
        return fail(io, "getopts", "invalid variable name", 1);
    };
    shell.set_var(name, option.to_string().into());
    Ok(Outcome::Continue(i32::from(done)))
}

pub(crate) fn exec(shell: &mut Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<Outcome> {
    let args = if let Some(rest) = args.strip_prefix(&[OsString::from("--")]) {
        rest
    } else {
        if args
            .first()
            .and_then(|a| a.to_str())
            .is_some_and(|s| s.starts_with('-'))
        {
            return fail(io, "exec", "expected [--] [program [arguments...]]", 2);
        }
        args
    };
    io.retain_redirections();
    let Some(program) = args.first() else {
        return Ok(Outcome::Continue(0));
    };
    if let Some(utility) = Utility::from_name(program) {
        shell.traps.clear();
        return Ok(Outcome::Exit(utility.run(shell, &args[1..], io)?));
    }
    let process = exec::spawn(shell, args, io)?;
    if let exec::Spawned::Failed(status) = process {
        return Ok(if shell.jobs.interactive {
            Outcome::Continue(status)
        } else {
            Outcome::Exit(status)
        });
    }
    shell.traps.clear();
    Ok(Outcome::Exit(exec::wait(shell, process)?))
}
