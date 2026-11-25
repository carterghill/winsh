use std::{
    env,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    process::{Child, Command},
};

use crate::{Shell, ShellIo, platform};

pub(crate) enum Spawned {
    Running(ChildProcess),
    Complete(i32),
}

/// A failed host operation must never abandon an already-started native child.
pub(crate) struct ChildProcess {
    child: Child,
    waited: bool,
}

impl ChildProcess {
    pub(crate) fn wait(&mut self) -> io::Result<i32> {
        let status = self.child.wait()?;
        self.waited = true;
        Ok(platform::exit_status(status))
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if !self.waited {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub(crate) fn run(shell: &Shell, arguments: &[OsString], io: &mut ShellIo) -> io::Result<i32> {
    match spawn(shell, arguments, io)? {
        Spawned::Running(mut child) => {
            if let Some(cancel) = &shell.cancellation {
                loop {
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        return Err(io::Error::other(
                            "pipeline cancelled after host I/O failure",
                        ));
                    }
                    if let Some(status) = child.child.try_wait()? {
                        child.waited = true;
                        return Ok(platform::exit_status(status));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            } else {
                child.wait()
            }
        }
        Spawned::Complete(status) => Ok(status),
    }
}

pub(crate) fn spawn(
    shell: &Shell,
    arguments: &[OsString],
    io: &mut ShellIo,
) -> io::Result<Spawned> {
    let program = &arguments[0];
    let executable = match resolve(shell, program) {
        Ok(path) => path,
        Err(error) => {
            io.err(format_args!(
                "winsh: {}: {error}\n",
                program.to_string_lossy()
            ))?;
            return Ok(Spawned::Complete(
                if error.kind() == io::ErrorKind::NotFound {
                    127
                } else {
                    126
                },
            ));
        }
    };
    // .sh is winsh's explicit script dispatch policy. Never ask Windows file
    // associations or another shell to reinterpret its source or arguments.
    if executable
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sh"))
    {
        if shell.source_depth >= 32 {
            io.err(format_args!("winsh: script nesting limit (32)\n"))?;
            return Ok(Spawned::Complete(2));
        }
        let source = match fs::read_to_string(&executable) {
            Ok(source) => source,
            Err(error) => {
                io.err(format_args!(
                    "winsh: {}: {error}\n",
                    program.to_string_lossy()
                ))?;
                return Ok(Spawned::Complete(126));
            }
        };
        let mut child = Shell::with_environment(shell.cwd.clone(), shell.environment.clone())?;
        child.source_depth = shell.source_depth + 1;
        child.cancellation = shell.cancellation.clone();
        child
            .utility_executable
            .clone_from(&shell.utility_executable);
        child.set_arguments(program.clone(), arguments[1..].to_vec());
        let source = source.trim_start_matches('\u{feff}').replace("\r\n", "\n");
        return Ok(Spawned::Complete(child.run(&source, io)?.status()));
    }
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
        return Ok(Spawned::Complete(126));
    }
    let mut child = Command::new(executable);
    child
        .args(&arguments[1..])
        .current_dir(&shell.cwd)
        .env_clear()
        .envs(shell.environment.iter().map(|(key, value)| (key, value)));
    io.configure(&mut child)?;
    match platform::spawn(&mut child) {
        Ok(child) => Ok(Spawned::Running(ChildProcess {
            child,
            waited: false,
        })),
        Err(error) => {
            io.err(format_args!(
                "winsh: {}: {error}\n",
                program.to_string_lossy()
            ))?;
            // Resolution succeeded; failure to start is an execution failure.
            Ok(Spawned::Complete(126))
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

pub(crate) fn executable_file(path: &Path) -> io::Result<()> {
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
