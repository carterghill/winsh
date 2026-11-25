use std::{
    fmt,
    fs::File,
    io::{self, IsTerminal, PipeReader, PipeWriter, Read, Write},
    process::{Command, Stdio},
};

#[cfg(unix)]
use std::os::fd::OwnedFd as OwnedIo;
#[cfg(windows)]
use std::os::windows::io::OwnedHandle as OwnedIo;

/// Explicit streams for both built-ins and native programs. Inherited streams
/// remember their original descriptor so `2>&1` can snapshot stdout correctly.
pub struct ShellIo {
    streams: [Stream; 3],
    stderr_redirected: bool,
}

pub(crate) enum Stream {
    Closed,
    Inherited(u8),
    File(File),
    Reader(PipeReader),
    Writer(PipeWriter),
}

impl Default for ShellIo {
    fn default() -> Self {
        Self {
            streams: [
                Stream::Inherited(0),
                Stream::Inherited(1),
                Stream::Inherited(2),
            ],
            stderr_redirected: false,
        }
    }
}

impl ShellIo {
    pub fn inherited() -> Self {
        Self::default()
    }

    pub fn with_stdin(mut self, file: File) -> Self {
        self.streams[0] = Stream::File(file);
        self
    }
    pub fn with_stdout(mut self, file: File) -> Self {
        self.streams[1] = Stream::File(file);
        self
    }
    pub fn with_stderr(mut self, file: File) -> Self {
        self.streams[2] = Stream::File(file);
        self
    }

    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            streams: [
                self.streams[0].try_clone()?,
                self.streams[1].try_clone()?,
                self.streams[2].try_clone()?,
            ],
            stderr_redirected: self.stderr_redirected,
        })
    }

    pub(crate) fn set(&mut self, descriptor: u8, stream: Stream) -> io::Result<()> {
        let slot = self
            .streams
            .get_mut(usize::from(descriptor))
            .ok_or_else(bad_descriptor)?;
        *slot = stream;
        if descriptor == 2 {
            self.stderr_redirected = true;
        }
        Ok(())
    }

    pub(crate) fn duplicate(&mut self, destination: u8, source: u8) -> io::Result<()> {
        let original = self
            .streams
            .get(usize::from(source))
            .ok_or_else(bad_descriptor)?;
        if matches!(original, Stream::Closed) {
            return Err(bad_descriptor());
        }
        let stream = original.try_clone()?;
        self.set(destination, stream)
    }

    pub(crate) fn out(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        self.streams[1].write_fmt(args)
    }
    pub(crate) fn output(&mut self) -> &mut dyn Write {
        &mut self.streams[1]
    }
    pub(crate) fn inherits_console_input(&self) -> bool {
        self.input_inherited() && io::stdin().is_terminal()
    }
    pub(crate) fn input_inherited(&self) -> bool {
        matches!(self.streams[0], Stream::Inherited(0))
    }
    pub(crate) fn input_closed(&self) -> bool {
        matches!(self.streams[0], Stream::Closed)
    }
    pub(crate) fn input_file(&self) -> io::Result<File> {
        match &self.streams[0] {
            Stream::Inherited(fd) => inherited_file(*fd),
            Stream::File(file) => file.try_clone(),
            Stream::Reader(pipe) => Ok(File::from(OwnedIo::from(pipe.try_clone()?))),
            Stream::Writer(pipe) => Ok(File::from(OwnedIo::from(pipe.try_clone()?))),
            _ => Err(bad_descriptor()),
        }
    }
    pub(crate) fn output_file(&self) -> io::Result<Option<File>> {
        match &self.streams[1] {
            Stream::File(file) => file.try_clone().map(Some),
            Stream::Inherited(fd) => inherited_file(*fd).map(Some),
            _ => Ok(None),
        }
    }
    pub(crate) fn read_byte(&mut self) -> io::Result<Option<u8>> {
        let mut byte = [0];
        let count = match &mut self.streams[0] {
            Stream::File(file) => file.read(&mut byte)?,
            Stream::Reader(reader) => reader.read(&mut byte)?,
            Stream::Inherited(fd) => inherited_file(*fd)?.read(&mut byte)?,
            _ => return Err(bad_descriptor()),
        };
        Ok(if count == 0 { None } else { Some(byte[0]) })
    }
    pub(crate) fn err(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        match self.streams[2].write_fmt(args) {
            // A command can close stderr or redirect it to an unwritable stream.
            // Preserve its status while still reporting failures on host stderr.
            Err(_) if self.stderr_redirected => Ok(()),
            result => result,
        }
    }

    pub(crate) fn configure(&self, command: &mut Command) -> io::Result<()> {
        command
            .stdin(self.streams[0].stdio(0)?)
            .stdout(self.streams[1].stdio(1)?)
            .stderr(self.streams[2].stdio(2)?);
        Ok(())
    }
}

