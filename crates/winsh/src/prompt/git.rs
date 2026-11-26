use crate::{Shell, exec};
use std::{
    fmt::Write as _,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Default)]
pub(super) struct Info {
    pub branch: String,
    pub changes: String,
    pub tracking: String,
}

pub(super) fn info(shell: &Shell) -> Info {
    let Some(dir) = git_dir(shell) else {
        return Info::default();
    };
    status(shell).unwrap_or_else(|| Info {
        branch: read_head(&dir).unwrap_or_default(),
        ..Info::default()
    })
}

fn git_dir(shell: &Shell) -> Option<PathBuf> {
    if let Some(dir) = shell.var("GIT_DIR").filter(|v| !v.is_empty()) {
        return Some(shell.cwd().join(dir));
    }
    shell.cwd().ancestors().find_map(|parent| {
        let dotgit = parent.join(".git");
        if dotgit.exists() {
            Some(dotgit)
        } else if parent.join("HEAD").is_file() && parent.join("objects").is_dir() {
            Some(parent.to_owned()) // Bare repository.
        } else {
            None
        }
    })
}

fn status(shell: &Shell) -> Option<Info> {
    let mut git = Command::new(exec::resolve(shell, "git".as_ref()).ok()?);
    git.current_dir(shell.cwd())
        .env_clear()
        .envs(shell.environment().iter().map(|(key, value)| (key, value)))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = shell.var("GIT_DIR").filter(|v| !v.is_empty()) {
        git.env("GIT_DIR", dir);
    }
    let output = git
        .args([
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--ahead-behind",
            "--untracked-files=normal",
        ])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| parse_status(&output.stdout))
}

fn parse_status(output: &[u8]) -> Info {
    let mut info = Info::default();
    let (mut staged, mut unstaged, mut untracked, mut conflicts) = (0usize, 0usize, 0usize, 0usize);
    let mut oid = "";
    let mut records = output.split(|&b| b == 0);
    while let Some(record) = records.next() {
        match record {
            [kind @ (b'1' | b'2'), b' ', x, y, b' ', ..] => {
                staged += usize::from(*x != b'.');
                unstaged += usize::from(*y != b'.');
                if *kind == b'2' {
                    // A rename or copy has a second NUL-delimited path.
                    records.next();
                }
            }
            [b'u', b' ', ..] => conflicts += 1,
            [b'?', b' ', ..] => untracked += 1,
            _ => {
                if let Some(name) = record.strip_prefix(b"# branch.head ") {
                    info.branch = String::from_utf8_lossy(name).into_owned();
                } else if let Some(hash) = record.strip_prefix(b"# branch.oid ") {
                    oid = std::str::from_utf8(hash).unwrap_or_default();
                } else if let Some(counts) = record.strip_prefix(b"# branch.ab ") {
                    let counts = std::str::from_utf8(counts).unwrap_or_default();
                    if let Some((ahead, behind)) =
                        counts.strip_prefix('+').and_then(|s| s.split_once(" -"))
                    {
                        append_count(&mut info.tracking, '↑', ahead.parse().unwrap_or(0));
                        append_count(&mut info.tracking, '↓', behind.parse().unwrap_or(0));
                    }
                }
            }
        }
    }
    if info.branch == "(detached)" {
        info.branch = oid.chars().take(8).collect();
    }
    for (marker, count) in [
        ('+', staged),
        ('!', unstaged),
        ('?', untracked),
        ('~', conflicts),
    ] {
        append_count(&mut info.changes, marker, count);
    }
    info
}

fn append_count(text: &mut String, marker: char, count: usize) {
    if count > 0 {
        write!(text, " {marker}{count}").unwrap();
    }
}

// Keep the branch visible in bare repositories and when Git is unavailable.
fn read_head(dir: &Path) -> Option<String> {
    let dir = if dir.is_file() {
        let pointer = read_git_line(dir)?;
        dir.parent()?.join(pointer.strip_prefix("gitdir: ")?)
    } else {
        dir.to_owned()
    };
    let head = read_git_line(&dir.join("HEAD"))?;
    if let Some(branch) = head.strip_prefix("ref: refs/heads/") {
        return Some(branch.to_owned());
    }
    (matches!(head.len(), 40 | 64) && head.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| head[..8].to_owned())
}

fn read_git_line(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let mut line = String::new();
    fs::File::open(path)
        .ok()?
        .take(8193)
        .read_to_string(&mut line)
        .ok()?;
    if line.len() > 8192 {
        return None;
    }
    line.truncate(line.trim_end_matches(['\r', '\n']).len());
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_records_handle_renames_conflicts_and_upstream_counts() {
        let info = parse_status(
            b"# branch.oid abcdef0123456789\0# branch.head (detached)\0# branch.ab +3 -2\0\
              2 RM N... 100644 100644 100644 a b R100 new\0? old name\0\
              u UU N... 100644 100644 100644 100644 a b c conflict\0? untracked\xff\0! ignored\0",
        );
        assert_eq!(info.branch, "abcdef01");
        assert_eq!(info.changes, " +1 !1 ?1 ~1");
        assert_eq!(info.tracking, " ↑3 ↓2");
    }
}
