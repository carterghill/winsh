use std::{
    fmt,
    fs::File,
    io::{self, Write},
    process::{Command, Stdio},
};

/// Command I/O. Unspecified streams inherit the host's standard handles.
/// Files can be supplied for both built-ins and native children, without
/// capturing entire child output in memory.
#[derive(Default)]
pub struct ShellIo {
    input: Option<File>,
    output: Option<File>,
    error: Option<File>,
}

impl ShellIo {
    pub fn inherited() -> Self {
        Self::default()
    }

    pub fn with_stdin(mut self, file: File) -> Self {
        self.input = Some(file);
        self
    }

    pub fn with_stdout(mut self, file: File) -> Self {
        self.output = Some(file);
        self
    }

    pub fn with_stderr(mut self, file: File) -> Self {
        self.error = Some(file);
        self
    }

    pub(crate) fn out(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        match &mut self.output {
            Some(file) => file.write_fmt(args),
            None => io::stdout().write_fmt(args),
        }
    }

    pub(crate) fn err(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        match &mut self.error {
            Some(file) => file.write_fmt(args),
            None => io::stderr().write_fmt(args),
        }
    }

    pub(crate) fn configure(&self, command: &mut Command) -> io::Result<()> {
        fn stream(file: &Option<File>) -> io::Result<Stdio> {
            file.as_ref()
                .map(|file| file.try_clone().map(Stdio::from))
                .unwrap_or_else(|| Ok(Stdio::inherit()))
        }
        command
            .stdin(stream(&self.input)?)
            .stdout(stream(&self.output)?)
            .stderr(stream(&self.error)?);
        Ok(())
    }
}
