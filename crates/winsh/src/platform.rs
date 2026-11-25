use std::{
    ffi::{OsStr, OsString},
    fs, io,
    path::{Component, Path, PathBuf},
    process::{Child, Command, ExitStatus},
};

#[cfg(windows)]
pub(crate) mod addresses;

pub(crate) fn env_name_eq(key: &OsStr, name: &str) -> bool {
    if cfg!(windows) {
        key.to_str()
            .is_some_and(|key| key.eq_ignore_ascii_case(name))
    } else {
        key == name
    }
}

pub(crate) fn env_value<'a>(
    environment: &'a [(OsString, OsString)],
    name: &str,
) -> Option<&'a OsStr> {
    environment
        .iter()
        .rev()
        .find(|(key, _)| env_name_eq(key, name))
        .map(|(_, value)| value.as_os_str())
}

pub(crate) fn require_directory(path: &Path) -> io::Result<()> {
    if fs::metadata(path)?.is_dir() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "not a directory",
        ))
    }
}

pub(crate) fn absolute(cwd: &Path, path: &Path) -> io::Result<PathBuf> {
    let joined = cwd.join(path);
    #[cfg(windows)]
    let joined = {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        // PathBuf::join normalizes dot-dot when the base has a verbatim prefix
        // (as canonicalize returns). Preserve those steps for directory() to
        // validate and resolve in logical or physical order.
        match (cwd.components().next(), path.components().next()) {
            (Some(Component::Prefix(prefix)), next)
                if prefix.kind().is_verbatim() && !matches!(next, Some(Component::Prefix(_))) =>
            {
                let mut units: Vec<u16> = if path.has_root() {
                    prefix.as_os_str().encode_wide().collect()
                } else {
                    cwd.as_os_str()
                        .encode_wide()
                        .chain([b'\\' as u16])
                        .collect()
                };
                units.extend(path.as_os_str().encode_wide().map(|unit| {
                    if unit == b'/' as u16 {
                        b'\\' as u16
                    } else {
                        unit
                    }
                }));
                PathBuf::from(OsString::from_wide(&units))
            }
            _ => joined,
        }
    };
    if !joined.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "drive-relative paths are not supported; use an absolute path",
        ));
    }
    Ok(joined)
}

/// Resolve dot-dot in the requested order, before Windows can normalize it.
/// Logical navigation keeps links in the spelling; physical navigation resolves
/// each prefix before stepping to its parent.
pub(crate) fn directory(path: &Path, physical: bool) -> io::Result<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                require_directory(&result)?;
                if physical {
                    result = fs::canonicalize(result)?;
                }
                result.pop();
            }
            _ => result.push(component.as_os_str()),
        }
    }
    require_directory(&result)?;
    if physical {
        fs::canonicalize(result)
    } else {
        Ok(result)
    }
}

pub(crate) fn exit_status(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
    }
    #[cfg(windows)]
    {
        match status.code() {
            // STATUS_CONTROL_C_EXIT
            Some(-1073741510) => 130,
            Some(code) => code,
            None => 1,
        }
    }
    #[cfg(not(any(unix, windows)))]
    status.code().unwrap_or(1)
}

#[cfg(not(windows))]
pub(crate) fn spawn(command: &mut Command) -> io::Result<Child> {
    command.spawn()
}

#[cfg(windows)]
pub(crate) use windows::spawn;

#[cfg(windows)]
pub(crate) fn enable_virtual_terminal_output(file: &fs::File) -> bool {
    use winapi_util::console::{mode, set_mode};
    use windows_sys::Win32::System::Console::ENABLE_VIRTUAL_TERMINAL_PROCESSING as VT;
    mode(file).is_ok_and(|flags| flags & VT != 0 || set_mode(file, flags | VT).is_ok())
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    use super::*;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        GetThreadErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetThreadErrorMode,
    };

    pub(crate) fn spawn(command: &mut Command) -> io::Result<Child> {
        struct Restore(u32);
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: this guard stays on the creating thread and restores
                // the mode returned by SetThreadErrorMode on that same thread.
                unsafe { SetThreadErrorMode(self.0, std::ptr::null_mut()) };
            }
        }
        let mut previous = 0;
        // SAFETY: valid output pointer; flags affect only this calling thread.
        // Invalid executables must return errors, not block behind Windows'
        // "Unsupported 16-Bit Application" or critical-error dialog.
        let changed = unsafe {
            SetThreadErrorMode(
                GetThreadErrorMode() | SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX,
                &mut previous,
            )
        };
        if changed == 0 {
            return Err(io::Error::last_os_error());
        }
        let _restore = Restore(previous);
        command.spawn()
    }
}
