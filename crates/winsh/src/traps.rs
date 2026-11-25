use crate::{Outcome, Shell, ShellIo, language_builtins::fail};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub(crate) enum Event {
    Exit,
    Hup,
    Int,
    Term,
    Err,
}
const SIGNALS: [Event; 3] = [Event::Hup, Event::Int, Event::Term];
const EVENTS: [Event; 5] = [Event::Exit, Event::Hup, Event::Int, Event::Term, Event::Err];
impl Event {
    fn parse(text: &str) -> Option<Self> {
        let text = text.to_ascii_uppercase();
        Some(match text.strip_prefix("SIG").unwrap_or(&text) {
            "0" | "EXIT" => Self::Exit,
            "1" | "HUP" => Self::Hup,
            "2" | "INT" => Self::Int,
            "15" | "TERM" => Self::Term,
            "ERR" => Self::Err,
            _ => return None,
        })
    }
    fn name(self) -> &'static str {
        match self {
            Self::Exit => "EXIT",
            Self::Err => "ERR",
            Self::Hup => "SIGHUP",
            Self::Int => "SIGINT",
            Self::Term => "SIGTERM",
        }
    }
    fn bit(self) -> u8 {
        1 << self as u8
    }
}

static SIGNAL_COUNTS: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
/// Notify CLI sessions of an OS signal. Actual trap evaluation occurs on
/// the evaluator thread, with the session's current variables and streams.
pub fn notify_signal(number: i32) {
    let index = match number {
        1 => 0,
        2 => 1,
        15 => 2,
        _ => return,
    };
    SIGNAL_COUNTS[index].fetch_add(1, Ordering::Relaxed);
}

#[derive(Default, Clone)]
pub(crate) struct Traps {
    pub(crate) actions: BTreeMap<Event, String>,
    inherited_listing: Option<Arc<BTreeMap<Event, String>>>,
    active: u8,
    observed: Option<[usize; 3]>,
    mailbox: Arc<AtomicU8>,
    forwards_to_parent: bool,
}
impl Traps {
    pub(crate) fn clear(&mut self) {
        self.actions.clear();
        self.inherited_listing = None;
    }
    pub(crate) fn listing(&self) -> &BTreeMap<Event, String> {
        self.inherited_listing.as_deref().unwrap_or(&self.actions)
    }
    pub(crate) fn inherit(&mut self, actions: BTreeMap<Event, String>) {
        for &signal in &SIGNALS {
            if actions.get(&signal).is_some_and(String::is_empty) {
                self.actions.insert(signal, String::new());
            }
        }
        if !actions.is_empty() {
            self.inherited_listing = Some(Arc::new(actions));
        }
    }
    pub(crate) fn subshell(&self) -> Self {
        let mut traps = Self {
            mailbox: self.mailbox.clone(),
            forwards_to_parent: true,
            observed: self.observed,
            ..Default::default()
        };
        traps.inherit(self.listing().clone());
        traps
    }
    pub(crate) fn script_child(&self) -> Self {
        let mut traps = Self {
            observed: self.observed,
            ..Default::default()
        };
        for signal in SIGNALS {
            if self.actions.get(&signal).is_some_and(String::is_empty) {
                traps.actions.insert(signal, String::new());
            }
        }
        traps
    }
}

