//! The worker waits on its bootstrap pipe until it belongs to the job object.
//! All descendants then inherit containment, including native pipeline stages.
#![allow(unsafe_code)]
use super::State;
use std::{
    fs::File,
    io::{self, Write},
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Child, ChildStdin, Command},
};
use windows_sys::Win32::{
    Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_ALREADY_EXISTS, GetLastError, WAIT_OBJECT_0,
    },
    System::{
        Console::{
            GetConsoleMode, GetStdHandle, INPUT_RECORD, KEY_EVENT, PeekConsoleInputW,
            ReadConsoleInputW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
        },
        JobObjects::*,
        Threading::{
            CREATE_NEW_PROCESS_GROUP, CreateEventW, GetCurrentProcess, WaitForSingleObject,
        },
    },
};

pub(super) fn configure(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

pub(super) struct Group(OwnedHandle);
impl Group {
    pub(super) fn attach(child: &Child) -> io::Result<Self> {
        // SAFETY: an unnamed job has default security; take ownership on success.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: handles and the correctly sized limits structure remain live.
        if unsafe {
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            )
        } == 0
            || unsafe { AssignProcessToJobObject(raw, child.as_raw_handle()) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }
    pub(super) fn freeze(&self, stopped: bool) -> io::Result<()> {
        // Windows 10+ JobObjectFreezeInformation (class 18). The SDK does not
        // expose this native class. Keep the ABI here and propagate OS errors.
        #[repr(C)]
        struct FreezeInfo {
            flags: u32,
            freeze: u8,
            swap: u8,
            reserved: [u8; 2],
            wake_filter: [u32; 2],
        }
        let info = FreezeInfo {
            flags: 1,
            freeze: u8::from(stopped),
            swap: 0,
            reserved: [0; 2],
            wake_filter: [0; 2],
        };
        // SAFETY: freeze the entire owned job in one kernel operation.
        if unsafe {
            SetInformationJobObject(
                self.0.as_raw_handle(),
                18,
                (&info as *const FreezeInfo).cast(),
                size_of_val(&info) as u32,
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    pub(super) fn terminate(&self, status: i32) -> io::Result<()> {
        // SAFETY: the handle belongs exclusively to this shell job.
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), status as u32) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    pub(super) fn poll(&self, child: &mut Child) -> io::Result<Option<State>> {
        Ok(child
            .try_wait()?
            .map(|s| State::Done(crate::platform::exit_status(s))))
    }
}

#[derive(Default, Clone, Copy)]
pub(super) struct Modes([Option<u32>; 2]);
impl Modes {
    pub(super) fn current() -> Self {
        Self([STD_INPUT_HANDLE, STD_OUTPUT_HANDLE].map(|fd| {
            let mut mode = 0;
            // SAFETY: borrowed standard handle and a writable mode word.
            (unsafe { GetConsoleMode(GetStdHandle(fd), &mut mode) } != 0).then_some(mode)
        }))
    }
    pub(super) fn restore(self) {
        for (fd, mode) in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE]
            .into_iter()
            .zip(self.0)
        {
            if let Some(mode) = mode {
                // SAFETY: mode was read from this standard console handle.
                unsafe { SetConsoleMode(GetStdHandle(fd), mode) };
            }
        }
    }
}
#[derive(Default)]
pub(super) struct Foreground {
    event: Option<OwnedHandle>,
    stop: bool,
    modes: Modes,
}
impl Drop for Foreground {
    fn drop(&mut self) {
        self.modes.restore();
    }
}
pub(super) struct InputRelay {
    sender: std::sync::mpsc::SyncSender<Vec<u8>>,
    pending: Option<Vec<u8>>,
    line: Vec<u16>,
}
pub(super) fn input_relay(mut pipe: ChildStdin) -> io::Result<InputRelay> {
    let (sender, receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(1);
    // A job may not be reading. Keep writes off the foreground control loop
    // so even a full pipe can always be interrupted or stopped.
    std::thread::Builder::new()
        .name("job stdin".into())
        .spawn(move || {
            for line in receiver {
                if pipe.write_all(&line).is_err() {
                    break;
                }
            }
        })?;
    Ok(InputRelay {
        sender,
        pending: None,
        line: Vec::new(),
    })
}
impl Foreground {
    pub(super) fn enter(_: u32) -> io::Result<Self> {
        use std::os::windows::ffi::OsStrExt;
        let mut guard = Self {
            event: None,
            stop: false,
            modes: Modes::current(),
        };
        let Some(name) = std::env::var_os("WINSH_TERMINAL_STOP_EVENT") else {
            return Ok(guard);
        };
        let wide: Vec<_> = name.encode_wide().chain([0]).collect();
        // SAFETY: named auto-reset event, opened only while a job owns the terminal.
        let raw = unsafe { CreateEventW(std::ptr::null(), 0, 0, wide.as_ptr()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let existing = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let event = unsafe { OwnedHandle::from_raw_handle(raw) };
        guard.event = (!existing).then_some(event);
        Ok(guard)
    }
    pub(super) fn stop_requested(&self) -> bool {
        // SAFETY: nonblocking query of this guard's live event.
        self.stop
            || self.event.as_ref().is_some_and(
                |h| unsafe { WaitForSingleObject(h.as_raw_handle(), 0) } == WAIT_OBJECT_0,
            )
    }
    // Background-started readers have a private input pipe. Only its current
    // foreground owner consumes key events; the shell prompt never races it.
    pub(super) fn forward_input(&mut self, input: &mut InputRelay) -> io::Result<bool> {
        let mut record = INPUT_RECORD::default();
        let mut count = 0;
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        for _ in 0..64 {
            if let Some(line) = input.pending.take() {
                use std::sync::mpsc::TrySendError;
                match input.sender.try_send(line) {
                    Ok(()) => {}
                    Err(TrySendError::Disconnected(_)) => return Ok(false),
                    Err(TrySendError::Full(line)) => {
                        input.pending = Some(line);
                        break;
                    }
                }
            }
            // SAFETY: one writable record, valid borrowed console, sole input reader.
            if unsafe { PeekConsoleInputW(handle, &mut record, 1, &mut count) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if count == 0 {
                break;
            }
            if unsafe { ReadConsoleInputW(handle, &mut record, 1, &mut count) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if count == 0 || record.EventType as u32 != KEY_EVENT {
                continue;
            }
            let key = unsafe { record.Event.KeyEvent };
            if key.bKeyDown == 0 {
                continue;
            }
            let ch = unsafe { key.uChar.UnicodeChar };
            match ch {
                0 => {}
                4 if input.line.is_empty() => return Ok(false),
                26 => {
                    self.stop = true;
                    break;
                }
                8 => {
                    if let Some(c) = input.line.pop() {
                        if (0xdc00..=0xdfff).contains(&c) {
                            input.line.pop();
                        }
                        io::stderr().write_all(b"\x08 \x08")?;
                    }
                }
                13 => {
                    let mut line = String::from_utf16_lossy(&input.line);
                    line.push('\n');
                    input.line.clear();
                    writeln!(io::stderr())?;
                    input.pending = Some(line.into_bytes());
                }
                c if c >= 32 || c == 9 => {
                    input.line.push(c);
                    if !(0xd800..=0xdbff).contains(&c) {
                        let n = if (0xdc00..=0xdfff).contains(&c) && input.line.len() >= 2 {
                            2
                        } else {
                            1
                        };
                        write!(
                            io::stderr(),
                            "{}",
                            String::from_utf16_lossy(&input.line[input.line.len() - n..])
                        )?;
                    }
                }
                _ => {}
            }
        }
        Ok(true)
    }
}

pub(super) fn terminal_input() -> io::Result<File> {
    File::options().read(true).write(true).open("CONIN$")
}
pub(super) fn prepare_input(_: &mut Command, _: &File) {}
pub(super) fn transfer_input(child: &Child, file: &File) -> io::Result<u64> {
    let mut target = std::ptr::null_mut();
    // SAFETY: duplicate only the caller-supplied input into our own waiting worker.
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            file.as_raw_handle(),
            child.as_raw_handle(),
            &mut target,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(target as usize as u64)
}
pub(super) fn received_input(value: u64) -> io::Result<File> {
    let mut target = std::ptr::null_mut();
    // Ask Windows to validate and duplicate the transferred handle. Only take
    // ownership of the returned duplicate; the one-shot worker owns the original.
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            value as usize as _,
            GetCurrentProcess(),
            &mut target,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_handle(target) })
}
