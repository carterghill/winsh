use super::{open_input, operands, output_id};
use crate::{Shell, ShellIo, platform};
use regex::bytes::{RegexSet, RegexSetBuilder};
use std::{
    ffi::{OsStr, OsString},
    io::{self, BufRead, BufReader},
    path::Path,
};
use walkdir::WalkDir;

#[derive(Default, PartialEq)]
enum Syntax {
    #[default]
    Basic,
    Extended,
    Fixed,
}

#[derive(Default)]
struct Options {
    syntax: Syntax,
    ignore_case: bool,
    numbered: bool,
    invert: bool,
    count: bool,
    quiet: bool,
    recursive: bool,
    filenames: Option<bool>,
}

impl Options {
    fn parse(mut args: &[OsString]) -> Result<(Self, Vec<&str>, &[OsString]), String> {
        let mut options = Self::default();
        let mut patterns = Vec::new();
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
            args = rest;
            let mut flags = flags.chars();
            while let Some(flag) = flags.next() {
                match flag {
                    'i' => options.ignore_case = true,
                    'n' => options.numbered = true,
                    'v' => options.invert = true,
                    'c' => options.count = true,
                    'q' => options.quiet = true,
                    'r' => options.recursive = true,
                    'H' => options.filenames = Some(true),
                    'h' => options.filenames = Some(false),
                    'E' if options.syntax != Syntax::Fixed => options.syntax = Syntax::Extended,
                    'F' if options.syntax != Syntax::Extended => options.syntax = Syntax::Fixed,
                    'E' | 'F' => return Err("choose one of -E and -F".into()),
                    'e' => {
                        let pattern = if flags.as_str().is_empty() {
                            let (arg, rest) = args.split_first().ok_or("-e requires a pattern")?;
                            args = rest;
                            arg.to_str().ok_or("pattern must be UTF-8")?
                        } else {
                            flags.as_str()
                        };
                        patterns.push(pattern);
                        break;
                    }
                    _ => return Err(format!("unknown option -{flag}")),
                }
            }
        }
        if patterns.is_empty() {
            let (arg, rest) = args.split_first().ok_or("expected a pattern")?;
            patterns.push(arg.to_str().ok_or("pattern must be UTF-8")?);
            args = rest;
        }
        Ok((options, patterns, args))
    }
    fn regex(&self, patterns: &[&str]) -> Result<RegexSet, String> {
        let expressions = patterns
            .iter()
            .flat_map(|pattern| {
                pattern
                    .split_terminator('\n')
                    .chain(pattern.is_empty().then_some(""))
            })
            .map(|pattern| match self.syntax {
                Syntax::Fixed => regex::escape(pattern),
                Syntax::Extended | Syntax::Basic => {
                    expression(pattern, self.syntax == Syntax::Basic)
                }
            });
        RegexSetBuilder::new(expressions)
            .case_insensitive(self.ignore_case)
            .build()
            .map_err(|error| format!("invalid pattern: {error}"))
    }
}

pub(super) fn reads_stdin(args: &[OsString]) -> bool {
    Options::parse(args).is_ok_and(|(opts, _, files)| {
        (files.is_empty() && !opts.recursive) || files.iter().any(|f| f == "-")
    })
}

// Translate the POSIX spelling of literals and BRE operators for the regex engine.
fn expression(pattern: &str, basic: bool) -> String {
    let mut result = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if !basic || !chars.peek().is_some_and(|c| "()+?|{}".contains(*c)) {
                    result.push('\\');
                }
                if let Some(escaped) = chars.next() {
                    result.push(escaped);
                }
            }
            '[' => {
                result.push('[');
                if chars.next_if_eq(&'^').is_some() {
                    result.push('^');
                }
                if chars.next_if_eq(&']').is_some() {
                    result.push_str(r"\]");
                }
                while let Some(member) = chars.next() {
                    match member {
                        ']' => {
                            result.push(']');
                            break;
                        }
                        '[' if chars.peek() == Some(&':') => {
                            result.push('[');
                            for ch in chars.by_ref() {
                                result.push(ch);
                                if ch == ']' {
                                    break;
                                }
                            }
                        }
                        '[' | '&' | '~' | '\\' => {
                            result.push('\\');
                            result.push(member);
                        }
                        other => result.push(other),
                    }
                }
            }
            '(' | ')' | '+' | '?' | '|' | '{' | '}' if basic => {
                result.push('\\');
                result.push(ch);
            }
            _ => result.push(ch),
        }
    }
    result
}