pub(crate) fn builtin(
    shell: &mut Shell,
    args: &[OsString],
    io: &mut ShellIo,
) -> io::Result<Outcome> {
    let args = args.strip_prefix(&[OsString::from("--")]).unwrap_or(args);
    if args.len() == 1 && args[0] == "-l" {
        io.out(format_args!("1) SIGHUP\n2) SIGINT\n15) SIGTERM\n"))?;
        return Ok(Outcome::Continue(0));
    }
    let print = args.is_empty() || args[0] == "-p";
    let reset = !print
        && args[0].to_str().is_some_and(|s| {
            (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                || (args.len() == 1 && Event::parse(s).is_some())
        });
    let names = if reset {
        args
    } else {
        args.get(1..).unwrap_or_default()
    };
    let names = if print {
        names.strip_prefix(&[OsString::from("--")]).unwrap_or(names)
    } else {
        names
    };
    let mut events = Vec::new();
    let mut status = 0;
    for name in names {
        let Some(event) = name.to_str().and_then(Event::parse) else {
            io.err(format_args!(
                "winsh: trap: {}: unsupported signal\n",
                name.to_string_lossy()
            ))?;
            status = 1;
            continue;
        };
        events.push(event);
    }
    if print {
        if names.is_empty() {
            events.extend(EVENTS);
        }
        for event in events {
            if let Some(action) = shell.traps.listing().get(&event) {
                io.out(format_args!(
                    "trap -- '{}' {}\n",
                    action.replace('\'', "'\\''"),
                    event.name()
                ))?;
            }
        }
    } else {
        if names.is_empty() {
            return fail(io, "trap", "expected action and signal", 2);
        }
        let Some(action) = args[0].to_str() else {
            return fail(io, "trap", "action must be UTF-8", 2);
        };
        shell.traps.inherited_listing = None;
        for event in events {
            if reset || action == "-" {
                shell.traps.actions.remove(&event);
            } else {
                shell.traps.actions.insert(event, action.into());
            }
        }
    }
    Ok(Outcome::Continue(status))
}

impl Shell {
    pub fn observe_signals(&mut self) {
        self.traps.observed = Some(std::array::from_fn(|i| {
            SIGNAL_COUNTS[i].load(Ordering::Relaxed)
        }));
    }
    pub(crate) fn foreground_completed(&mut self, status: i32) {
        // The child shared our keyboard interrupt and chose to continue.
        if status != 130
            && !self.traps.actions.contains_key(&Event::Int)
            && let Some(seen) = &mut self.traps.observed
        {
            seen[1] = SIGNAL_COUNTS[1].load(Ordering::Relaxed);
        }
    }

    fn run_trap(&mut self, event: Event, status: i32, io: &mut ShellIo) -> io::Result<Outcome> {
        let Some(action) = self.traps.actions.get(&event).cloned() else {
            return Ok(Outcome::Continue(status));
        };
        if action.is_empty() || self.traps.active & event.bit() != 0 {
            return Ok(Outcome::Continue(status));
        }
        self.traps.active |= event.bit();
        self.last_status = status;
        let flow = self.flow.take();
        let result = self.run(&action, io);
        self.traps.active &= !event.bit();
        if self.flow.is_none() {
            self.flow = flow;
        }
        let outcome = match result? {
            Outcome::Exit(code) => Outcome::Exit(code),
            Outcome::Continue(_) => Outcome::Continue(status),
        };
        Ok(self.record(outcome))
    }

    /// Finish this shell session and run its EXIT action once. Interactive
    /// and embedded sessions may execute many command lists before this point.
    pub fn finish(&mut self, status: i32, io: &mut ShellIo) -> io::Result<i32> {
        let result = self.run_trap(Event::Exit, status, io);
        self.traps.actions.remove(&Event::Exit);
        Ok(result?.status())
    }

    pub(crate) fn error_trap(&mut self, status: i32, io: &mut ShellIo) -> io::Result<Outcome> {
        self.run_trap(Event::Err, status, io)
    }

    pub(crate) fn kill_self(&self, args: &[OsString]) -> bool {
        let Some((pid, options)) = args.split_last() else {
            return false;
        };
        if self.shell_pid != std::process::id()
            || pid.to_str().and_then(|s| s.parse().ok()) != Some(self.shell_pid)
        {
            return false;
        }
        let signal = match options {
            [] => Some("TERM"),
            [option] => option.to_str().and_then(|s| s.strip_prefix('-')),
            [flag, name] if flag == "-s" || flag == "-n" => name.to_str(),
            _ => None,
        };
        let Some(signal) = signal else {
            return false;
        };
        if signal == "0" {
            return true;
        }
        let Some(event) = Event::parse(signal).filter(|s| SIGNALS.contains(s)) else {
            return false;
        };
        self.traps.mailbox.fetch_or(event.bit(), Ordering::Relaxed);
        true
    }

    pub(crate) fn pending_signals(&mut self, io: &mut ShellIo) -> io::Result<Outcome> {
        let mut pending = if self.traps.forwards_to_parent {
            0
        } else {
            self.traps.mailbox.swap(0, Ordering::Relaxed)
        };
        if let Some(observed) = &mut self.traps.observed {
            for (i, event) in SIGNALS.iter().enumerate() {
                let count = SIGNAL_COUNTS[i].load(Ordering::Relaxed);
                if count != observed[i] {
                    observed[i] = count;
                    pending |= event.bit();
                }
            }
        }
        let mut result = Outcome::Continue(self.last_status);
        for event in SIGNALS {
            if pending & event.bit() == 0 {
                continue;
            }
            if self.traps.actions.contains_key(&event) {
                result = self.run_trap(event, self.last_status, io)?;
            } else {
                let code = match event {
                    Event::Int => 130,
                    Event::Term => 143,
                    _ => 129,
                };
                result = if self.jobs.interactive && event == Event::Int {
                    Outcome::Continue(code)
                } else {
                    Outcome::Exit(code)
                };
            }
            if matches!(result, Outcome::Exit(_)) {
                break;
            }
        }
        Ok(self.record(result))
    }

    pub fn interrupt_input(&mut self, io: &mut ShellIo) -> io::Result<Outcome> {
        // Line editors can report Ctrl+C without generating an OS signal.
        let signalled = self
            .traps
            .observed
            .is_some_and(|old| (0..3).any(|i| old[i] != SIGNAL_COUNTS[i].load(Ordering::Relaxed)));
        if !signalled {
            self.traps
                .mailbox
                .fetch_or(Event::Int.bit(), Ordering::Relaxed);
        }
        self.interrupt();
        self.pending_signals(io)
    }
}
