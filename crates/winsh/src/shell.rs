use std::{
    env,
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
};

use crate::{
    ShellIo, builtins, exec,
    pipeline::{self, PreparedCommand},
    platform, redirect, syntax,
};

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
#[derive(Clone)]
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

    /// Parse and execute one foreground pipeline. Command and syntax failures
    /// are reported through stderr and a status; Err is for host I/O failures.
    pub fn run(&mut self, source: &str, io: &mut ShellIo) -> io::Result<Outcome> {
        match syntax::parse_pipeline(source) {
            Ok(Some(pipeline)) => self.execute_pipeline(&pipeline, io),
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
        self.evaluate(std::slice::from_ref(command), &[], io)
    }

    pub fn execute_pipeline(
        &mut self,
        pipeline: &syntax::Pipeline,
        io: &mut ShellIo,
    ) -> io::Result<Outcome> {
        self.evaluate(&pipeline.commands, &pipeline.connections, io)
    }

    fn evaluate(
        &mut self,
        commands: &[syntax::SimpleCommand],
        connections: &[syntax::PipeKind],
        io: &mut ShellIo,
    ) -> io::Result<Outcome> {
        if commands.is_empty() || connections.len() != commands.len() - 1 {
            io.err(format_args!("winsh: invalid pipeline syntax tree\n"))?;
            return Ok(self.record(Outcome::Continue(2)));
        }
        // Validate every argument and target before opening files or starting
        // processes. Unsupported language syntax cannot cause partial execution.
        let mut prepared = match commands
            .iter()
            .map(|command| PreparedCommand::expand(command, self))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(commands) => commands,
            Err(error) => {
                io.err(format_args!("winsh: {error}\n"))?;
                return Ok(self.record(Outcome::Continue(2)));
            }
        };
        let outcome = if prepared.len() > 1 {
            Outcome::Continue(pipeline::run(self, prepared, connections, io)?)
        } else {
            let command = prepared.pop().unwrap();
            let mut streams = io.try_clone()?;
            if !redirect::apply(self, &command.redirects, &mut streams)? {
                Outcome::Continue(1)
            } else if let Some(name) = command.arguments.first() {
                if builtins::recognizes(name) {
                    pipeline::builtin(self, &command.arguments, &mut streams)
                } else {
                    Outcome::Continue(exec::run(self, &command.arguments, &mut streams)?)
                }
            } else {
                Outcome::Continue(0)
            }
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