pub(super) fn run(shell: &Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<i32> {
    let (options, regex, files) =
        match Options::parse(args).and_then(|(options, patterns, files)| {
            let regex = options.regex(&patterns)?;
            Ok((options, regex, files))
        }) {
            Ok(parsed) => parsed,
            Err(error) => {
                io.err(format_args!("grep: {error}\n"))?;
                return Ok(2);
            }
        };
    let output = if options.quiet || options.count {
        None
    } else {
        output_id(io)?
    };
    let implicit_directory = options.recursive && files.is_empty();
    let cwd = [OsString::from(".")];
    let files = if implicit_directory { &cwd[..] } else { files };
    let mut search = Search {
        options,
        regex,
        output,
        matched: false,
        failed: false,
        line: Vec::new(),
    };
    for name in operands(files) {
        let root = if search.options.recursive && name != "-" {
            platform::absolute(shell.cwd(), Path::new(name))
                .ok()
                .filter(|p| p.is_dir())
        } else {
            None
        };
        if let Some(root) = root {
            let walk = WalkDir::new(&root)
                .min_depth(1)
                .sort_by_file_name()
                .into_iter();
            #[cfg(windows)]
            let walk = walk.filter_entry(|entry| {
                // WalkDir caches directory metadata on Windows. Also skip
                // non-symlink directory reparse points.
                !entry.file_type().is_dir()
                    || entry
                        .metadata()
                        .is_ok_and(|m| !super::files::directory_reparse_point(&m))
            });
            for entry in walk {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        io.err(format_args!("grep: {error}\n"))?;
                        search.failed = true;
                        continue;
                    }
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                let relative = entry
                    .path()
                    .strip_prefix(&root)
                    .expect("walk is rooted at this operand");
                let mut label = if implicit_directory {
                    String::new()
                } else {
                    name.to_string_lossy().into_owned()
                };
                for part in relative.iter() {
                    if !label.is_empty() && !label.ends_with(std::path::is_separator) {
                        label.push('/');
                    }
                    label.push_str(&part.to_string_lossy());
                }
                search.file(shell, io, entry.path().as_os_str(), &label, true)?;
                if search.options.quiet && search.matched {
                    return Ok(0);
                }
            }
        } else {
            let label = if name == "-" {
                "(standard input)".into()
            } else {
                name.to_string_lossy()
            };
            search.file(shell, io, name, &label, files.len() > 1)?;
            if search.options.quiet && search.matched {
                return Ok(0);
            }
        }
    }
    io.output().flush()?;
    Ok(if search.failed {
        2
    } else {
        i32::from(!search.matched)
    })
}

struct Search {
    options: Options,
    regex: RegexSet,
    output: Option<same_file::Handle>,
    matched: bool,
    failed: bool,
    line: Vec<u8>,
}

impl Search {
    fn file(
        &mut self,
        shell: &Shell,
        io: &mut ShellIo,
        name: &OsStr,
        label: &str,
        default_prefix: bool,
    ) -> io::Result<()> {
        let options = &self.options;
        let prefix = options.filenames.unwrap_or(default_prefix);
        let input = match open_input(shell, io, name, self.output.as_ref()) {
            Ok(input) => input,
            Err(error) => {
                io.err(format_args!("grep: {label}: {error}\n"))?;
                self.failed = true;
                return Ok(());
            }
        };
        let mut input = BufReader::new(input);
        let mut selected = 0u64;
        let mut line_number = 0u64;
        let line = &mut self.line;
        loop {
            line.clear();
            match input.read_until(b'\n', line) {
                Ok(0) => break,
                Ok(_) => line_number += 1,
                Err(error) => {
                    io.err(format_args!("grep: {label}: {error}\n"))?;
                    self.failed = true;
                    break;
                }
            }
            let text = line
                .strip_suffix(b"\r\n")
                .or_else(|| line.strip_suffix(b"\n"))
                .unwrap_or(line);
            if self.regex.is_match(text) == options.invert {
                continue;
            }
            self.matched = true;
            if options.quiet {
                return Ok(());
            }
            selected += 1;
            if !options.count {
                if prefix {
                    write!(io.output(), "{label}:")?;
                }
                if options.numbered {
                    write!(io.output(), "{line_number}:")?;
                }
                io.output().write_all(line)?;
                if !line.ends_with(b"\n") {
                    io.output().write_all(b"\n")?;
                }
            }
        }
        if options.count {
            if prefix {
                write!(io.output(), "{label}:")?;
            }
            writeln!(io.output(), "{selected}")?;
        }
        Ok(())
    }
}
