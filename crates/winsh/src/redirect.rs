use std::{ffi::OsString, fs::OpenOptions, io, path::Path};

use crate::{Shell, ShellIo, platform, streams::Stream, syntax::RedirectionKind};

pub(crate) struct Redirect {
    pub descriptor: Option<u8>,
    pub kind: RedirectionKind,
    pub target: OsString,
}

/// Apply in source order to stage-local streams. Earlier stderr redirections
/// also govern diagnostics if a later open or descriptor duplication fails.
pub(crate) fn apply(
    shell: &Shell,
    redirects: &[Redirect],
    streams: &mut ShellIo,
) -> io::Result<bool> {
    for redirect in redirects {
        if let Err(error) = one(shell, redirect, streams) {
            streams.err(format_args!(
                "winsh: {}: {error}\n",
                redirect.target.to_string_lossy()
            ))?;
            return Ok(false);
        }
    }
    Ok(true)
}

fn one(shell: &Shell, redirect: &Redirect, streams: &mut ShellIo) -> io::Result<()> {
    use RedirectionKind::*;
    let default = if matches!(redirect.kind, Input | ReadWrite | DuplicateInput) {
        0
    } else {
        1
    };
    let descriptor = redirect.descriptor.unwrap_or(default);
    if matches!(redirect.kind, DuplicateInput | DuplicateOutput) {
        if let Some(text) = redirect.target.to_str() {
            if text == "-" {
                return streams.set(descriptor, Stream::Closed);
            }
            let (number, moving) = text
                .strip_suffix('-')
                .map_or((text, false), |number| (number, true));
            if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) {
                let source = number
                    .parse::<u8>()
                    .ok()
                    .filter(|fd| *fd <= 2)
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "bad file descriptor (supported: 0, 1, 2)",
                        )
                    })?;
                streams.duplicate(descriptor, source)?;
                if moving {
                    streams.set(source, Stream::Closed)?;
                }
                return Ok(());
            }
        }
        // Bash's >&file shorthand redirects both output streams. With an
        // explicit descriptor, and for <&, the operand must be a descriptor.
        if redirect.kind != DuplicateOutput || redirect.descriptor.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "bad file descriptor",
            ));
        }
    }
    let path = platform::absolute(shell.cwd(), Path::new(&redirect.target))?;
    let mut options = OpenOptions::new();
    match redirect.kind {
        Input => {
            options.read(true);
        }
        ReadWrite => {
            options.read(true).write(true).create(true);
        }
        Append | AppendAndError => {
            options.append(true).create(true);
        }
        _ => {
            options.write(true).create(true).truncate(true);
        }
    }
    let file = options.open(path)?;
    streams.set(descriptor, Stream::File(file))?;
    if matches!(
        redirect.kind,
        OutputAndError | AppendAndError | DuplicateOutput
    ) {
        streams.duplicate(2, descriptor)?;
    }
    Ok(())
}
