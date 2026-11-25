use crate::{
    Outcome, Shell, ShellIo, builtins,
    streams::Stream,
    syntax::{PipeKind, ScriptPipeline},
};
use std::{ffi::OsString, io, thread};

/// Every stage owns its session and pipe ends. Start all workers before waiting;
/// this also lets compound commands and functions stream through native pipes.
pub(crate) fn run(shell: &Shell, pipeline: &ScriptPipeline, streams: &ShellIo) -> io::Result<i32> {
    // Allocate and clone every handle before starting workers, so setup failures
    // cannot leave a producer waiting on a consumer that was never started.
    let mut inputs = Vec::new();
    let mut next_input = None;
    for index in 0..pipeline.commands.len() {
        let mut io = streams.try_clone()?;
        if let Some(reader) = next_input.take() {
            io.set(0, Stream::Reader(reader))?;
        }
        if pipeline.connections.get(index).is_some() {
            let (reader, writer) = io::pipe()?;
            io.set(1, Stream::Writer(writer))?;
            next_input = Some(reader);
        }
        inputs.push(io);
    }
    thread::scope(|scope| {
        let cancellation = shell.cancellation.clone().unwrap_or_default();
        let mut workers = Vec::new();
        for (index, (command, mut io)) in pipeline.commands.iter().zip(inputs).enumerate() {
            let mut session = shell.subshell();
            session.cancellation = Some(cancellation.clone());
            let cancellation = cancellation.clone();
            let pipe_stderr = pipeline.connections.get(index) == Some(&PipeKind::StdoutAndStderr);
            workers.push(scope.spawn(move || {
                let result = session
                    .command(command, &mut io, pipe_stderr)
                    .and_then(|outcome| session.finish(outcome.status(), &mut io));
                if result.is_err() {
                    cancellation.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                result
            }));
        }
        let mut result = Ok(0);
        for worker in workers {
            let status = worker
                .join()
                .map_err(|_| io::Error::other("pipeline worker panicked"))
                .and_then(|result| result);
            // Join in pipeline order. With pipefail, retain the rightmost
            // failure even if a later stage succeeds. Preserve host I/O errors.
            if result.is_ok() && (!shell.options.pipefail || !matches!(status, Ok(0))) {
                result = status;
            }
        }
        result
    })
}
pub(crate) fn builtin(shell: &mut Shell, arguments: &[OsString], streams: &mut ShellIo) -> Outcome {
    match builtins::dispatch(shell, arguments, streams) {
        Ok(Some(outcome)) => outcome,
        Ok(None) => unreachable!(),
        Err(error) => {
            let _ = streams.err(format_args!(
                "winsh: {}: {error}\n",
                arguments[0].to_string_lossy()
            ));
            Outcome::Continue(1)
        }
    }
}
