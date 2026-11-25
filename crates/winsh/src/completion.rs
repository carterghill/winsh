//! Literal command and path completion against a snapshot of the shell session.
use std::{
    collections::BTreeSet,
    env,
    ffi::OsString,
    fs,
    path::{self, Path, PathBuf},
};

use crate::{
    Shell, Utility, builtins, exec, platform,
    syntax::{Quote, Target, WordPart, WordPartKind, completion_word},
};

pub struct CompletionContext {
    cwd: PathBuf,
    path: Option<OsString>,
    home: Option<PathBuf>,
    functions: Vec<String>,
}

impl Shell {
    pub fn completion_context(&self) -> CompletionContext {
        CompletionContext {
            cwd: self.cwd.clone(),
            path: self.env("PATH").map(ToOwned::to_owned),
            home: self.home().map(PathBuf::from),
            functions: self.functions.keys().cloned().collect(),
        }
    }
}

impl CompletionContext {
    /// Return (menu label, text to insert at the cursor) for each match.
    pub fn candidates(&self, before: &str, after: &str) -> Vec<(String, String)> {
        let Some((target, word, quote)) = completion_word(before) else {
            return vec![];
        };
        let mut token = String::new();
        if !literal(&word.parts, false, &mut token) {
            return vec![];
        }
        let tilde = matches!(word.parts.first().map(|p| &p.kind), Some(WordPartKind::Text(s)) if s.starts_with('~'));
        let split = token.rfind(path::is_separator).map_or(0, |index| index + 1);
        let (parent, prefix) = token.split_at(split);
        let mut names = BTreeSet::new();
        if tilde && token == "~" && self.home.is_some() {
            names.insert("~/".to_owned());
        } else if target == Target::Command && parent.is_empty() {
            let commands = builtins::names().chain(Utility::COMMANDS.iter().map(|(name, _)| *name));
            names.extend(
                commands
                    .filter(|name| name.starts_with(prefix))
                    .map(str::to_owned),
            );
            names.extend(
                self.functions
                    .iter()
                    .filter(|name| name.starts_with(prefix))
                    .cloned(),
            );
            if let Some(paths) = &self.path {
                for dir in env::split_paths(paths) {
                    if let Ok(dir) = platform::absolute(&self.cwd, &dir) {
                        entries(&dir, prefix, Target::Command, false, &mut names);
                    }
                }
            }
            entries(&self.cwd, prefix, Target::Directory, true, &mut names);
        } else {
            let directory = if tilde {
                self.home
                    .as_ref()
                    .zip(parent.strip_prefix("~/"))
                    .map(|(home, rest)| home.join(rest.trim_start_matches(path::is_separator)))
            } else {
                Some(PathBuf::from(parent))
            };
            if let Some(dir) = directory.and_then(|p| platform::absolute(&self.cwd, &p).ok()) {
                entries(&dir, prefix, target, true, &mut names);
            }
        }
        names
            .into_iter()
            .map(|name| {
                let addition = insertion(&name[prefix.len()..], name.ends_with('/'), quote, after);
                (name, addition)
            })
            .collect()
    }
}

fn literal(parts: &[WordPart], quoted: bool, result: &mut String) -> bool {
    for part in parts {
        match &part.kind {
            WordPartKind::Text(s)
                if !s.contains('$') && (quoted || !s.contains(['*', '?', '['])) =>
            {
                result.push_str(s)
            }
            WordPartKind::SingleQuoted(s) => result.push_str(s),
            WordPartKind::DoubleQuoted(parts) if literal(parts, true, result) => {}
            WordPartKind::Escaped(c) => result.push(*c),
            _ => return false,
        }
    }
    true
}

fn entries(
    dir: &Path,
    prefix: &str,
    target: Target,
    directories: bool,
    names: &mut BTreeSet<String>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    if directories && matches!(prefix, "." | "..") {
        names.insert(format!("{prefix}/"));
    }
    for entry in entries.flatten() {
        let Ok(mut name) = entry.file_name().into_string() else {
            continue;
        };
        let matches = name.starts_with(prefix)
            || (cfg!(windows)
                && name
                    .get(..prefix.len())
                    .is_some_and(|s| s.eq_ignore_ascii_case(prefix)));
        if !matches
            || name.chars().any(char::is_control)
            || (name.starts_with('.') && !prefix.starts_with('.'))
        {
            continue;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() || (kind.is_symlink() && path.is_dir()) {
            if !directories {
                continue;
            }
            name.push('/');
        } else if target == Target::Directory || (target == Target::Command && !is_command(&path)) {
            continue;
        }
        names.insert(name);
    }
}

fn is_command(path: &Path) -> bool {
    #[cfg(windows)]
    if !path.extension().is_some_and(|ext| {
        ["exe", "com", "sh"]
            .iter()
            .any(|name| ext.eq_ignore_ascii_case(name))
    }) {
        return false;
    }
    exec::executable_file(path).is_ok()
}

fn insertion(rest: &str, directory: bool, quote: Option<Quote>, after: &str) -> String {
    let mut text = String::new();
    for ch in rest.chars() {
        match quote {
            Some(Quote::Single) if ch == '\'' => text.push_str("'\\''"),
            Some(Quote::Double) if matches!(ch, '\\' | '"' | '$' | '`') => {
                text.push('\\');
                text.push(ch);
            }
            None if !ch.is_alphanumeric() && !"_./-:+,@%".contains(ch) => {
                text.push('\\');
                text.push(ch);
            }
            _ => text.push(ch),
        }
    }
    // Keep text to the right of the cursor, including an existing closing
    // quote or the rest of a filename being edited in the middle of a line.
    if !after.is_empty() {
        if let Some((i, _)) = text
            .char_indices()
            .find(|(i, _)| after.starts_with(&text[*i..]))
        {
            text.truncate(i);
        }
        return text;
    }
    match quote {
        Some(Quote::Single) => text.push('\''),
        Some(Quote::Double) => text.push('"'),
        None => {}
    }
    if !directory {
        text.push(' ');
    }
    text
}
