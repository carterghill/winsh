use super::Utility;
use crate::{Shell, ShellIo, platform};
use same_file::Handle;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, FileTimes, Metadata},
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub(super) fn run(
    utility: Utility,
    name: &str,
    allowed: &str,
    shell: &Shell,
    mut operands: &[OsString],
    io: &mut ShellIo,
) -> io::Result<i32> {
    let mut flags = String::new();
    while let Some((arg, rest)) = operands.split_first() {
        if arg == "--" {
            operands = rest;
            break;
        }
        let Some(options) = arg
            .to_str()
            .and_then(|s| s.strip_prefix('-'))
            .filter(|s| !s.is_empty())
        else {
            break;
        };
        for flag in options.chars() {
            if !allowed.contains(flag) {
                io.err(format_args!("{name}: unknown option -{flag}\n"))?;
                return Ok(1);
            }
            flags.push(flag);
        }
        operands = rest;
    }
    let recursive = flags.contains(['r', 'R']);
    let force = flags.contains('f');
    if operands.is_empty() {
        if matches!(utility, Utility::Rm) && force {
            return Ok(0);
        }
        io.err(format_args!("{name}: missing operand\n"))?;
        return Ok(1);
    }
    if matches!(utility, Utility::Cp | Utility::Mv) {
        return transfer(utility, name, shell, operands, recursive, io);
    }
    let mut status = 0;
    for operand in operands {
        let result = resolve(shell, operand).and_then(|path| match utility {
            Utility::Mkdir if flags.contains('p') => fs::create_dir_all(path),
            Utility::Mkdir => fs::create_dir(path),
            Utility::Touch => touch(&path, &flags),
            Utility::Rm => {
                protect_operand(operand, &path)?;
                match remove(&path, recursive) {
                    Err(e) if force && e.kind() == io::ErrorKind::NotFound => Ok(()),
                    result => result,
                }
            }
            _ => unreachable!(),
        });
        if let Err(error) = result {
            io.err(format_args!(
                "{name}: {}: {error}\n",
                operand.to_string_lossy()
            ))?;
            status = 1;
        }
    }
    Ok(status)
}

fn resolve(shell: &Shell, operand: &OsStr) -> io::Result<PathBuf> {
    if operand.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty path"));
    }
    platform::absolute(shell.cwd(), Path::new(operand))
}

fn separator(byte: &u8) -> bool {
    *byte == b'/' || (cfg!(windows) && *byte == b'\\')
}

fn ends_in_separator(path: &OsStr) -> bool {
    path.as_encoded_bytes().last().is_some_and(separator)
}

fn final_component(path: &OsStr) -> Option<&[u8]> {
    path.as_encoded_bytes()
        .rsplit(separator)
        .find(|s| !s.is_empty())
}

fn protect_operand(operand: &OsStr, path: &Path) -> io::Result<()> {
    // Path::components normalizes a trailing ".". Check the original spelling too.
    let last = final_component(operand);
    if matches!(last, Some(b"." | b".."))
        || path.parent().is_none()
        || (cfg!(windows) && last.is_some_and(|s| s.iter().all(|b| matches!(b, b'.' | b' '))))
    {
        return Err(io::Error::other("refusing a filesystem root, '.', or '..'"));
    }
    Ok(())
}

fn touch(path: &Path, flags: &str) -> io::Result<()> {
    if ends_in_separator(path.as_os_str())
        && let Err(error) = platform::require_directory(path)
    {
        return if flags.contains('c') && error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        };
    }
    let mut options = File::options();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_WRITE_ATTRIBUTES,
        };
        options
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.read(true).custom_flags(libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if flags.contains('c') {
                return Ok(());
            }
            File::options()
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)?
        }
        #[cfg(unix)]
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            File::options().write(true).open(path)?
        }
        Err(error) => return Err(error),
    };
    let now = SystemTime::now();
    let mut times = FileTimes::new();
    if flags.contains('a') || !flags.contains('m') {
        times = times.set_accessed(now);
    }
    if flags.contains('m') || !flags.contains('a') {
        times = times.set_modified(now);
    }
    file.set_times(times)
}

fn transfer(
    utility: Utility,
    name: &str,
    shell: &Shell,
    operands: &[OsString],
    recursive: bool,
    io: &mut ShellIo,
) -> io::Result<i32> {
    let (last, sources) = operands.split_last().expect("at least one operand");
    if sources.is_empty() {
        io.err(format_args!("{name}: missing destination\n"))?;
        return Ok(1);
    }
    let destination = match resolve(shell, last) {
        Ok(path) => path,
        Err(error) => {
            io.err(format_args!(
                "{name}: {}: {error}\n",
                last.to_string_lossy()
            ))?;
            return Ok(1);
        }
    };
    let into_dir = destination.is_dir();
    if !into_dir && (sources.len() > 1 || ends_in_separator(last)) {
        io.err(format_args!(
            "{name}: {}: destination must be an existing directory\n",
            destination.display()
        ))?;
        return Ok(1);
    }
    let mut status = 0;
    for operand in sources {
        let result = (|| {
            let source = resolve(shell, operand)?;
            if ends_in_separator(operand) {
                platform::require_directory(&source)?;
            }
            if matches!(utility, Utility::Mv) {
                protect_operand(operand, &source)?;
            }
            let target = if !into_dir || final_component(operand) == Some(b".") {
                destination.clone()
            } else {
                let basename = source.file_name().ok_or_else(|| {
                    io::Error::other("source needs a filename for this destination")
                })?;
                destination.join(basename)
            };
            if matches!(utility, Utility::Cp) {
                copy(&source, &target, recursive)
            } else {
                move_entry(&source, &target)
            }
        })();
        if let Err(error) = result {
            io.err(format_args!(
                "{name}: {} -> {}: {error}\n",
                operand.to_string_lossy(),
                destination.display()
            ))?;
            status = 1;
        }
    }
    Ok(status)
}

