#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    use std::{env, ffi::OsString, io, path::PathBuf};
    use winsh_terminal::window;

    fn run() -> io::Result<i32> {
        let mut program = env::current_exe()?.with_file_name("winsh.exe");
        let mut arguments: Vec<OsString> = Vec::new();
        let mut options = env::args_os().skip(1);
        while let Some(option) = options.next() {
            match option.to_str() {
                Some("--help" | "-h") => {
                    window::message(
                        "winsh terminal\n\nUsage: winsh-terminal [--shell <executable>] [-- <shell arguments>]\n\nBy default, launches winsh.exe beside this executable.\nBuild both executables with: cargo build --workspace\n\nCtrl+Shift+V / Shift+Insert: paste\nCtrl+Shift+C: copy visible screen\nMouse wheel / Shift+PageUp / Shift+PageDown: scroll\nCtrl+C: interrupt input or the foreground program",
                        false,
                    );
                    return Ok(0);
                }
                Some("--shell") => {
                    program = PathBuf::from(options.next().ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "--shell requires an executable path",
                        )
                    })?)
                }
                Some("--") => {
                    arguments.extend(options);
                    break;
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "unsupported argument; use --help",
                    ));
                }
            }
        }
        if !program.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Shell executable not found: {}\n\nBuild both executables with cargo build --workspace, or use --shell <path>.",
                    program.display()
                ),
            ));
        }
        window::run(&program, &arguments)
    }

    let status = match run() {
        Ok(status) => status,
        Err(error) => {
            window::message(&error.to_string(), true);
            1
        }
    };
    std::process::exit(status);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("winsh-terminal requires Windows 10 version 1809 or later (ConPTY).");
    std::process::exit(1);
}
