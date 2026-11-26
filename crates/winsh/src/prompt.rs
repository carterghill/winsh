//! Interactive zsh-style prompt expansion, independent of the terminal frontend.
use crate::{Shell, ShellIo, expand, syntax};
use chrono::{DateTime, Local, format::StrftimeItems};
use std::{
    fmt::Write as _,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    str::Chars,
    sync::LazyLock,
};

pub(crate) const STARTER_PROFILE: &str = include_str!("../winshrc");

/// Fill the theme function's local variables. Its winshrc owns all colors and layout.
pub(crate) fn set_prompt_info(shell: &mut Shell) {
    let icon = match std::env::consts::OS {
        "windows" => "\u{f17a}",
        "macos" => "\u{f179}",
        _ => "\u{f17c}",
    };
    let path = directory(shell, true, 0);
    let path = path.to_string_lossy();
    #[cfg(windows)]
    let path = path.replace('\\', "/");
    let (parent, folder) = match path.rfind('/') {
        Some(i) if i + 1 < path.len() => path.split_at(i + 1),
        _ => ("", path.as_ref()),
    };
    let branch = git_head(shell).unwrap_or_default();
    shell.set_var("prompt_os", icon.into());
    for (name, text) in [
        ("prompt_parent", parent),
        ("prompt_folder", folder),
        ("prompt_branch", &branch),
    ] {
        // Substitution precedes zsh percent expansion. Display file and ref
        // names literally even when they contain percent escapes or controls.
        let mut literal = String::with_capacity(text.len());
        for ch in text.chars() {
            if ch == '%' {
                literal.push_str("%%");
            } else if ch.is_control() {
                literal.extend(ch.escape_default());
            } else {
                literal.push(ch);
            }
        }
        shell.set_var(name, literal.into());
    }
}

// Only HEAD is needed. This also works for an unborn branch and does not
// start Git or scan the working tree on every input line.
fn git_head(shell: &Shell) -> Option<String> {
    let dir = if let Some(dir) = shell.var("GIT_DIR").filter(|v| !v.is_empty()) {
        shell.cwd().join(dir)
    } else {
        shell.cwd().ancestors().find_map(|parent| {
            let dotgit = parent.join(".git");
            if dotgit.exists() {
                Some(dotgit)
            } else if parent.join("HEAD").is_file() && parent.join("objects").is_dir() {
                Some(parent.to_owned()) // Bare repository.
            } else {
                None
            }
        })?
    };
    let dir = if dir.is_file() {
        // A linked worktree or submodule can use a relative gitdir pointer.
        let pointer = read_git_line(&dir)?;
        dir.parent()?.join(pointer.strip_prefix("gitdir: ")?)
    } else {
        dir
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

pub(crate) fn ensure_profile(path: &Path) -> io::Result<()> {
    if path.try_exists()? {
        return Ok(());
    }
    let directory = path.parent().expect("winshrc has a config directory");
    fs::create_dir_all(directory)?;
    // Publish the complete script at once, even when two tabs open together.
    let mut temp = tempfile::NamedTempFile::new_in(directory)?;
    temp.write_all(STARTER_PROFILE.as_bytes())?;
    match temp.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error),
    }
}

#[derive(Default)]
pub struct Prompt {
    /// Text sent to the terminal, including styles.
    pub display: String,
    /// Printable text used by the line editor to measure the cursor position.
    pub visible: String,
}

impl Shell {
    pub fn render_prompt(&self, continuation: bool, io: &mut ShellIo) -> io::Result<Prompt> {
        let name = if continuation { "PS2" } else { "PS1" };
        let mut template = self.var(name).unwrap_or_default().to_string_lossy();
        if self.options.prompt_subst && template.contains(['$', '\\', '`']) {
            // Prompt commands see the last command's status and shell functions.
            // Expansion assignments, exit, and substitution status stay in the copy.
            let result = syntax::parse_document_body(&template)
                .map_err(|e| e.to_string())
                .and_then(|word| {
                    expand::scalar(&word, &mut self.subshell(), io).map_err(|e| e.to_string())
                });
            match result {
                Ok(value) => template = value.to_string_lossy().into_owned().into(),
                Err(error) => {
                    io.err(format_args!("winsh: {name}: {error}\n"))?;
                    template = if continuation { "> " } else { "%d $ " }.into();
                }
            }
        }
        let mut renderer = Renderer {
            shell: self,
            prompt: Prompt::default(),
            hidden: 0,
            now: None,
        };
        renderer.section(&mut template.chars(), None, true, 0);
        // A user function may emit ANSI sequences directly, outside %{...%}.
        if renderer.prompt.visible.contains('\x1b') {
            static ANSI: LazyLock<regex::Regex> = LazyLock::new(|| {
                regex::Regex::new(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\))")
                    .unwrap()
            });
            renderer.prompt.visible = ANSI.replace_all(&renderer.prompt.visible, "").into_owned();
        }
        Ok(renderer.prompt)
    }
}

