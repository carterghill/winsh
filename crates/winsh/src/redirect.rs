use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
};

use crate::{
    Outcome, Shell, ShellIo, expand, platform,
    streams::Stream,
    syntax::{self, Redirection, RedirectionKind},
};

/// Apply in source order to stage-local streams. Earlier stderr redirections
/// also govern diagnostics if a later open or descriptor duplication fails.
pub(crate) fn apply(
    shell: &mut Shell,
    redirects: &[Redirection],
    streams: &mut ShellIo,
) -> io::Result<Option<Outcome>> {
    for redirect in redirects {
        let result = if let Some(doc) = &redirect.here_document {
            let body = if doc.quoted {
                doc.body.as_str().into()
            } else {
                let expanded = syntax::parse_document_body(&doc.body)
                    .map_err(|e| expand::error(&redirect.target, e.to_string()))
                    .and_then(|word| expand::scalar(&word, shell, streams));
                match expanded {
                    Ok(value) => std::borrow::Cow::Owned(value.to_string_lossy().into_owned()),
                    Err(error) => return shell.expansion_failure(error, streams).map(Some),
                }
            };
            document_input(&body)
                .and_then(|file| streams.set(redirect.descriptor.unwrap_or(0), Stream::File(file)))
                .map_err(|error| format!("here-document: {error}"))
        } else {
            let fields = match expand::fields(&redirect.target, shell, streams, true) {
                Ok(fields) => fields,
                Err(error) => return shell.expansion_failure(error, streams).map(Some),
            };
            let [target] = fields.as_slice() else {
                return shell
                    .expansion_failure(
                        expand::error(&redirect.target, "ambiguous redirect"),
                        streams,
                    )
                    .map(Some);
            };
            one(shell, redirect, target, streams)
                .map_err(|error| format!("{}: {error}", target.to_string_lossy()))
        };
        if let Err(error) = result {
            streams.err(format_args!("winsh: {error}\n"))?;
            return Ok(Some(Outcome::Continue(1)));
        }
    }
    Ok(None)
}

fn document_input(body: &str) -> io::Result<File> {
    // A large document must be ready before the consumer starts, even if it
    // never reads stdin. Pass it an independent, read-only file handle.
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(body.as_bytes())?;
    File::open(file.path())
}

fn one(
    shell: &Shell,
    redirect: &Redirection,
    target: &OsStr,
    streams: &mut ShellIo,
) -> io::Result<()> {
    use RedirectionKind::*;
    let default = if matches!(redirect.kind, Input | ReadWrite | DuplicateInput) {
        0
    } else {
        1
    };
    let descriptor = redirect.descriptor.unwrap_or(default);
    if matches!(redirect.kind, DuplicateInput | DuplicateOutput) {
        if let Some(text) = target.to_str() {
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
    let path = platform::absolute(shell.cwd(), Path::new(target))?;
    let mut options = OpenOptions::new();
    match redirect.kind {
        HereDocument | HereDocumentStripTabs => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing here-document body",
            ));
        }
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
