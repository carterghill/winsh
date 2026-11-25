use crate::{Shell, ShellIo, platform};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, Metadata},
    io::{self, IsTerminal, Write},
    path::Path,
};
use unicode_width::UnicodeWidthStr;

mod mappings;

// The first space gives a wide Nerd Font icon room to draw. The second
// separates it from the filename in both the grid and one-line output.
const ICON_SPACES: &str = "  ";

#[derive(Default)]
struct Options {
    hidden: bool,
    dot_dirs: bool,
    list_dirs: bool,
    directories_first: bool,
    colors: bool,
    icons: bool,
    one_line: bool,
}

fn options(mut args: &[OsString], tty: bool) -> Result<(Options, &[OsString]), String> {
    let mut opts = Options::default();
    while let Some((arg, rest)) = args.split_first() {
        let Some(flag) = arg.to_str().filter(|s| s.starts_with('-') && *s != "-") else {
            break;
        };
        args = rest;
        if flag == "--" {
            break;
        }
        if flag == "--directories-first" || flag == "--group-directories-first" {
            opts.directories_first = true;
        } else if flag.starts_with("--") {
            let (name, when) = flag.split_once('=').unwrap_or((flag, "auto"));
            let setting = match name {
                "--color" | "--colors" => &mut opts.colors,
                "--icons" => &mut opts.icons,
                _ => return Err(format!("unknown option {flag}")),
            };
            *setting = match when {
                "auto" | "automatic" => tty,
                "always" => true,
                "never" => false,
                _ => return Err(format!("{name}: expected auto, always, or never")),
            };
        } else {
            for short in flag[1..].chars() {
                match short {
                    '1' => opts.one_line = true,
                    'a' => {
                        opts.hidden = true;
                        opts.dot_dirs = true;
                    }
                    'A' => opts.hidden = true,
                    'd' => opts.list_dirs = true,
                    _ => return Err(format!("unknown option -{short}")),
                }
            }
        }
    }
    Ok((opts, args))
}

struct Entry {
    name: OsString,
    directory: bool,
    text: String,
    width: usize,
}

impl Entry {
    fn new(
        name: OsString,
        path: &Path,
        meta: Option<&Metadata>,
        opts: &Options,
        tty: bool,
    ) -> Self {
        let symlink = meta.is_some_and(Metadata::is_symlink);
        let target = symlink.then(|| fs::metadata(path).ok()).flatten();
        let directory = target.as_ref().or(meta).is_some_and(Metadata::is_dir);
        let label = display_name(&name, tty);
        let width = label.width() + usize::from(opts.icons) * (1 + ICON_SPACES.len());
        if !opts.colors && !opts.icons {
            return Self {
                name,
                directory,
                text: label,
                width,
            };
        }
        let basename = path.file_name().and_then(OsStr::to_str).unwrap_or("");
        let extension = path
            .extension()
            .and_then(OsStr::to_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let style = if !opts.colors {
            ""
        } else if symlink {
            if target.is_some() { "36" } else { "31" }
        } else if directory {
            "1;34"
        } else {
            file_style(basename, &extension, meta)
        };
        let mut text = String::new();
        if opts.icons {
            let icon = if directory {
                mappings::directory_icon(basename).unwrap_or_else(|| {
                    if fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none()) {
                        '\u{f115}'
                    } else {
                        '\u{e5ff}'
                    }
                })
            } else {
                mappings::name_icon(basename)
                    .or_else(|| mappings::extension_icon(&extension))
                    .unwrap_or(if extension.is_empty() {
                        '\u{f086f}'
                    } else {
                        '\u{f15b}'
                    })
            };
            // Icons inherit the foreground hue. Bold and underline belong to the filename.
            let color = style.rsplit(';').next().filter(|s| s.starts_with('3'));
            if let Some(color) = color {
                text.push_str(&format!("\x1b[{color}m{icon}\x1b[0m"));
            } else {
                text.push(icon);
            }
            text.push_str(ICON_SPACES);
        }
        if style.is_empty() {
            text.push_str(&label);
        } else {
            text.push_str(&format!("\x1b[{style}m{label}\x1b[0m"));
        }
        Self {
            name,
            directory,
            text,
            width,
        }
    }
}

fn display_name(name: &OsStr, tty: bool) -> String {
    let text = name.to_string_lossy();
    if tty && text.chars().any(char::is_control) {
        let mut escaped = String::new();
        for ch in text.chars() {
            if ch.is_control() {
                escaped.extend(ch.escape_default());
            } else {
                escaped.push(ch);
            }
        }
        escaped
    } else {
        text.into_owned()
    }
}

fn file_style(name: &str, ext: &str, meta: Option<&Metadata>) -> &'static str {
    #[cfg(unix)]
    if let Some(meta) = meta {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        let kind = meta.file_type();
        if kind.is_fifo() {
            return "33";
        }
        if kind.is_socket() {
            return "1;31";
        }
        if kind.is_char_device() || kind.is_block_device() {
            return "1;33";
        }
        if meta.permissions().mode() & 0o111 != 0 {
            return "1;32";
        }
    }
    #[cfg(windows)]
    if meta.is_some_and(Metadata::is_file) && matches!(ext, "exe" | "com" | "cmd" | "bat") {
        return "1;32";
    }
    if name
        .get(..6)
        .is_some_and(|s| s.eq_ignore_ascii_case("readme"))
    {
        return "1;4;33";
    }
    if let Some(style) = mappings::name_style(name).or_else(|| mappings::extension_style(ext)) {
        return style;
    }
    if name.ends_with('~') || (name.starts_with('#') && name.ends_with('#')) {
        "2"
    } else {
        ""
    }
}