impl Stream {
    fn try_clone(&self) -> io::Result<Self> {
        Ok(match self {
            Self::Closed => Self::Closed,
            Self::Inherited(descriptor) => Self::Inherited(*descriptor),
            Self::File(file) => Self::File(file.try_clone()?),
            Self::Reader(pipe) => Self::Reader(pipe.try_clone()?),
            Self::Writer(pipe) => Self::Writer(pipe.try_clone()?),
        })
    }

    fn stdio(&self, destination: u8) -> io::Result<Stdio> {
        Ok(match self {
            Self::Closed => {
                // Command requires valid owned handles. Give a closed input a
                // write-only handle, and closed outputs a read-only handle.
                // Native reads/writes fail instead of silently using NUL or
                // accidentally inheriting the host's original stream. No
                // opposite endpoint survives to hold another pipeline open.
                let (reader, writer) = io::pipe()?;
                if destination == 0 {
                    drop(reader);
                    writer.into()
                } else {
                    drop(writer);
                    reader.into()
                }
            }
            Self::Inherited(source) if *source == destination => Stdio::inherit(),
            Self::Inherited(source) => inherited_file(*source)?.into(),
            Self::File(file) => file.try_clone()?.into(),
            Self::Reader(pipe) => pipe.try_clone()?.into(),
            Self::Writer(pipe) => pipe.try_clone()?.into(),
        })
    }
}

impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Inherited(1) => io::stdout().write(bytes),
            Self::Inherited(2) => io::stderr().write(bytes),
            Self::Inherited(fd) => inherited_file(*fd)?.write(bytes),
            Self::File(file) => file.write(bytes),
            Self::Writer(pipe) => pipe.write(bytes),
            Self::Closed | Self::Reader(_) => Err(bad_descriptor()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Inherited(1) => io::stdout().flush(),
            Self::Inherited(2) => io::stderr().flush(),
            _ => Ok(()),
        }
    }

    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        match self {
            Self::Closed => Err(bad_descriptor()),
            Self::Inherited(1) => io::stdout().write_fmt(args),
            Self::Inherited(2) => io::stderr().write_fmt(args),
            Self::Inherited(source) => inherited_file(*source)?.write_fmt(args),
            Self::File(file) => file.write_fmt(args),
            Self::Writer(pipe) => pipe.write_fmt(args),
            Self::Reader(_) => Err(bad_descriptor()),
        }
    }
}

fn bad_descriptor() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "bad file descriptor (supported: 0, 1, 2)",
    )
}

fn inherited_file(descriptor: u8) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        let handle = match descriptor {
            0 => io::stdin().as_handle().try_clone_to_owned()?,
            1 => io::stdout().as_handle().try_clone_to_owned()?,
            2 => io::stderr().as_handle().try_clone_to_owned()?,
            _ => return Err(bad_descriptor()),
        };
        Ok(handle.into())
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        let descriptor = match descriptor {
            0 => io::stdin().as_fd().try_clone_to_owned()?,
            1 => io::stdout().as_fd().try_clone_to_owned()?,
            2 => io::stderr().as_fd().try_clone_to_owned()?,
            _ => return Err(bad_descriptor()),
        };
        Ok(descriptor.into())
    }
}
