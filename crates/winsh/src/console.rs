//! Terminal input and the CLI's Windows Ctrl+C handler. The library never
//! installs process-wide handlers.
use std::{
    fs::File,
    io::{self, Read},
};

/// Read redirected source without read-ahead. A child inheriting stdin must be
/// able to consume bytes following its command line.
pub fn redirected_stdin() -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        Ok(io::stdin().as_handle().try_clone_to_owned()?.into())
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        Ok(io::stdin().as_fd().try_clone_to_owned()?.into())
    }
}

pub fn read_line_unbuffered(input: &mut File, line: &mut String) -> io::Result<usize> {
    let mut bytes = Vec::new();
    let mut byte = [0];
    loop {
        match input.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                bytes.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    let count = bytes.len();
    *line = String::from_utf8(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(count)
}

#[cfg(windows)]
pub use windows::{Handler, read_line};

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    use std::io;
    use windows_sys::Win32::{
        Foundation::{ERROR_OPERATION_ABORTED, GetLastError, SetLastError},
        System::Console::{
            CONSOLE_READCONSOLE_CONTROL, CTRL_BREAK_EVENT, CTRL_C_EVENT, GetStdHandle,
            ReadConsoleW, STD_INPUT_HANDLE, SetConsoleCtrlHandler,
        },
    };

    // Protect the shell process. ReadConsoleW reports interrupted input itself,
    // and native child exit status reports interruption during execution.
    unsafe extern "system" fn handler(event: u32) -> i32 {
        match event {
            CTRL_C_EVENT => winsh::interrupt_foreground(),
            CTRL_BREAK_EVENT => winsh::stop_foreground(),
            _ => return 0,
        }
        1
    }

    pub struct Handler;

    impl Handler {
        pub fn install() -> io::Result<Self> {
            // SAFETY: clear an inherited ignore-Ctrl+C attribute. Registering a
            // callback alone does not clear it, and children would inherit it.
            if unsafe { SetConsoleCtrlHandler(None, 0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the callback has the Windows ABI, static lifetime, and
            // no state. A non-null handler doesn't make children
            // inherit the special "ignore Ctrl+C" setting.
            if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(Self)
            }
        }
    }

    impl Drop for Handler {
        fn drop(&mut self) {
            // SAFETY: unregister the same static function installed above.
            unsafe { SetConsoleCtrlHandler(Some(handler), 0) };
        }
    }

    /// Unlike std's Windows Stdin reader, return Ctrl+C to the input loop instead
    /// of silently retrying ReadConsoleW. Accumulate UTF-16 before converting so
    /// even a surrogate pair split across reads is preserved.
    pub fn read_line(line: &mut String) -> io::Result<usize> {
        let control = CONSOLE_READCONSOLE_CONTROL {
            nLength: size_of::<CONSOLE_READCONSOLE_CONTROL>() as u32,
            nInitialChars: 0,
            dwCtrlWakeupMask: 1 << 0x1a, // Ctrl+Z: Windows console EOF.
            dwControlKeyState: 0,
        };
        let mut units = Vec::new();
        loop {
            let mut buffer = [0u16; 256];
            let mut count = 0;
            // SAFETY: valid initialized UTF-16 buffer, bounded length, and valid
            // output/control pointers. The standard input handle is borrowed.
            let (success, error) = unsafe {
                SetLastError(0);
                let success = ReadConsoleW(
                    GetStdHandle(STD_INPUT_HANDLE),
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut count,
                    &control,
                );
                (success, GetLastError())
            };
            if error == ERROR_OPERATION_ABORTED && (success == 0 || count == 0) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if success == 0 {
                return Err(io::Error::from_raw_os_error(error as i32));
            }
            units.extend_from_slice(&buffer[..count as usize]);
            if units.last() == Some(&0x1a) {
                units.pop();
                break;
            }
            if count == 0 || units.last() == Some(&u16::from(b'\n')) {
                break;
            }
        }
        *line = String::from_utf16(&units)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(line.len())
    }
}

#[cfg(not(windows))]
pub struct Handler;

#[cfg(not(windows))]
impl Handler {
    pub fn install() -> io::Result<Self> {
        Ok(Self)
    }
}

#[cfg(not(windows))]
pub fn read_line(line: &mut String) -> io::Result<usize> {
    io::stdin().read_line(line)
}
