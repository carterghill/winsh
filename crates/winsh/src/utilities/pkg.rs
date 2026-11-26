use std::{ffi::OsString, io};

use crate::{Shell, ShellIo, exec};

const USAGE: &str = "Usage: pkg install <package-name-or-ID>";

pub(super) fn run(shell: &Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<i32> {
    if matches!(args, [help] if help == "--help" || help == "-h") {
        io.out(format_args!("{USAGE}\n"))?;
        return Ok(0);
    }
    let package = match args {
        [verb, query]
            if verb == "install"
                && !query.is_empty()
                && !query.as_encoded_bytes().starts_with(b"-") =>
        {
            query
        }
        _ => {
            io.err(format_args!("{USAGE}\n"))?;
            return Ok(2);
        }
    };
    if package != "python3" {
        return install(shell, ["--query".into(), package.clone()], io);
    }
    // python3.exe may already exist as a Store shortcut on a clean machine.
    // pymanager.exe identifies the install manager, even with a legacy py.exe.
    if exec::resolve(shell, "pymanager.exe".as_ref())
        .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
    {
        let options = ["--exact", "--id", "9NQ7512CXL7T", "--source", "msstore"];
        let status = install(shell, options.map(OsString::from), io)?;
        if status != 0 {
            return Ok(status);
        }
    }
    let args = ["pymanager.exe", "install", "3"].map(OsString::from);
    let process = exec::spawn(shell, &args, io)?;
    if let exec::Spawned::Failed(127) = process {
        io.err(format_args!(
            "pkg: enable the Python install manager in Manage app execution aliases and check that %LOCALAPPDATA%/Microsoft/WindowsApps is on PATH\n"
        ))?;
    }
    exec::wait(shell, process)
}

fn install(
    shell: &Shell,
    options: impl IntoIterator<Item = OsString>,
    io: &mut ShellIo,
) -> io::Result<i32> {
    let mut args = vec!["winget.exe".into(), "install".into()];
    args.extend(options);
    let process = exec::spawn(shell, &args, io)?;
    if let exec::Spawned::Failed(127) = process {
        io.err(format_args!(
            "pkg: install or update App Installer for WinGet, then check that winget.exe is on PATH\n"
        ))?;
    }
    exec::wait(shell, process)
}