struct Renderer<'a> {
    shell: &'a Shell,
    prompt: Prompt,
    hidden: usize,
    now: Option<DateTime<Local>>,
}

impl Renderer<'_> {
    fn text(&mut self, value: &str) {
        self.prompt.display.push_str(value);
        if self.hidden == 0 {
            self.prompt.visible.push_str(value);
        }
    }

    // Parse both arms of a conditional so nested escapes consume their own
    // delimiters. Only the selected arm contributes text or styling.
    fn section(&mut self, chars: &mut Chars<'_>, end: Option<char>, emit: bool, depth: u8) {
        while let Some(ch) = chars.next() {
            if Some(ch) == end {
                break;
            }
            if ch != '%' {
                if emit {
                    self.text(ch.encode_utf8(&mut [0; 4]));
                }
                continue;
            }
            let start = chars.as_str();
            let count = number(chars);
            let Some(code) = chars.next() else {
                if emit {
                    self.text("%");
                    self.text(start);
                }
                break;
            };
            if code == '(' && depth < 32 {
                let inner_count = number(chars).or(count).unwrap_or(0);
                let condition = chars.next();
                if let Some(separator) = chars.next() {
                    let matched = match condition {
                        Some('?') => self.shell.last_status() == inner_count,
                        Some('!') => privileged(),
                        _ => false,
                    };
                    self.section(chars, Some(separator), emit && matched, depth + 1);
                    self.section(chars, Some(')'), emit && !matched, depth + 1);
                }
                continue;
            }
            let argument = if matches!(code, 'F' | 'K' | 'D') {
                braced(chars)
            } else {
                None
            };
            if emit && !self.escape(code, count, argument) {
                self.text("%");
                self.text(&start[..start.len() - chars.as_str().len()]);
            }
        }
    }

    fn escape(&mut self, code: char, count: Option<i32>, argument: Option<&str>) -> bool {
        match code {
            '%' => self.text("%"),
            ')' => self.text(")"),
            'n' => {
                let user = ["USERNAME", "USER", "LOGNAME"]
                    .into_iter()
                    .find_map(|name| self.shell.var(name).filter(|v| !v.is_empty()));
                self.text(&user.unwrap_or("user".as_ref()).to_string_lossy());
            }
            'm' | 'M' => {
                let host = ["HOST", "COMPUTERNAME", "HOSTNAME"]
                    .into_iter()
                    .find_map(|name| self.shell.var(name).filter(|v| !v.is_empty()))
                    .map(|v| v.to_string_lossy())
                    .unwrap_or_else(|| system_host().into());
                let parts: Vec<_> = host.split('.').collect();
                let n = count.unwrap_or(1);
                let host = if code == 'M' || n == 0 {
                    host
                } else if n < 0 {
                    parts[parts.len().saturating_sub(n.unsigned_abs() as usize)..]
                        .join(".")
                        .into()
                } else {
                    parts[..parts.len().min(n as usize)].join(".").into()
                };
                self.text(&host);
            }
            '~' | '/' | 'd' | 'C' | 'c' | '.' => {
                let count = count.unwrap_or(i32::from(matches!(code, 'C' | 'c' | '.')));
                let path = directory(self.shell, matches!(code, '~' | 'c' | '.'), count);
                self.text(&path.to_string_lossy());
            }
            '#' => self.text(if privileged() { "#" } else { "%" }),
            '?' => self.text(&self.shell.last_status().to_string()),
            '{' => self.hidden += 1,
            '}' => self.hidden = self.hidden.saturating_sub(1),
            'F' | 'K' => {
                let color = argument
                    .map(str::to_owned)
                    .or_else(|| count.map(|n| n.to_string()));
                if let Some(style) = color_style(code, color.as_deref().unwrap_or("default")) {
                    self.prompt.display.push_str(&style);
                }
            }
            'D' | 'T' | '*' | 't' | '@' | 'w' | 'W' => {
                let format = match code {
                    'D' => argument.unwrap_or("%y-%m-%d"),
                    'T' => "%H:%M",
                    '*' => "%H:%M:%S",
                    't' | '@' => "%-I:%M%P",
                    'w' => "%a %e",
                    _ => "%m/%d/%y",
                };
                let time = self.now.get_or_insert_with(Local::now);
                let mut text = String::new();
                if write!(
                    text,
                    "{}",
                    time.format_with_items(StrftimeItems::new(format))
                )
                .is_err()
                {
                    return false;
                }
                self.text(&text);
            }
            _ => {
                let style = match code {
                    'f' => "\x1b[39m",
                    'k' => "\x1b[49m",
                    'B' => "\x1b[1m",
                    'b' => "\x1b[22m",
                    'U' => "\x1b[4m",
                    'u' => "\x1b[24m",
                    'S' => "\x1b[7m",
                    's' => "\x1b[27m",
                    'E' => "\x1b[K",
                    _ => return false,
                };
                self.prompt.display.push_str(style);
            }
        }
        true
    }
}

