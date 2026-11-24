use crate::{Outcome, Shell, ShellIo, shell::Flow};
use std::{
    ffi::{OsStr, OsString},
    fs, io,
    path::Path,
};

pub(crate) fn recognizes(name: &OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(
            ":" | "true"
                | "false"
                | "echo"
                | "printf"
                | "test"
                | "["
                | "export"
                | "unset"
                | "local"
                | "set"
                | "shift"
                | "return"
                | "break"
                | "continue"
                | "."
                | "source"
                | "read"
        )
    )
}
pub(crate) fn dispatch(
    shell: &mut Shell,
    name: &str,
    args: &[OsString],
    io: &mut ShellIo,
) -> io::Result<Outcome> {
    let status = match name {
        ":" | "true" => 0,
        "false" => 1,
        "echo" => {
            let mut args = args;
            let newline = args.first().is_none_or(|a| a != "-n");
            if !newline {
                args = &args[1..];
            }
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    io.out(format_args!(" "))?;
                }
                io.out(format_args!("{}", arg.to_string_lossy()))?;
            }
            if newline {
                io.out(format_args!("\n"))?;
            }
            0
        }
        "printf" => match printf(args, io) {
            Ok(()) => 0,
            Err(message) => return fail(io, name, message, 2),
        },
        "test" | "[" => {
            let args = if name == "[" {
                if args.last().is_none_or(|s| s != "]") {
                    return fail(io, name, "missing ]", 2);
                }
                &args[..args.len() - 1]
            } else {
                args
            };
            match test(shell, args) {
                Ok(value) => i32::from(!value),
                Err(message) => return fail(io, name, message, 2),
            }
        }
        "export" | "local" => {
            if name == "local" && shell.scopes.is_empty() {
                return fail(io, name, "only valid inside a function", 1);
            }
            if args.is_empty() && name == "export" {
                for (key, value) in shell.environment() {
                    io.out(format_args!(
                        "export {}='{}'\n",
                        key.to_string_lossy(),
                        value.to_string_lossy().replace('\'', "'\\''")
                    ))?;
                }
                return Ok(Outcome::Continue(0));
            }
            for arg in args {
                let Some(text) = arg.to_str() else {
                    return fail(io, name, "identifier must be UTF-8", 1);
                };
                let (key, value) = text
                    .split_once('=')
                    .map_or((text, None), |(k, v)| (k, Some(v)));
                if !crate::syntax::valid_name(key) {
                    return fail(io, name, "invalid identifier", 1);
                }
                if name == "local" {
                    let saved = shell.save_var(key);
                    shell
                        .scopes
                        .last_mut()
                        .unwrap()
                        .entry(key.into())
                        .or_insert(saved);
                    if value.is_none() {
                        shell.unset(key);
                        shell.variables.insert(key.into(), OsString::new());
                    }
                }
                if let Some(value) = value {
                    shell.set_var(key, value.into());
                }
                if name == "export" {
                    let value = shell.var(key).unwrap_or_default().to_owned();
                    shell.variables.remove(key);
                    shell.set_env(key, value);
                }
            }
            0
        }
        "unset" => {
            let (functions, args) = if args.first().is_some_and(|s| s == "-f") {
                (true, &args[1..])
            } else if args.first().is_some_and(|s| s == "-v" || s == "--") {
                (false, &args[1..])
            } else {
                (false, args)
            };
            for arg in args {
                let Some(key) = arg.to_str().filter(|k| crate::syntax::valid_name(k)) else {
                    return fail(io, name, "invalid identifier", 1);
                };
                if functions {
                    shell.functions.remove(key);
                } else {
                    shell.unset(key);
                }
            }
            0
        }
        "set" => {
            if args.first().is_some_and(|a| a == "--") {
                shell.positional = args[1..].to_vec();
                0
            } else {
                return fail(io, name, "supported form: set -- [arguments...]", 2);
            }
        }
        "shift" => {
            let n = match count(args, 1) {
                Ok(n) => n,
                Err(message) => return fail(io, name, message, 2),
            };
            if n > shell.positional.len() {
                return fail(io, name, "shift count exceeds number of arguments", 1);
            }
            shell.positional.drain(..n);
            0
        }
        "return" => {
            if shell.function_depth == 0 && shell.source_depth == 0 {
                return fail(
                    io,
                    name,
                    "only valid inside a function or sourced script",
                    1,
                );
            }
            let n = match count(args, shell.last_status() as usize) {
                Ok(n) => n,
                Err(message) => return fail(io, name, message, 2),
            };
            let status = (n % 256) as i32;
            shell.flow = Some(Flow::Return(status));
            status
        }
        "break" | "continue" => {
            let n = match count(args, 1) {
                Ok(n) if n > 0 => n,
                _ => return fail(io, name, "positive loop count required", 1),
            };
            if shell.loop_depth == 0 {
                return fail(io, name, "only valid inside a loop", 1);
            }
            let n = n.min(shell.loop_depth);
            shell.flow = Some(if name == "break" {
                Flow::Break(n)
            } else {
                Flow::Continue(n)
            });
            0
        }
        "." | "source" => {
            if shell.source_depth >= 64 {
                return fail(io, name, "source nesting limit (64)", 2);
            }
            let Some(file) = args.first() else {
                return fail(io, name, "filename required", 2);
            };
            let source = match fs::read_to_string(shell.cwd().join(file)) {
                Ok(s) => s,
                Err(e) => return fail(io, name, e, 1),
            };
            let previous = if args.len() > 1 {
                Some(std::mem::replace(&mut shell.positional, args[1..].to_vec()))
            } else {
                None
            };
            shell.source_depth += 1;
            let result = shell.run(
                source
                    .trim_start_matches('\u{feff}')
                    .replace("\r\n", "\n")
                    .as_str(),
                io,
            );
            shell.source_depth -= 1;
            if let Some(previous) = previous {
                shell.positional = previous;
            }
            if let Some(Flow::Return(status)) = shell.flow {
                shell.flow = None;
                return Ok(Outcome::Continue(status));
            }
            return result;
        }
        "read" => return read(shell, args, io),
        _ => unreachable!(),
    };
    Ok(Outcome::Continue(status))
}
fn fail(
    io: &mut ShellIo,
    name: &str,
    message: impl std::fmt::Display,
    status: i32,
) -> io::Result<Outcome> {
    io.err(format_args!("winsh: {name}: {message}\n"))?;
    Ok(Outcome::Continue(status))
}
fn count(args: &[OsString], default: usize) -> Result<usize, &'static str> {
    if args.len() > 1 {
        return Err("too many arguments");
    }
    args.first().map_or(Ok(default), |arg| {
        arg.to_str()
            .and_then(|s| s.parse().ok())
            .ok_or("nonnegative integer required")
    })
}
fn printf(args: &[OsString], io: &mut ShellIo) -> Result<(), String> {
    let args = args.strip_prefix(&[OsString::from("--")]).unwrap_or(args);
    let Some(format) = args.first().and_then(|a| a.to_str()) else {
        return Err("format required".into());
    };
    let mut index = 1;
    loop {
        let before = index;
        let mut chars = format.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                let next = chars.next().ok_or("trailing backslash in format")?;
                let ch = escape(next)?;
                io.out(format_args!("{ch}")).map_err(|e| e.to_string())?;
            } else if ch == '%' {
                let spec = chars.next().ok_or("missing format specifier")?;
                if spec == '%' {
                    io.out(format_args!("%")).map_err(|e| e.to_string())?;
                    continue;
                }
                let value = args
                    .get(index)
                    .map(OsString::as_os_str)
                    .unwrap_or(OsStr::new(""));
                index += 1;
                let text = value.to_string_lossy();
                let rendered = match spec {
                    's' => text.into_owned(),
                    'b' => {
                        let mut out = String::new();
                        let mut chars = text.chars();
                        while let Some(ch) = chars.next() {
                            out.push(if ch == '\\' {
                                escape(chars.next().ok_or("trailing backslash")?)?
                            } else {
                                ch
                            });
                        }
                        out
                    }
                    'd' | 'i' | 'u' | 'x' | 'X' | 'o' => {
                        let value: i64 = if text.is_empty() {
                            0
                        } else {
                            text.parse().map_err(|_| "numeric argument required")?
                        };
                        match spec {
                            'x' => format!("{value:x}"),
                            'X' => format!("{value:X}"),
                            'o' => format!("{value:o}"),
                            'u' => (value as u64).to_string(),
                            _ => value.to_string(),
                        }
                    }
                    _ => return Err(format!("unsupported format specifier %{spec}")),
                };
                io.out(format_args!("{rendered}"))
                    .map_err(|e| e.to_string())?;
            } else {
                io.out(format_args!("{ch}")).map_err(|e| e.to_string())?;
            }
        }
        if index == before || index >= args.len() {
            break;
        }
    }
    Ok(())
}
fn escape(ch: char) -> Result<char, String> {
    match ch {
        'n' => Ok('\n'),
        'r' => Ok('\r'),
        't' => Ok('\t'),
        'a' => Ok('\u{7}'),
        'b' => Ok('\u{8}'),
        'v' => Ok('\u{b}'),
        'f' => Ok('\u{c}'),
        '\\' => Ok('\\'),
        _ => Err(format!("unsupported escape \\{ch}")),
    }
}
fn test(shell: &Shell, args: &[OsString]) -> Result<bool, String> {
    let text = |n: usize| args[n].to_string_lossy();
    match args.len() {
        0 => Ok(false),
        1 => Ok(!args[0].is_empty()),
        _ if args[0] == "!" => Ok(!test(shell, &args[1..])?),
        2 => match text(0).as_ref() {
            "-n" => Ok(!args[1].is_empty()),
            "-z" => Ok(args[1].is_empty()),
            "-e" | "-f" | "-d" | "-s" => {
                let path = shell.cwd().join(Path::new(&args[1]));
                Ok(match text(0).as_ref() {
                    "-e" => path.exists(),
                    "-f" => path.is_file(),
                    "-d" => path.is_dir(),
                    _ => fs::metadata(path).is_ok_and(|m| m.len() > 0),
                })
            }
            _ => Err("unsupported unary test operator".into()),
        },
        3 => match text(1).as_ref() {
            "=" | "==" => Ok(args[0] == args[2]),
            "!=" => Ok(args[0] != args[2]),
            "<" => Ok(args[0] < args[2]),
            ">" => Ok(args[0] > args[2]),
            "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" => {
                let a: i64 = text(0).parse().map_err(|_| "integer required")?;
                let b: i64 = text(2).parse().map_err(|_| "integer required")?;
                Ok(match text(1).as_ref() {
                    "-eq" => a == b,
                    "-ne" => a != b,
                    "-lt" => a < b,
                    "-le" => a <= b,
                    "-gt" => a > b,
                    _ => a >= b,
                })
            }
            _ => Err("unsupported binary test operator".into()),
        },
        _ => Err("use shell && / || to combine tests".into()),
    }
}
fn read(shell: &mut Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<Outcome> {
    let raw = args.first().is_some_and(|a| a == "-r");
    let args = if raw { &args[1..] } else { args };
    let default = [OsString::from("REPLY")];
    let names = if args.is_empty() { &default[..] } else { args };
    if names
        .iter()
        .any(|n| n.to_str().is_none_or(|n| !crate::syntax::valid_name(n)))
    {
        return fail(io, "read", "invalid variable name", 2);
    }
    let mut bytes = Vec::new();
    let mut eof = false;
    loop {
        let Some(byte) = io.read_byte()? else {
            eof = true;
            break;
        };
        if byte == b'\n' {
            break;
        }
        if byte == b'\\' && !raw {
            if let Some(next) = io.read_byte()? {
                if next != b'\n' {
                    bytes.push(next);
                }
            } else {
                eof = true;
                break;
            }
        } else {
            bytes.push(byte);
        }
    }
    let line = String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "read input must be UTF-8"))?;
    if args.is_empty() {
        shell.set_var("REPLY", line.into());
    } else {
        let ifs = shell
            .var("IFS")
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| " \t\n".into());
        let mut rest = line.trim_matches(|c| ifs.contains(c) && matches!(c, ' ' | '\t' | '\n'));
        for (index, name) in names.iter().enumerate() {
            let value = if index + 1 == names.len() {
                let value = rest;
                rest = "";
                value
            } else if let Some((pos, ch)) = rest.char_indices().find(|(_, c)| ifs.contains(*c)) {
                let value = &rest[..pos];
                rest = rest[pos + ch.len_utf8()..]
                    .trim_start_matches(|c| ifs.contains(c) && matches!(c, ' ' | '\t' | '\n'));
                value
            } else {
                let value = rest;
                rest = "";
                value
            };
            shell.set_var(name.to_str().unwrap(), value.into());
        }
    }
    Ok(Outcome::Continue(i32::from(eof)))
}
