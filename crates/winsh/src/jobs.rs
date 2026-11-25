//! Each job has a real worker PID, its own shell state and an owned process tree.
use crate::{
    Outcome, Shell, ShellIo, exec, platform,
    syntax::{AndOr, Command as ShellCommand, Script, ScriptPipeline},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as os;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as os;

pub(crate) const BUILTINS: &[&str] = &["jobs", "fg", "bg", "wait", "kill", "stop"];
static INTERRUPTS: AtomicUsize = AtomicUsize::new(0);
static STOPS: AtomicUsize = AtomicUsize::new(0);
/// Notify an active foreground wait from the CLI console handler.
pub fn interrupt_foreground() {
    INTERRUPTS.fetch_add(1, Ordering::Relaxed);
}
pub fn stop_foreground() {
    STOPS.fetch_add(1, Ordering::Relaxed);
}

#[derive(Default)]
pub(crate) struct Jobs {
    executable: Option<PathBuf>,
    pub(crate) interactive: bool,
    pub(crate) last_pid: Option<u32>,
    sequence: u32,
    entries: BTreeMap<u32, Job>,
    selection: Vec<u32>,
    inherited: Vec<Listing>,
}
// A subshell inherits values, never the authority to manipulate its parent's jobs.
impl Clone for Jobs {
    fn clone(&self) -> Self {
        Self {
            executable: self.executable.clone(),
            last_pid: self.last_pid,
            inherited: self.listings(),
            sequence: self.sequence,
            selection: self.selection.clone(),
            ..Self::default()
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum State {
    Running,
    Stopped,
    Done(i32),
}
#[derive(Clone, Serialize, Deserialize)]
struct Listing {
    id: u32,
    pid: u32,
    state: State,
    label: String,
}
impl Listing {
    fn show(&self, io: &mut ShellIo) -> io::Result<()> {
        let state = match self.state {
            State::Running => "Running".into(),
            State::Stopped => "Stopped".into(),
            State::Done(status) => format!("Done({status})"),
        };
        io.out(format_args!(
            "[{}] {state}  {}  {}\n",
            self.id, self.pid, self.label
        ))
    }
}
struct Process {
    child: Child,
    group: os::Group,
}
struct Job {
    pid: u32,
    label: String,
    state: State,
    process: Option<Process>,
    input: Option<os::InputRelay>,
    notified: bool,
    #[cfg(windows)]
    modes: Option<os::Modes>,
}
struct WaitControl {
    interrupts: usize,
    stops: usize,
    terminal: Option<os::Foreground>,
}
impl WaitControl {
    fn new(pid: u32, terminal: bool) -> io::Result<Self> {
        Ok(Self {
            interrupts: INTERRUPTS.load(Ordering::Relaxed),
            stops: STOPS.load(Ordering::Relaxed),
            terminal: if terminal {
                Some(os::Foreground::enter(pid)?)
            } else {
                None
            },
        })
    }
}
impl Job {
    fn poll(&mut self) -> io::Result<()> {
        if let Some(process) = &mut self.process
            && let Some(state) = process.group.poll(&mut process.child)?
        {
            self.state = state;
            if matches!(state, State::Done(_)) {
                self.process = None;
                self.input = None;
            }
        }
        Ok(())
    }
    fn freeze(&mut self, stopped: bool) -> io::Result<()> {
        let process = self
            .process
            .as_ref()
            .ok_or_else(|| io::Error::other("job already finished"))?;
        let wanted = if stopped {
            State::Stopped
        } else {
            State::Running
        };
        if self.state != wanted {
            process.group.freeze(stopped)?;
            self.state = wanted;
        }
        Ok(())
    }
    fn listing(&self, id: u32) -> Listing {
        Listing {
            id,
            pid: self.pid,
            state: self.state,
            label: self.label.clone(),
        }
    }
    fn wait(&mut self, foreground: bool, mut control: WaitControl) -> io::Result<i32> {
        #[cfg(windows)]
        if control.terminal.is_some()
            && let Some(modes) = self.modes.take()
        {
            modes.restore();
        }
        if foreground && self.state == State::Stopped {
            self.freeze(false)?;
        }
        loop {
            self.poll()?;
            match self.state {
                State::Done(status) => return Ok(status),
                State::Stopped => return Ok(148),
                State::Running => {}
            }
            let interrupts = INTERRUPTS.load(Ordering::Relaxed);
            if interrupts != control.interrupts {
                if foreground {
                    self.terminate(130)?;
                    control.interrupts = interrupts;
                } else {
                    return Ok(130);
                }
            } else if foreground
                && (STOPS.load(Ordering::Relaxed) != control.stops
                    || control
                        .terminal
                        .as_ref()
                        .is_some_and(|t| t.stop_requested()))
            {
                self.freeze(true)?;
                #[cfg(windows)]
                if control.terminal.is_some() {
                    self.modes = Some(os::Modes::current());
                }
                return Ok(148);
            }
            if let (Some(terminal), Some(input)) = (&mut control.terminal, &mut self.input)
                && !terminal.forward_input(input)?
            {
                self.input = None;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn terminate(&mut self, status: i32) -> io::Result<()> {
        let process = self
            .process
            .as_ref()
            .ok_or_else(|| io::Error::other("job already finished"))?;
        process.group.terminate(status)?;
        #[cfg(unix)]
        if self.state == State::Stopped && status != 137 {
            process.group.freeze(false)?;
        }
        self.state = State::Running;
        Ok(())
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        if let Some(p) = &mut self.process {
            let _ = p.group.terminate(137);
            let _ = p.child.wait();
        }
    }
}

#[derive(Serialize, Deserialize)]
enum Task {
    List(AndOr),
    Pipeline(ScriptPipeline),
    Program {
        args: Vec<OsString>,
        #[serde(skip)]
        relay_input: bool,
    },
}
#[derive(Serialize, Deserialize)]
enum Input {
    Empty,
    Closed,
    Terminal,
    Pipe,
    Descriptor(u64),
}
#[derive(Serialize, Deserialize)]
struct Request {
    task: Task,
    input: Input,
    variables: BTreeMap<String, OsString>,
    unassigned_exports: Vec<OsString>,
    options: crate::shell::ShellOptions,
    ignore_errexit: bool,
    functions: BTreeMap<String, ShellCommand>,
    aliases: BTreeMap<String, crate::aliases::Alias>,
    args: Vec<OsString>,
    arg0: OsString,
    status: i32,
    last_pid: Option<u32>,
    shell_pid: u32,
    listings: Vec<Listing>,
    selection: Vec<u32>,
    source_depth: usize,
    function_depth: usize,
    loop_depth: usize,
    utilities: Option<PathBuf>,
}

impl Request {
    fn read(mut input: File) -> io::Result<Self> {
        let mut length = [0; 8];
        input.read_exact(&mut length)?;
        let length = u64::from_le_bytes(length);
        let mut frame = Vec::new();
        if input.take(length).read_to_end(&mut frame)? as u64 != length {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        // Each of the parser's 64 allowed compound levels has several nested
        // JSON objects and arrays. Bound that wire depth before decoding it.
        let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
        for &byte in &frame {
            if escaped {
                escaped = false;
            } else if quoted && byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = !quoted;
            } else if !quoted {
                match byte {
                    b'[' | b'{' => {
                        depth += 1;
                        if depth > 1024 {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "job nesting limit",
                            ));
                        }
                    }
                    b']' | b'}' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
        }
        let mut decoder = serde_json::Deserializer::from_slice(&frame);
        decoder.disable_recursion_limit();
        let request = Self::deserialize(&mut decoder)?;
        decoder.end()?;
        Ok(request)
    }
}

impl Shell {
    /// Specify the winsh binary used to host jobs when embedding the engine.
    pub fn set_job_executable(&mut self, executable: PathBuf) {
        self.jobs.executable = Some(executable);
    }
    pub fn enable_interactive_jobs(&mut self) {
        self.jobs.interactive = true;
    }
    /// Internal worker entry point. The parent closes the bootstrap transaction
    /// before this process can execute any part of the user's job.
    #[doc(hidden)]
    pub fn run_job_worker(&mut self, streams: ShellIo) -> io::Result<i32> {
        // Debug builds also need room for serde's nested visitor frames.
        thread::scope(|scope| {
            thread::Builder::new()
                .name("job".into())
                .stack_size(16 * 1024 * 1024)
                .spawn_scoped(scope, || self.execute_job(streams))?
                .join()
                .map_err(|_| io::Error::other("job worker panicked"))?
        })
    }
    fn execute_job(&mut self, streams: ShellIo) -> io::Result<i32> {
        let request = Request::read(streams.input_file()?)?;
        let mut streams = match request.input {
            Input::Pipe => streams,
            Input::Closed => {
                let mut streams = streams;
                streams.set(0, crate::streams::Stream::Closed)?;
                streams
            }
            Input::Empty => {
                streams.with_stdin(File::open(if cfg!(windows) { "NUL" } else { "/dev/null" })?)
            }
            Input::Terminal => streams.with_stdin(os::terminal_input()?),
            Input::Descriptor(fd) => streams.with_stdin(os::received_input(fd)?),
        };
        self.variables = request.variables;
        self.unassigned_exports = request.unassigned_exports;
        self.options = request.options;
        self.ignore_errexit = request.ignore_errexit;
        self.functions = request.functions;
        self.aliases = request.aliases;
        self.set_arguments(request.arg0, request.args);
        self.last_status = request.status;
        self.jobs.last_pid = request.last_pid;
        self.shell_pid = request.shell_pid;
        self.jobs.sequence = request.listings.iter().map(|l| l.id).max().unwrap_or(0);
        self.jobs.inherited = request.listings;
        self.jobs.selection = request.selection;
        self.source_depth = request.source_depth;
        self.function_depth = request.function_depth;
        self.loop_depth = request.loop_depth;
        if self.function_depth != 0 {
            self.scopes.push(BTreeMap::new());
        }
        self.utility_executable = request.utilities;
        match request.task {
            Task::List(list) => Ok(self
                .execute_script(&Script { lists: vec![list] }, &mut streams)?
                .status()),
            Task::Pipeline(p) => Ok(self.evaluate_pipeline(&p, &mut streams)?.status()),
            Task::Program { args, .. } => exec::run(self, &args, &mut streams),
        }
    }
    pub(crate) fn launch_list(
        &mut self,
        list: AndOr,
        label: &str,
        io: &mut ShellIo,
    ) -> io::Result<Outcome> {
        self.launch(Task::List(list), label.to_owned(), true, io)
    }
    pub(crate) fn launch_pipeline(
        &mut self,
        pipeline: &ScriptPipeline,
        io: &mut ShellIo,
    ) -> io::Result<Outcome> {
        let label = pipeline.label.as_deref().unwrap_or("pipeline").to_owned();
        self.launch(Task::Pipeline(pipeline.clone()), label, false, io)
    }
    pub(crate) fn run_program(
        &mut self,
        args: &[OsString],
        utility: Option<crate::Utility>,
        io: &mut ShellIo,
    ) -> io::Result<i32> {
        if !self.jobs.interactive {
            return exec::run(self, args, io);
        }
        let display = if utility.is_some() { &args[2..] } else { args };
        let relay_input = cfg!(windows) && utility.is_some_and(|u| u.reads_stdin(&display[1..]));
        let label = display
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(self
            .launch(
                Task::Program {
                    args: args.to_vec(),
                    relay_input,
                },
                label,
                false,
                io,
            )?
            .status())
    }
    fn launch(
        &mut self,
        task: Task,
        label: String,
        background: bool,
        io: &mut ShellIo,
    ) -> io::Result<Outcome> {
        if !matches!(task, Task::Program { .. }) {
            self.jobs.refresh()?;
        }
        let (mut job, control) = match self.start_job(task, label, background, io) {
            Ok(job) => job,
            Err(e) => {
                io.err(format_args!("winsh: cannot start job: {e}\n"))?;
                return Ok(self.record(Outcome::Continue(1)));
            }
        };
        let status = if background {
            self.jobs.last_pid = Some(job.pid);
            let pid = job.pid;
            let id = self.jobs.insert(job)?;
            if self.jobs.interactive {
                io.err(format_args!("[{id}] {pid}\n"))?;
            }
            0
        } else {
            let status = match job.wait(true, control) {
                Ok(status) => status,
                Err(error) => {
                    let id = self.jobs.insert(job)?;
                    io.err(format_args!("winsh: foreground job %{id}: {error}\n"))?;
                    return Ok(self.record(Outcome::Continue(1)));
                }
            };
            if job.state == State::Stopped {
                let label = job.label.clone();
                let id = self.jobs.insert(job)?;
                io.err(format_args!("[{id}] Stopped  {label}\n"))?;
            }
            status
        };
        Ok(self.record(Outcome::Continue(status)))
    }
    fn start_job(
        &self,
        task: Task,
        label: String,
        background: bool,
        streams: &ShellIo,
    ) -> io::Result<(Job, WaitControl)> {
        let executable = match &self.jobs.executable {
            Some(p) => p.clone(),
            None => exec::resolve(self, "winsh".as_ref())?,
        };
        let mut command = Command::new(executable);
        command
            .arg("--job-worker")
            .current_dir(&self.cwd)
            .env_clear()
            .envs(self.environment.iter().cloned());
        streams.configure(&mut command)?;
        command.stdin(Stdio::piped());
        os::configure(&mut command);
        let terminal = streams.inherits_console_input();
        let file = if !streams.input_closed()
            && !terminal
            && (!background || !streams.input_inherited())
        {
            Some(streams.input_file()?)
        } else {
            None
        };
        if let Some(f) = &file {
            os::prepare_input(&mut command, f);
        }
        let mut child = platform::spawn(&mut command)?;
        let group = match os::Group::attach(&child) {
            Ok(group) => group,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        let mut writer = child.stdin.take().expect("worker has piped bootstrap");
        let mut job = Job {
            pid: child.id(),
            label,
            state: State::Running,
            input: None,
            notified: false,
            #[cfg(windows)]
            modes: None,
            process: Some(Process { child, group }),
        };
        let control = WaitControl::new(job.pid, self.jobs.interactive && !background)?;
        let input = if let Some(file) = file {
            Input::Descriptor(os::transfer_input(
                &job.process.as_ref().unwrap().child,
                &file,
            )?)
        } else if streams.input_closed() {
            Input::Closed
        } else if background && (!self.jobs.interactive || !terminal) {
            Input::Empty
        } else if terminal
            && cfg!(windows)
            && (background
                || matches!(
                    task,
                    Task::Program {
                        relay_input: true,
                        ..
                    }
                ))
        {
            Input::Pipe
        } else if terminal {
            Input::Terminal
        } else {
            Input::Empty
        };
        let keep_input = matches!(input, Input::Pipe);
        let shell_task = !matches!(task, Task::Program { .. });
        let request = Request {
            task,
            input,
            variables: if shell_task {
                self.variables.clone()
            } else {
                BTreeMap::new()
            },
            functions: if shell_task {
                self.functions.clone()
            } else {
                BTreeMap::new()
            },
            aliases: if shell_task {
                self.aliases.clone()
            } else {
                BTreeMap::new()
            },
            args: self.positional.clone(),
            unassigned_exports: self.unassigned_exports.clone(),
            options: self.options,
            ignore_errexit: self.ignore_errexit,
            arg0: self.arg0.clone(),
            status: self.last_status,
            last_pid: self.jobs.last_pid,
            shell_pid: self.shell_pid,
            listings: if shell_task {
                self.jobs.listings()
            } else {
                Vec::new()
            },
            selection: if shell_task {
                self.jobs.selection.clone()
            } else {
                Vec::new()
            },
            source_depth: self.source_depth,
            function_depth: self.function_depth,
            loop_depth: self.loop_depth,
            utilities: self.utility_executable.clone(),
        };
        let bytes = serde_json::to_vec(&request)?;
        writer.write_all(&(bytes.len() as u64).to_le_bytes())?;
        writer.write_all(&bytes)?;
        if keep_input {
            job.input = Some(os::input_relay(writer)?);
        }
        Ok((job, control))
    }
    pub fn report_finished_jobs(&mut self, io: &mut ShellIo) -> io::Result<()> {
        self.jobs.refresh()?;
        for (&id, job) in &mut self.jobs.entries {
            if let State::Done(status) = job.state
                && !job.notified
            {
                io.err(format_args!("[{id}] Done({status})  {}\n", job.label))?;
                job.notified = true;
            }
        }
        Ok(())
    }
}

impl Jobs {
    fn listings(&self) -> Vec<Listing> {
        self.inherited
            .iter()
            .cloned()
            .chain(self.entries.iter().map(|(&id, job)| job.listing(id)))
            .collect()
    }
    fn refresh(&mut self) -> io::Result<()> {
        for job in self.entries.values_mut() {
            job.poll()?;
        }
        self.selection.retain(|id| {
            self.entries.get(id).is_some_and(|j| j.process.is_some())
                || self
                    .inherited
                    .iter()
                    .any(|j| j.id == *id && !matches!(j.state, State::Done(_)))
        });
        Ok(())
    }
    fn insert(&mut self, job: Job) -> io::Result<u32> {
        self.refresh()?;
        // Keep recent wait statuses without retaining finished process handles.
        let done: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, j)| matches!(j.state, State::Done(_)))
            .map(|(&id, _)| id)
            .collect();
        for id in done.iter().take(done.len().saturating_sub(255)) {
            self.entries.remove(id);
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("job IDs exhausted"))?;
        self.entries.insert(self.sequence, job);
        self.selection.push(self.sequence);
        Ok(self.sequence)
    }
    fn find(&self, arg: Option<&OsString>) -> Option<u32> {
        let spec = match arg {
            Some(arg) => arg.to_str()?,
            None => "%+",
        };
        let mut active = self.selection.iter().rev().copied();
        match spec {
            "%" | "%+" | "%%" => active.next(),
            "%-" => active.nth(1),
            _ if spec.starts_with('%') => spec[1..]
                .parse()
                .ok()
                .filter(|id| self.entries.contains_key(id)),
            _ => {
                let pid: u32 = spec.parse().ok()?;
                self.entries
                    .iter()
                    .rev()
                    .find(|(_, j)| j.pid == pid)
                    .map(|(&id, _)| id)
            }
        }
    }
    pub(crate) fn builtin(
        &mut self,
        name: &str,
        args: &[OsString],
        io: &mut ShellIo,
    ) -> io::Result<i32> {
        self.refresh()?;
        let (mut action, mut args) = (name, args);
        if name == "jobs" {
            let listings = self.listings();
            if args.is_empty() {
                for job in listings {
                    job.show(io)?;
                }
                return Ok(0);
            }
            let mut status = 0;
            for arg in args {
                let current = self.find(Some(arg));
                let selected = listings.iter().rev().find(|j| {
                    current == Some(j.id)
                        || arg.to_str().is_some_and(|s| {
                            if let Some(id) = s.strip_prefix('%') {
                                id.parse() == Ok(j.id)
                            } else {
                                s.parse() == Ok(j.pid)
                            }
                        })
                });
                if let Some(job) = selected {
                    job.show(io)?;
                } else {
                    io.err(format_args!(
                        "winsh: jobs: {}: no such job\n",
                        arg.to_string_lossy()
                    ))?;
                    status = 1;
                }
            }
            return Ok(status);
        }
        let mut signal = 143;
        if name == "kill"
            && let Some(option) = args
                .first()
                .and_then(|s| s.to_str())
                .filter(|s| s.starts_with('-'))
        {
            match option {
                "-STOP" | "-TSTP" => action = "stop",
                "-CONT" => action = "bg",
                "-KILL" | "-9" => signal = 137,
                "-INT" | "-2" => signal = 130,
                "-TERM" | "-15" => {}
                _ => {
                    io.err(format_args!("winsh: kill: unsupported signal {option}\n"))?;
                    return Ok(2);
                }
            }
            args = &args[1..];
        }
        if (matches!(name, "fg" | "bg" | "stop") && args.len() > 1)
            || (name == "kill" && args.is_empty())
        {
            io.err(format_args!(
                "winsh: {name}: expected a job specification such as %1\n"
            ))?;
            return Ok(2);
        }
        let specs: Vec<_> = if args.is_empty() && action == "wait" {
            self.entries.keys().copied().map(Ok).collect()
        } else {
            let input = if args.is_empty() {
                vec![None]
            } else {
                args.iter().map(Some).collect()
            };
            input
                .into_iter()
                .map(|arg| {
                    self.find(arg).ok_or_else(|| {
                        arg.map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "current job".into())
                    })
                })
                .collect()
        };
        let mut status = 0;
        for spec in specs {
            let id = match spec {
                Ok(id) if self.entries.contains_key(&id) => id,
                other => {
                    let spec = other.map(|id| format!("%{id}")).unwrap_or_else(|s| s);
                    io.err(format_args!("winsh: {name}: {spec}: no such job\n"))?;
                    status = if name == "wait" { 127 } else { 1 };
                    continue;
                }
            };
            let job = self.entries.get_mut(&id).unwrap();
            match action {
                "stop" | "bg" => {
                    let stop = action == "stop";
                    match job.freeze(stop) {
                        Ok(()) => {
                            if name == "bg" {
                                self.last_pid = Some(job.pid);
                            }
                            self.selection.retain(|n| *n != id);
                            self.selection.push(id);
                            if self.interactive {
                                job.listing(id).show(io)?;
                            }
                        }
                        Err(e) => {
                            io.err(format_args!("winsh: {name}: %{id}: {e}\n"))?;
                            status = 1;
                        }
                    }
                }
                "kill" => match job.terminate(signal) {
                    Ok(()) => {}
                    Err(e) => {
                        io.err(format_args!("winsh: kill: %{id}: {e}\n"))?;
                        status = 1;
                    }
                },
                "fg" | "wait" => {
                    if action == "fg" && self.interactive {
                        io.err(format_args!("{}\n", job.label))?;
                    }
                    let foreground = action == "fg";
                    let control = WaitControl::new(
                        job.pid,
                        foreground && self.interactive && job.process.is_some(),
                    )?;
                    status = job.wait(foreground, control)?;
                    if matches!(job.state, State::Done(_)) {
                        self.entries.remove(&id);
                        self.selection.retain(|n| *n != id);
                    } else if status == 130 {
                        return Ok(status);
                    } else if action == "fg" {
                        self.selection.retain(|n| *n != id);
                        self.selection.push(id);
                        if self.interactive {
                            job.listing(id).show(io)?;
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
        Ok(if name == "wait" && args.is_empty() {
            0
        } else {
            status
        })
    }
}