fn number(chars: &mut Chars<'_>) -> Option<i32> {
    let input = chars.as_str();
    let unsigned = input.strip_prefix('-').unwrap_or(input);
    let digits = unsigned.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let len = input.len() - unsigned.len() + digits;
    *chars = input[len..].chars();
    Some(input[..len].parse().unwrap_or(i32::MAX))
}

fn braced<'a>(chars: &mut Chars<'a>) -> Option<&'a str> {
    let (argument, rest) = chars.as_str().strip_prefix('{')?.split_once('}')?;
    *chars = rest.chars();
    Some(argument)
}

fn directory(shell: &Shell, short_home: bool, count: i32) -> PathBuf {
    let mut path = shell.cwd().to_path_buf();
    if short_home
        && let Some(home) = shell.home().filter(|v| !v.is_empty())
        && let Ok(tail) = path.strip_prefix(home)
    {
        path = if tail.as_os_str().is_empty() {
            "~".into()
        } else {
            Path::new("~").join(tail)
        };
    }
    if count == 0 {
        return path;
    }
    let n = count.unsigned_abs() as usize;
    // Count directory names; the drive and root belong to the first component.
    let names = path
        .components()
        .filter(|part| matches!(part, std::path::Component::Normal(_)))
        .count();
    if n > names || names == 0 {
        return path;
    }
    if count < 0 {
        path.ancestors().nth(names - n).unwrap().into()
    } else {
        path.components()
            .skip(path.components().count() - n)
            .collect()
    }
}

fn color_style(code: char, color: &str) -> Option<String> {
    let base = if code == 'F' { 30 } else { 40 };
    let names = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    if let Some(index) = names.iter().position(|name| *name == color) {
        Some(format!("\x1b[{}m", base + index))
    } else if color == "default" {
        Some(format!("\x1b[{}m", base + 9))
    } else if let Ok(index) = color.parse::<u8>() {
        Some(format!("\x1b[{};5;{index}m", base + 8))
    } else {
        let hex = color.strip_prefix('#')?;
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let rgb = u32::from_str_radix(hex, 16).ok()?;
        Some(format!(
            "\x1b[{};2;{};{};{}m",
            base + 8,
            rgb >> 16,
            (rgb >> 8) & 255,
            rgb & 255
        ))
    }
}

#[allow(unsafe_code)]
fn privileged() -> bool {
    #[cfg(unix)]
    // SAFETY: geteuid takes no arguments.
    unsafe {
        libc::geteuid() == 0
    }
    #[cfg(windows)]
    // SAFETY: checks membership in the current token's enabled Administrators group.
    unsafe {
        windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0
    }
}

fn system_host() -> &'static str {
    #[cfg(unix)]
    {
        static HOST: LazyLock<String> = LazyLock::new(|| {
            let mut bytes = [0u8; 256];
            // SAFETY: the buffer is writable for the supplied length.
            #[allow(unsafe_code)]
            if unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } == 0 {
                let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                return String::from_utf8_lossy(&bytes[..len]).into_owned();
            }
            "localhost".into()
        });
        &HOST
    }
    #[cfg(windows)]
    {
        "localhost"
    }
}
