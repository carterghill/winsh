//! Safe owning wrappers around ConPTY. The reader, writer, and control worker
//! are separate so a full pipe cannot block window events or shutdown.
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{HANDLE, WAIT_OBJECT_0},
    System::{
        Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole},
        IO::CancelSynchronousIo,
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Pipes::CreatePipe,
        Threading::{
            CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            DeleteProcThreadAttributeList, EVENT_MODIFY_STATE, EXTENDED_STARTUPINFO_PRESENT,
            GetExitCodeProcess, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
            OpenEventW, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread,
            STARTF_USESTDHANDLES, STARTUPINFOEXW, SetEvent, TerminateProcess,
            UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

use crate::terminal::Terminal;

enum Control {
    Resize(u16, u16),
    Shutdown,
}

pub enum Event {
    Exited(i32),
    Resized(u16, u16),
    Error(String),
}

pub struct Session {
    pub(crate) initial_directory: PathBuf,
    stop_event: Vec<u16>,
    input: mpsc::Sender<Vec<u8>>,
    control: mpsc::Sender<Control>,
    events: mpsc::Receiver<Event>,
    worker: Option<JoinHandle<()>>,
}

impl Session {
    pub fn spawn(
        program: &Path,
        arguments: &[OsString],
        rows: u16,
        columns: u16,
        terminal: Arc<Mutex<Terminal>>,
    ) -> io::Result<Self> {
        Self::spawn_in(
            program,
            arguments,
            &std::env::current_dir()?,
            rows,
            columns,
            terminal,
        )
    }

    pub(crate) fn spawn_in(
        program: &Path,
        arguments: &[OsString],
        directory: &Path,
        rows: u16,
        columns: u16,
        terminal: Arc<Mutex<Terminal>>,
    ) -> io::Result<Self> {
        static NEXT_SESSION: AtomicUsize = AtomicUsize::new(1);
        let name = format!(
            "Local\\winsh-stop-{}-{}",
            std::process::id(),
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        );
        let stop_event = wide(OsStr::new(&name))?;
        let mut resources = Resources::create(program, arguments, rows, columns, &name, directory)?;
        let mut output = resources.console.output.take().expect("new output pipe");
        let (input, writes) = mpsc::channel::<Vec<u8>>();
        let (control, controls) = mpsc::channel();
        let (events_send, events) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));

        let writer_stop = Arc::clone(&stopping);
        let writer_events = events_send.clone();
        let mut pipe = resources.input;
        let writer = thread::spawn(move || {
            while !writer_stop.load(Ordering::Acquire) {
                match writes.recv_timeout(Duration::from_millis(25)) {
                    Ok(bytes) => {
                        if let Err(error) = pipe.write_all(&bytes) {
                            if !writer_stop.load(Ordering::Acquire) {
                                let _ = writer_events
                                    .send(Event::Error(format!("terminal input: {error}")));
                            }
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        let replies = input.clone();
        let reader_terminal = Arc::clone(&terminal);
        let reader_events = events_send.clone();
        let reader = thread::spawn(move || {
            let mut buffer = [0; 8192];
            loop {
                match output.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let response = reader_terminal.lock().unwrap().process(&buffer[..count]);
                        if !response.is_empty() && replies.send(response).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => break,
                    Err(error) => {
                        let _ =
                            reader_events.send(Event::Error(format!("terminal output: {error}")));
                        break;
                    }
                }
            }
        });

        let worker = thread::spawn(move || {
            let mut status = 0;
            loop {
                // SAFETY: the process handle is owned and stays live in this worker.
                if unsafe { WaitForSingleObject(resources.process.as_raw_handle(), 0) }
                    == WAIT_OBJECT_0
                {
                    let mut code = 0;
                    // SAFETY: valid process handle and output pointer.
                    if unsafe { GetExitCodeProcess(resources.process.as_raw_handle(), &mut code) }
                        != 0
                    {
                        status = code as i32;
                    }
                    break;
                }
                match controls.recv_timeout(Duration::from_millis(25)) {
                    Ok(Control::Resize(rows, columns)) => {
                        terminal.lock().unwrap().resize(rows, columns);
                        // SAFETY: this worker exclusively owns the pseudoconsole.
                        let result = unsafe {
                            ResizePseudoConsole(resources.console.handle, size(rows, columns))
                        };
                        if result < 0 {
                            let _ = events_send
                                .send(Event::Error(format!("resize ConPTY: HRESULT {result:#x}")));
                            break;
                        }
                        let _ = events_send.send(Event::Resized(rows, columns));
                    }
                    Ok(Control::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
            stopping.store(true, Ordering::Release);
            // SAFETY: this job contains only this session's shell and descendants.
            // Kill the entire tree before closing ConPTY; a child ignoring close
            // events must not keep the window or its pipes alive indefinitely.
            unsafe { TerminateJobObject(resources.job.as_raw_handle(), 1) };
            drop(resources.console); // The output reader continues draining here.
            // SAFETY: JoinHandle owns a live thread handle until join below. This
            // cancels a writer blocked in synchronous pipe I/O during shutdown.
            unsafe { CancelSynchronousIo(writer.as_raw_handle()) };
            let _ = writer.join();
            let _ = reader.join();
            let _ = events_send.send(Event::Exited(status));
        });
        Ok(Self {
            initial_directory: directory.to_owned(),
            stop_event,
            input,
            control,
            events,
            worker: Some(worker),
        })
    }

    pub(crate) fn initial_title(&self) -> String {
        directory_title(&self.initial_directory)
            .to_string_lossy()
            .into_owned()
    }

    pub fn write(&self, bytes: Vec<u8>) -> io::Result<()> {
        // Ctrl+Z is a console input character on Windows. A foreground winsh
        // job advertises a stop event; all other hosted programs get normal input.
        if bytes == [26] && self.stop_foreground()? {
            return Ok(());
        }
        self.input
            .send(bytes)
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }

    pub(crate) fn stop_foreground(&self) -> io::Result<bool> {
        // SAFETY: NUL-terminated name; take ownership only on successful open.
        let raw = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, self.stop_event.as_ptr()) };
        if raw.is_null() {
            return Ok(false);
        }
        let event = unsafe { OwnedHandle::from_raw_handle(raw) };
        if unsafe { SetEvent(event.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(true)
    }

    pub fn resize(&self, rows: u16, columns: u16) {
        let _ = self.control.send(Control::Resize(rows, columns));
    }

    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct PseudoConsole {
    handle: HPCON,
    // Retained only during setup. On failure, close the reader before ConPTY
    // so ClosePseudoConsole cannot wait for an undrained output pipe.
    output: Option<File>,
}

impl Drop for PseudoConsole {
    fn drop(&mut self) {
        drop(self.output.take());
        // SAFETY: created successfully, uniquely owned, closed exactly once.
        unsafe { ClosePseudoConsole(self.handle) };
    }
}

struct Resources {
    console: PseudoConsole,
    process: OwnedHandle,
    job: OwnedHandle,
    input: File,
}

impl Resources {
    fn create(
        program: &Path,
        arguments: &[OsString],
        rows: u16,
        columns: u16,
        stop_event: &str,
        directory: &Path,
    ) -> io::Result<Self> {
        let program = program.canonicalize()?;
        let application = wide(program.as_os_str())?;
        let mut command = command_line(program.as_os_str(), arguments)?;
        let environment = terminal_environment(stop_event);
        let (input_read, input_write) = pipe()?;
        let (output_read, output_write) = pipe()?;
        let mut handle = 0;
        // SAFETY: valid owned synchronous pipe ends and initialized output pointer.
        let result = unsafe {
            CreatePseudoConsole(
                size(rows, columns),
                input_read.as_raw_handle(),
                output_write.as_raw_handle(),
                0,
                &mut handle,
            )
        };
        if result < 0 {
            return Err(io::Error::other(format!(
                "create ConPTY: HRESULT {result:#x}"
            )));
        }
        let console = PseudoConsole {
            handle,
            output: Some(output_read.into()),
        };
        let attributes = Attributes::new(handle)?;
        let job = job()?;
        let mut startup = STARTUPINFOEXW::default();
        let mut title = wide(directory_title(directory))?;
        let directory = wide(directory.as_os_str())?;
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.lpTitle = title.as_mut_ptr();
        // Explicit null standard handles let ConPTY populate console handles.
        // Otherwise Windows can copy a launching terminal's redirected pipes
        // into the child, bypassing its pseudoconsole entirely.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.lpAttributeList = attributes.pointer();
        let mut information = PROCESS_INFORMATION::default();
        // SAFETY: strings are NUL terminated, command line is writable, all
        // structures and the extended attribute list remain valid for the call.
        // No ordinary handles are inherited; ConPTY provides the child's console.
        let success = unsafe {
            CreateProcessW(
                application.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                environment.as_ptr().cast(),
                directory.as_ptr(),
                &startup.StartupInfo,
                &mut information,
            )
        };
        if success == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful CreateProcessW transferred two uniquely owned handles.
        let process = unsafe { OwnedHandle::from_raw_handle(information.hProcess) };
        // SAFETY: the thread handle is distinct from the process handle.
        let thread = unsafe { OwnedHandle::from_raw_handle(information.hThread) };
        // SAFETY: owned handles; the suspended process cannot spawn descendants
        // before it has been assigned to the kill-on-close job.
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) } == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: setup failed; terminate only the suspended process we created.
            unsafe { TerminateProcess(process.as_raw_handle(), 1) };
            return Err(error);
        }
        // SAFETY: the owned primary thread was created suspended above.
        if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        // These copies must close after process creation so EOF is observable.
        drop(input_read);
        drop(output_write);
        Ok(Self {
            console,
            process,
            job,
            input: input_write.into(),
        })
    }
}

fn directory_title(directory: &Path) -> &OsStr {
    directory.file_name().unwrap_or(directory.as_os_str())
}

fn size(rows: u16, columns: u16) -> COORD {
    COORD {
        X: columns.clamp(1, i16::MAX as u16) as i16,
        Y: rows.clamp(1, i16::MAX as u16) as i16,
    }
}

fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read: HANDLE = ptr::null_mut();
    let mut write: HANDLE = ptr::null_mut();
    // SAFETY: valid output pointers; null security attributes disable inheritance.
    if unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreatePipe returned two valid, distinct handles owned by this call.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    })
}

fn job() -> io::Result<OwnedHandle> {
    // SAFETY: unnamed job with default security, no pointers retained by Windows.
    let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successfully created, uniquely owned job handle.
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: structure size and pointer match the requested information class.
    if unsafe {
        SetInformationJobObject(
            raw,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

struct Attributes {
    storage: Vec<usize>,
}

impl Attributes {
    fn new(console: HPCON) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: the first call only obtains the allocation size.
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut bytes) };
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let pointer = storage.as_mut_ptr().cast();
        // SAFETY: sufficient pointer-aligned storage, live until Attributes drops.
        if unsafe { InitializeProcThreadAttributeList(pointer, 1, 0, &mut bytes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let list = Self { storage };
        // SAFETY: ConPTY requires the HPCON value itself, not a pointer to it.
        if unsafe {
            UpdateProcThreadAttribute(
                list.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                console as *const _,
                size_of::<HPCON>(),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(list)
    }

    fn pointer(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_ptr().cast_mut().cast()
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: initialized attribute list, freed before its backing storage.
        unsafe { DeleteProcThreadAttributeList(self.pointer()) };
    }
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut result: Vec<_> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "command contains a NUL character",
        ));
    }
    result.push(0);
    Ok(result)
}

// The child runs in our VT terminal even if the launcher inherited TERM=dumb.
// CreateProcessW expects sorted, NUL-terminated name=value pairs and a final NUL.
fn terminal_environment(stop_event: &str) -> Vec<u16> {
    let mut vars: Vec<_> = std::env::vars_os()
        .filter(|(name, _)| {
            !name.eq_ignore_ascii_case("TERM")
                && !name.eq_ignore_ascii_case("WINSH_TERMINAL_STOP_EVENT")
        })
        .collect();
    vars.push(("TERM".into(), "xterm-256color".into()));
    vars.push(("WINSH_TERMINAL_STOP_EVENT".into(), stop_event.into()));
    // Expose the composer's insertion point for click positioning in Claude
    // Code's inline UI. Honor an explicit preference inherited from the user.
    if !vars
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_CODE_NATIVE_CURSOR"))
    {
        vars.push(("CLAUDE_CODE_NATIVE_CURSOR".into(), "1".into()));
    }
    vars.sort_by_cached_key(|(name, _)| name.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (name, value) in vars {
        block.extend(name.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

// Windows CRT quoting: double backslashes before a quote or closing delimiter.
fn command_line(program: &OsStr, arguments: &[OsString]) -> io::Result<Vec<u16>> {
    let mut result = Vec::new();
    for argument in std::iter::once(program).chain(arguments.iter().map(OsString::as_os_str)) {
        if !result.is_empty() {
            result.push(b' ' as u16);
        }
        result.push(b'"' as u16);
        let mut slashes = 0;
        for unit in argument.encode_wide() {
            match unit {
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "command contains a NUL character",
                    ));
                }
                92 => slashes += 1,
                _ => {
                    result.extend(std::iter::repeat_n(
                        92,
                        slashes * if unit == 34 { 2 } else { 1 },
                    ));
                    if unit == 34 {
                        result.push(92);
                    }
                    result.push(unit);
                    slashes = 0;
                }
            }
        }
        result.extend(std::iter::repeat_n(92, slashes * 2));
        result.push(34);
    }
    result.push(0);
    if result.len() > 32_767 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "command line exceeds the Windows limit",
        ));
    }
    Ok(result)
}
