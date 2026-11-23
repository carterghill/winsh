use std::{ffi::OsString, io, thread};

use crate::{
    Outcome, Shell, ShellIo, builtins, exec, expand,
    redirect::{self, Redirect},
    streams::Stream,
    syntax::{PipeKind, SimpleCommand},
};

pub(crate) struct PreparedCommand {
    pub arguments: Vec<OsString>,
    pub redirects: Vec<Redirect>,
}

impl PreparedCommand {
    pub(crate) fn expand(
        command: &SimpleCommand,
        shell: &Shell,
    ) -> Result<Self, expand::ExpansionError> {
        Ok(Self {
            arguments: expand::arguments(command, shell)?,
            redirects: command
                .redirections
                .iter()
                .map(|redirect| {
                    Ok(Redirect {
                        descriptor: redirect.descriptor,
                        kind: redirect.kind,
                        target: expand::word(&redirect.target, shell)?,
                    })
                })
                .collect::<Result<_, expand::ExpansionError>>()?,
        })
    }
}

/// All stages are started before any waits. Only the endpoints needed by the
/// next stage remain in the parent; Command and stage I/O copies drop promptly
/// so downstream EOF and upstream broken-pipe detection work reliably.
pub(crate) fn run(
    shell: &Shell,
    commands: Vec<PreparedCommand>,
    connections: &[PipeKind],
    streams: &ShellIo,
) -> io::Result<i32> {
    thread::scope(|scope| {
        let mut stages = Vec::new();
        let mut next_input = None;
        for (index, command) in commands.into_iter().enumerate() {
            let mut io = streams.try_clone()?;
            if let Some(reader) = next_input.take() {
                io.set(0, Stream::Reader(reader))?;
            }
            if connections.get(index).is_some() {
                let (reader, writer) = io::pipe()?;
                io.set(1, Stream::Writer(writer))?;
                next_input = Some(reader);
            }
            if !redirect::apply(shell, &command.redirects, &mut io)? {
                stages.push(Stage::Complete(1));
                continue;
            }
            if connections.get(index) == Some(&PipeKind::StdoutAndStderr)
                && let Err(error) = io.duplicate(2, 1)
            {
                io.err(format_args!("winsh: |&: {error}\n"))?;
                stages.push(Stage::Complete(1));
                continue;
            }
            let Some(name) = command.arguments.first() else {
                stages.push(Stage::Complete(0));
                continue;
            };
            if builtins::recognizes(name) {
                let mut session = shell.clone();
                stages.push(Stage::Builtin(scope.spawn(move || {
                    builtin(&mut session, &command.arguments, &mut io).status()
                })));
            } else {
                stages.push(match exec::spawn(shell, &command.arguments, &mut io)? {
                    exec::Spawned::Running(child) => Stage::Native(child),
                    exec::Spawned::Complete(status) => Stage::Complete(status),
                });
            }
        }
        drop(next_input);
        let mut status = 0;
        for stage in stages {
            status = match stage {
                Stage::Native(mut child) => child.wait()?,
                Stage::Builtin(worker) => worker
                    .join()
                    .map_err(|_| io::Error::other("pipeline built-in panicked"))?,
                Stage::Complete(status) => status,
            };
        }
        Ok(status)
    })
}

enum Stage<'scope> {
    Native(exec::ChildProcess),
    Builtin(thread::ScopedJoinHandle<'scope, i32>),
    Complete(i32),
}

pub(crate) fn builtin(shell: &mut Shell, arguments: &[OsString], streams: &mut ShellIo) -> Outcome {
    match builtins::dispatch(shell, arguments, streams) {
        Ok(Some(outcome)) => outcome,
        Ok(None) => unreachable!("only recognized built-ins are dispatched"),
        Err(error) => {
            // A failed output write (including a downstream reader exiting)
            // fails the command, not the entire interactive session.
            let _ = streams.err(format_args!(
                "winsh: {}: {error}\n",
                arguments[0].to_string_lossy()
            ));
            Outcome::Continue(1)
        }
    }
}
