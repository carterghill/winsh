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

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct OptionScan {
    // Zero requests a fresh scan on the next getopts call.
    pub(crate) index: usize,
    group: String,
    offset: usize,
}

impl OptionScan {
    pub(crate) fn assign_index(&mut self, value: &OsStr) {
        // A temporary binding can be restored before another option is read.
        // Discard pending letters only when next() actually starts a fresh scan.
        self.index = value.to_str().and_then(|s| s.parse().ok()).unwrap_or(0);
        if self.index == 1 {
            self.index = 0;
        }
    }

    fn next(&mut self, argv: &[OsString]) -> Option<char> {
        if self.index == 0 {
            self.index = 1;
            self.group.clear();
        }
        self.index = self.index.min(argv.len() + 1);
        if self.group.is_empty() {
            let arg = argv.get(self.index - 1)?.to_str()?;
            if arg == "--" {
                self.index += 1;
                return None;
            }
            let flags = arg.strip_prefix('-').filter(|s| !s.is_empty())?;
            self.group.push_str(flags);
            self.offset = 0;
        }
        let ch = self.group[self.offset..].chars().next()?;
        self.offset += ch.len_utf8();
        if self.offset == self.group.len() {
            self.group.clear();
            self.index += 1;
        }
        Some(ch)
    }

    fn argument(&mut self, argv: &[OsString]) -> Option<OsString> {
        let value = if self.group.is_empty() {
            argv.get(self.index - 1)?.clone()
        } else {
            let remainder = self.group[self.offset..].into();
            self.group.clear();
            remainder
        };
        self.index += 1;
        Some(value)
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
    let mut option = '?';
    let mut optarg = None;
    let next = shell.option_scan.next(argv);
    if let Some(ch) = next {
        if let Some((_, after)) = spec.split_once(ch).filter(|_| ch != ':') {
            option = ch;
            if after.starts_with(':') {
                optarg = shell.option_scan.argument(argv);
                if optarg.is_none() {
                    if silent {
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
            }
        } else if silent {
            optarg = Some(ch.to_string().into());
        } else if shell.var("OPTERR") != Some(OsStr::new("0")) {
            io.err(format_args!("winsh: getopts: illegal option -- {ch}\n"))?;
        }
    }
    shell.publish_optind();
    match optarg {
        Some(value) => shell.set_var("OPTARG", value),
        None => shell.unset("OPTARG"),
    }
    let Some(name) = name.to_str().filter(|s| syntax::valid_name(s)) else {
        return fail(io, "getopts", "invalid variable name", 1);
    };
    shell.set_var(name, option.to_string().into());
    Ok(Outcome::Continue(i32::from(next.is_none())))
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