#[cfg(windows)]
fn terminal_width(file: &File) -> Option<usize> {
    let rect = winapi_util::console::screen_buffer_info(file)
        .ok()?
        .window_rect();
    usize::try_from(i32::from(rect.right) - i32::from(rect.left) + 1).ok()
}

#[cfg(unix)]
fn terminal_width(file: &File) -> Option<usize> {
    Some(usize::from(
        rustix::termios::tcgetwinsize(file).ok()?.ws_col,
    ))
}

fn print_entries(
    entries: &mut [Entry],
    directories_first: bool,
    width: Option<usize>,
    output: &mut dyn Write,
) -> io::Result<()> {
    entries.sort_unstable_by(|a, b| {
        (directories_first && b.directory)
            .cmp(&(directories_first && a.directory))
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut rows = entries.len();
    let mut widths = Vec::new();
    if let Some(width) = width
        && !entries.is_empty()
    {
        let shortest = entries.iter().map(|e| e.width).min().unwrap();
        let max_cols = entries.len().min((width + 2) / (shortest + 2));
        let mut previous = 0;
        for cols in (2..=max_cols).rev() {
            let candidate = entries.len().div_ceil(cols);
            if candidate == previous {
                continue;
            }
            previous = candidate;
            widths.clear();
            widths.extend(
                entries
                    .chunks(candidate)
                    .map(|col| col.iter().map(|e| e.width).max().unwrap()),
            );
            if widths.iter().sum::<usize>() + 2 * (widths.len() - 1) <= width {
                rows = candidate;
                break;
            }
        }
    }
    for row in 0..rows {
        for (col, index) in (row..entries.len()).step_by(rows).enumerate() {
            let entry = &entries[index];
            write!(output, "{}", entry.text)?;
            if index + rows < entries.len() {
                write!(
                    output,
                    "{:padding$}",
                    "",
                    padding = widths[col] + 2 - entry.width
                )?;
            }
        }
        writeln!(output)?;
    }
    Ok(())
}

pub(super) fn run(shell: &Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<i32> {
    let terminal = io.output_file()?.filter(File::is_terminal);
    let tty = terminal.is_some();
    let (opts, args) = match options(args, tty) {
        Ok(value) => value,
        Err(message) => {
            io.err(format_args!("ls: {message}\n"))?;
            return Ok(2);
        }
    };
    #[cfg(windows)]
    let opts = {
        let colors = opts.colors
            && terminal
                .as_ref()
                .is_none_or(platform::enable_virtual_terminal_output);
        Options { colors, ..opts }
    };
    let width = terminal.as_ref().filter(|_| !opts.one_line).map(|file| {
        shell
            .var("COLUMNS")
            .and_then(OsStr::to_str)
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|&n| n != 0)
            .map(usize::from)
            .or_else(|| terminal_width(file).filter(|&w| w > 0))
            .unwrap_or(80)
    });
    let current = [OsString::from(".")];
    let operands = if args.is_empty() { &current[..] } else { args };
    let mut files = Vec::new();
    let mut directories = Vec::new();
    let mut status = 0;
    for name in operands {
        let path = platform::absolute(shell.cwd(), Path::new(name));
        match path.and_then(|path| Ok((fs::symlink_metadata(&path)?, path))) {
            Ok((meta, path))
                if !opts.list_dirs && (meta.is_dir() || (meta.is_symlink() && path.is_dir())) =>
            {
                directories.push((name, path));
            }
            Ok((meta, path)) => {
                files.push(Entry::new(name.clone(), &path, Some(&meta), &opts, tty))
            }
            Err(error) => {
                io.err(format_args!("ls: {}: {error}\n", name.to_string_lossy()))?;
                status = 2;
            }
        }
    }
    let mut printed = !files.is_empty();
    print_entries(&mut files, opts.directories_first, width, io.output())?;
    directories.sort_unstable();
    for (name, path) in directories {
        let entries = match fs::read_dir(&path) {
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
            writeln!(io.output(), "{}:", display_name(name, tty))?;
            printed = true;
        }
        let mut listed = Vec::new();
        let details = opts.directories_first || opts.colors || opts.icons;
        if opts.dot_dirs {
            for dot in [".", ".."] {
                let path = path.join(dot);
                let meta = details.then(|| fs::metadata(&path).ok()).flatten();
                listed.push(Entry::new(dot.into(), &path, meta.as_ref(), &opts, tty));
            }
        }
        for item in entries {
            match item {
                Ok(entry) => {
                    let name = entry.file_name();
                    if opts.hidden || !name.as_encoded_bytes().starts_with(b".") {
                        let meta = details.then(|| entry.metadata().ok()).flatten();
                        listed.push(Entry::new(name, &entry.path(), meta.as_ref(), &opts, tty));
                    }
                }
                Err(error) => {
                    io.err(format_args!("ls: {}: {error}\n", name.to_string_lossy()))?;
                    status = status.max(1);
                }
            }
        }
        print_entries(&mut listed, opts.directories_first, width, io.output())?;
    }
    io.output().flush()?;
    Ok(status)
}
