use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io,
};

use crate::{
    Shell, ShellIo,
    shell::assignment,
    syntax::{SimpleCommand, Word, WordPartKind, parse_simple_command},
};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Alias {
    source: String,
    words: Vec<Word>,
}

fn display(name: &str, alias: &Alias, io: &mut ShellIo) -> io::Result<()> {
    let quoted = alias.source.replace('\'', "'\\''");
    io.out(format_args!("alias {name}='{quoted}'\n"))
}

pub(crate) fn define(shell: &mut Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<i32> {
    let args = args.strip_prefix(&[OsString::from("--")]).unwrap_or(args);
    if args.is_empty() {
        for (name, alias) in &shell.aliases {
            display(name, alias, io)?;
        }
    }
    let mut status = 0;
    for arg in args {
        let Some(arg) = arg.to_str() else {
            io.err(format_args!("alias: expected UTF-8 name and value\n"))?;
            status = 1;
            continue;
        };
        if let Some((name, source)) = arg.split_once('=') {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
            {
                io.err(format_args!("alias: invalid name: {name}\n"))?;
                status = 1;
                continue;
            }
            match parse_simple_command(source) {
                Ok(Some(command)) if command.redirections.is_empty() => {
                    shell.aliases.insert(
                        name.into(),
                        Alias {
                            source: source.into(),
                            words: command.words,
                        },
                    );
                }
                _ => {
                    io.err(format_args!(
                        "alias: {name}: expected a command and optional arguments\n"
                    ))?;
                    status = 1;
                }
            }
        } else if let Some(alias) = shell.aliases.get(arg) {
            display(arg, alias, io)?;
        } else {
            io.err(format_args!("alias: {arg}: not found\n"))?;
            status = 1;
        }
    }
    Ok(status)
}

pub(crate) fn remove(shell: &mut Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<i32> {
    if args == [OsString::from("-a")] {
        shell.aliases.clear();
        return Ok(0);
    }
    let args = args.strip_prefix(&[OsString::from("--")]).unwrap_or(args);
    if args.is_empty() {
        io.err(format_args!("unalias: expected an alias name\n"))?;
        return Ok(2);
    }
    let mut status = 0;
    for arg in args {
        if arg
            .to_str()
            .and_then(|name| shell.aliases.remove(name))
            .is_none()
        {
            io.err(format_args!(
                "unalias: {}: not found\n",
                arg.to_string_lossy()
            ))?;
            status = 1;
        }
    }
    Ok(status)
}

/// Substitute parsed words before shell expansion, preserving the caller's
/// quotes, redirects and arguments. Suppress each name for its own expansion.
pub(crate) fn expand(
    aliases: &BTreeMap<String, Alias>,
    simple: &SimpleCommand,
) -> Option<SimpleCommand> {
    if aliases.is_empty() {
        return None;
    }
    let mut expanded = None;
    let mut seen = BTreeSet::new();
    let mut start = 0;
    loop {
        let words = &expanded.as_ref().unwrap_or(simple).words;
        start += words[start..]
            .iter()
            .take_while(|w| assignment(w).is_some())
            .count();
        let Some(word) = words.get(start) else { break };
        let [part] = word.parts.as_slice() else { break };
        let WordPartKind::Text(name) = &part.kind else {
            break;
        };
        let Some((key, alias)) = aliases.get_key_value(name) else {
            break;
        };
        if !seen.insert(key) {
            break;
        }
        expanded
            .get_or_insert_with(|| simple.clone())
            .words
            .splice(start..=start, alias.words.iter().cloned());
    }
    expanded
}
