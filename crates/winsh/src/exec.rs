use std::{
    env,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{Shell, ShellIo, platform};

pub(crate) fn run(shell: &Shell, arguments: &[OsString], io: &mut ShellIo) -> io::Result<i32> {
    let program = &arguments[0];
    let executable = match resolve(shell, program) {
        Ok(path) => path,
        Err(error) => {
            io.err(format_args!(
                "winsh: {}: {error}\n",
                program.to_string_lossy()
            ))?;
            return Ok(if error.kind() == io::ErrorKind::NotFound {
                127
            } else {
                126
            });
        }
    };
    // Rust can implicitly invoke cmd.exe for batch files. Defer that separate
    // argument language until we implement and test an explicit batch policy.
    if cfg!(windows)
        && executable.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
    {
        io.err(format_args!(
            "winsh: batch files are not supported yet; invoke their interpreter explicitly\n"
        ))?;
        return Ok(126);
    }
    let mut child = Command::new(executable);
    child
        .args(&arguments[1..])
        .current_dir(&shell.cwd)
        .env_clear()
        .envs(shell.environment.iter().map(|(key, value)| (key, value)));
    io.configure(&mut child)?;
    match platform::spawn(&mut child).and_then(|mut child| child.wait()) {
        Ok(status) => Ok(platform::exit_status(status)),
        Err(error) => {
            io.err(format_args!(
                "winsh: {}: {error}\n",
                program.to_string_lossy()
            ))?;
            // Resolution succeeded; failure to start is an execution failure.
            Ok(126)
        }
    }
}

fn resolve(shell: &Shell, program: &OsStr) -> io::Result<PathBuf> {
    if program.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "empty command name",
        ));
    }
    let path = Path::new(program);
    let explicit = path.is_absolute()
        || path.components().count() > 1
        || program.as_encoded_bytes().contains(&b'/')
        || (cfg!(windows) && program.as_encoded_bytes().contains(&b'\\'));
    if explicit {
        return candidate(&platform::absolute(&shell.cwd, path)?);
    }

    let mut failure = None;
    if let Some(paths) = shell.env("PATH") {
        for directory in env::split_paths(paths) {
            let path = platform::absolute(&shell.cwd, &directory)?.join(program);
            match candidate(&path) {
                Ok(path) => return Ok(path),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => failure = Some(error),
            }
        }
    }
    Err(failure.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "command not found")))
}

fn candidate(path: &Path) -> io::Result<PathBuf> {
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        // Explicit extensions (notably .cmd/.bat) must resolve to their own
        // file before considering an omitted .exe suffix.
        if path.extension().is_some() && path.is_file() {
            return Ok(path.to_owned());
        }
        // Append rather than replace: dotted directories and command names
        // retain their spelling. PATHEXT interpreter dispatch is deferred.
        let mut executable = path.as_os_str().to_os_string();
        executable.push(".exe");
        match executable_file(Path::new(&executable)) {
            Ok(()) => return Ok(executable.into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    executable_file(path)?;
    Ok(path.to_owned())
}

fn executable_file(path: &Path) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "not an executable file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "permission denied",
            ));
        }
    }
    Ok(())
}