fn move_entry(source: &Path, target: &Path) -> io::Result<()> {
    let source_meta = fs::symlink_metadata(source)?;
    if source == target {
        return Err(io::Error::other(
            "source and destination are the same entry",
        ));
    }
    if source_meta.is_file()
        && fs::symlink_metadata(target).is_ok_and(|m| m.is_file())
        && same_file::is_same_file(source, target)?
    {
        // Allow case-only renames on a case-insensitive filesystem. Two hard
        // links have distinct canonical paths and must keep both names.
        if source.file_name() == target.file_name()
            || fs::canonicalize(source)? != fs::canonicalize(target)?
        {
            return Err(io::Error::other("source and destination are the same file"));
        }
    }
    fs::rename(source, target)
}

fn remove(path: &Path, recursive: bool) -> io::Result<()> {
    // Drop trailing separators so metadata describes the entry, even for a
    // directory symlink. Never pass a followed link to recursive removal.
    let trailing_separator = ends_in_separator(path.as_os_str());
    let path = path.components().as_path();
    let meta = fs::symlink_metadata(path)?;
    if trailing_separator && !meta.is_dir() && !path.is_dir() {
        return Err(io::ErrorKind::NotADirectory.into());
    }
    #[cfg(windows)]
    if directory_reparse_point(&meta) {
        return fs::remove_dir(path);
    }
    if meta.is_dir() {
        if !recursive {
            return Err(io::Error::other("is a directory (use -r)"));
        }
        if fs::canonicalize(path)?.parent().is_none() {
            return Err(io::Error::other("refusing to remove a filesystem root"));
        }
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(windows)]
fn directory_reparse_point(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    };
    let mask = FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT;
    meta.file_attributes() & mask == mask
}

fn copy(source: &Path, target: &Path, recursive: bool) -> io::Result<()> {
    let mut pending = vec![(source.to_owned(), target.to_owned())];
    while let Some((source, target)) = pending.pop() {
        copy_entry(&source, &target, recursive, &mut pending).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", source.display()))
        })?;
    }
    Ok(())
}

fn copy_entry(
    source: &Path,
    target: &Path,
    recursive: bool,
    pending: &mut Vec<(PathBuf, PathBuf)>,
) -> io::Result<()> {
    let meta = if recursive {
        fs::symlink_metadata(source)?
    } else {
        fs::metadata(source)?
    };
    if meta.is_symlink() {
        return copy_link(source, target, &meta);
    }
    #[cfg(windows)]
    if directory_reparse_point(&meta) {
        return Err(io::Error::other("unsupported directory reparse point"));
    }
    if meta.is_dir() {
        if !recursive {
            return Err(io::Error::other("is a directory (use -r)"));
        }
        let existing = match fs::symlink_metadata(target) {
            Ok(meta) if meta.is_dir() => true,
            Ok(_) => return Err(io::Error::other("destination entry is not a directory")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(e),
        };
        let parent = target.parent().unwrap_or(target);
        let physical = fs::canonicalize(if existing { target } else { parent })?;
        let original = fs::canonicalize(source)?;
        // Canonicalization resolves junctions and '..'. Check the directory
        // at the matching depth too, for alternate spellings on Windows.
        let depth = original.components().count();
        let peer = physical
            .ancestors()
            .find(|p| p.components().count() == depth);
        if physical.starts_with(&original)
            || peer
                .map(|p| same_file::is_same_file(p, &original))
                .transpose()?
                .unwrap_or(false)
        {
            return Err(io::Error::other("cannot copy a directory into itself"));
        }
        if !existing {
            fs::create_dir(target)?;
        }
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            pending.push((entry.path(), target.join(entry.file_name())));
        }
        return Ok(());
    }
    if !meta.is_file() {
        return Err(io::Error::other("source is not a regular file"));
    }
    copy_file(source, target, &meta)
}

fn copy_file(source: &Path, target: &Path, meta: &Metadata) -> io::Result<()> {
    let mut input = File::open(source)?;
    if fs::metadata(target).is_ok_and(|m| !m.is_file()) {
        return Err(io::Error::other("destination is not a regular file"));
    }
    let mut options = File::options();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(meta.permissions().mode() & 0o777);
    }
    #[cfg(not(unix))]
    let _ = meta;
    let mut output = match options.open(target) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Exclusive creation avoids creating the referent of a dangling
            // destination link. If another copy won the race, open its file.
            match options.create_new(true).open(target) {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    File::options().write(true).open(target)?
                }
                result => result?,
            }
        }
        result => result?,
    };
    // Compare the open files before truncating. This also catches hard links,
    // alternate case on Windows and paths through directory junctions.
    if Handle::from_file(input.try_clone()?)? == Handle::from_file(output.try_clone()?)? {
        return Err(io::Error::other("source and destination are the same file"));
    }
    output.set_len(0)?;
    io::copy(&mut input, &mut output)?;
    Ok(())
}

fn copy_link(source: &Path, target: &Path, _meta: &Metadata) -> io::Result<()> {
    let link = fs::read_link(source)?;
    // A repeated recursive copy may meet a link it already created.
    if fs::read_link(target).is_ok_and(|existing| existing == link) {
        if source.file_name() == target.file_name()
            && let (Some(a), Some(b)) = (source.parent(), target.parent())
            && same_file::is_same_file(a, b)?
        {
            return Err(io::Error::other("source and destination are the same link"));
        }
        return Ok(());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(link, target)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{symlink_dir, symlink_file};
        if directory_reparse_point(_meta) {
            symlink_dir(link, target)
        } else {
            symlink_file(link, target)
        }
    }
}
