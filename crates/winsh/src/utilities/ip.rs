use std::{
    ffi::OsString,
    io::{self, Read, Write},
    net::IpAddr,
};

use crate::{Shell, ShellIo, exec, platform, streams::Stream};

const USAGE: &str = "Usage: ip [--local]";

pub(super) fn run(shell: &Shell, args: &[OsString], io: &mut ShellIo) -> io::Result<i32> {
    let local_only = match args {
        [] => false,
        [arg] if arg == "--local" => true,
        [arg] if arg == "--help" || arg == "-h" => {
            io.out(format_args!("{USAGE}\n"))?;
            return Ok(0);
        }
        _ => {
            io.err(format_args!("{USAGE}\n"))?;
            return Ok(2);
        }
    };
    let bold = !local_only
        && io
            .output_file()?
            .as_ref()
            .is_some_and(platform::enable_virtual_terminal_output);
    let mut status = 0;
    match platform::addresses::local() {
        Ok(ips) if !ips.is_empty() => {
            if local_only {
                let line = ips
                    .iter()
                    .map(IpAddr::to_string)
                    .collect::<Vec<_>>()
                    .join(" ");
                io.out(format_args!("{line}\n"))?;
            } else {
                for ip in ips {
                    write_address(io.output(), "Local", ip, bold)?;
                }
            }
        }
        Ok(_) => {
            io.err(format_args!("ip: no active local addresses\n"))?;
            status = 1;
        }
        Err(error) => {
            io.err(format_args!("ip: local address lookup failed: {error}\n"))?;
            status = 1;
        }
    }
    if !local_only {
        match public_ip(shell, io) {
            Ok(ip) => write_address(io.output(), "Public", ip, bold)?,
            Err(error) => {
                io.err(format_args!("ip: public address lookup failed: {error}\n"))?;
                status = 1;
            }
        }
    }
    Ok(status)
}

fn write_address(out: &mut dyn Write, scope: &str, ip: IpAddr, bold: bool) -> io::Result<()> {
    let family = if ip.is_ipv4() { "IPv4" } else { "IPv6" };
    let (start, end) = if bold {
        ("\x1b[1m", "\x1b[22m")
    } else {
        ("", "")
    };
    writeln!(out, "{start}{scope} {family}:{end} {ip}")
}

fn public_ip(shell: &Shell, io: &ShellIo) -> io::Result<IpAddr> {
    let args = [
        "curl.exe",
        "--disable",
        "--silent",
        "--show-error",
        "--fail",
        "--max-time",
        "5",
        "--max-filesize",
        "64",
        "https://api64.ipify.org",
    ]
    .map(OsString::from);
    let (reader, writer) = io::pipe()?;
    let process = {
        let mut streams = io.try_clone()?;
        streams.set(1, Stream::Writer(writer))?;
        exec::spawn(shell, &args, &mut streams)?
    };
    let mut bytes = Vec::new();
    reader.take(65).read_to_end(&mut bytes)?;
    let status = exec::wait(shell, process)?;
    if status != 0 {
        return Err(io::Error::other(format!(
            "curl.exe exited with status {status}"
        )));
    }
    std::str::from_utf8(&bytes)
        .ok()
        .filter(|s| s.len() <= 64)
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "ipify returned an invalid IP address",
            )
        })
}
