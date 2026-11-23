use std::{
    env,
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
};

use crate::{ShellIo, builtins, exec, expand, platform, syntax};

/// A command result is distinct from an instruction to leave the input loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Continue(i32),
    Exit(i32),
}

impl Outcome {
    pub fn status(self) -> i32 {
        match self {
            Self::Continue(status) | Self::Exit(status) => status,
        }
    }
}

/// One shell session. Changing it never changes the host process's cwd or env.
pub struct Shell {
    pub(crate) cwd: PathBuf,
    pub(crate) environment: Vec<(OsString, OsString)>,
    last_status: i32,
}

impl Shell {
    /// Start a session from the host's current directory and environment.
    pub fn new() -> io::Result<Self> {
        Self::with_environment(env::current_dir()?, env::vars_os())
    }

    /// Start with explicit state, useful for embedding and isolated tests.
    pub fn with_environment(
        cwd: PathBuf,
        environment: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> io::Result<Self> {
        if !cwd.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "shell working directory must be absolute",
            ));
        }
        platform::require_directory(&cwd)?;
        let mut shell = Self {
            cwd,
            environment: environment.into_iter().collect(),
            last_status: 0,
        };
        shell.set_env("PWD", shell.cwd.clone().into_os_string());
        Ok(shell)
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn last_status(&self) -> i32 {
        self.last_status
    }

    pub fn environment(&self) -> &[(OsString, OsString)] {
        &self.environment
    }

    /// Parse and execute one command. Command and syntax failures are reported
    /// through stderr and a shell status; Err is reserved for host I/O failures.
    pub fn run(&mut self, source: &str, io: &mut ShellIo) -> io::Result<Outcome> {
        match syntax::parse_simple_command(source) {
            Ok(Some(command)) => self.execute(&command, io),
            Ok(None) => Ok(Outcome::Continue(self.last_status)),
            Err(error) => {
                io.err(format_args!("winsh: {error}\n"))?;
                Ok(self.record(Outcome::Continue(2)))
            }
        }
    }

    /// Execute parsed syntax through the same path used by run().
    pub fn execute(
        &mut self,
        command: &syntax::SimpleCommand,
        io: &mut ShellIo,
    ) -> io::Result<Outcome> {
        let arguments = match expand::arguments(command, self) {
            Ok(arguments) => arguments,
            Err(error) => {
                io.err(format_args!("winsh: {error}\n"))?;
                return Ok(self.record(Outcome::Continue(2)));
            }
        };
        if arguments.is_empty() {
            return Ok(Outcome::Continue(self.last_status));
        }
        let outcome = match builtins::dispatch(self, &arguments, io)? {
            Some(outcome) => outcome,
            None => Outcome::Continue(exec::run(self, &arguments, io)?),
        };
        Ok(self.record(outcome))
    }

    /// Record an input interruption without terminating the session.
    pub fn interrupt(&mut self) {
        self.last_status = 130;
    }

    fn record(&mut self, outcome: Outcome) -> Outcome {
        self.last_status = outcome.status();
        outcome
    }

    pub(crate) fn env(&self, name: &str) -> Option<&OsStr> {
        platform::env_value(&self.environment, name)
    }

    pub(crate) fn home(&self) -> Option<&OsStr> {
        self.env("HOME").or_else(|| {
            if cfg!(windows) {
                self.env("USERPROFILE")
            } else {
                None
            }
        })
    }

    pub(crate) fn set_env(&mut self, name: &str, value: OsString) {
        self.environment
            .retain(|(key, _)| !platform::env_name_eq(key, name));
        self.environment.push((OsString::from(name), value));
    }
}
