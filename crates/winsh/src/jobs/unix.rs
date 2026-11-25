#![allow(unsafe_code)]
use super::State;
use std::{
    io,
    os::unix::process::{CommandExt, ExitStatusExt},
    process::{Child, Command, ExitStatus},
};

pub(super) fn configure(cmd: &mut Command) {
    cmd.process_group(0);
}
pub(super) type InputRelay = std::process::ChildStdin;
pub(super) fn input_relay(pipe: InputRelay) -> io::Result<InputRelay> {
    Ok(pipe)
}
pub(super) struct Group(i32);
impl Group {
    pub(super) fn attach(child: &Child) -> io::Result<Self> {
        Ok(Self(child.id() as i32))
    }
    fn signal(&self, sig: i32) -> io::Result<()> {
        // SAFETY: this group is rooted at the still-owned worker process.
        if unsafe { libc::kill(-self.0, sig) } == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    pub(super) fn freeze(&self, stopped: bool) -> io::Result<()> {
        self.signal(if stopped {
            libc::SIGSTOP
        } else {
            libc::SIGCONT
        })
    }
    pub(super) fn terminate(&self, status: i32) -> io::Result<()> {
        self.signal(status - 128)
    }
    pub(super) fn poll(&self, _: &mut Child) -> io::Result<Option<State>> {
        let mut status = 0;
        // SAFETY: wait only on our own child; status is a writable integer.
        let pid = unsafe {
            libc::waitpid(
                self.0,
                &mut status,
                libc::WNOHANG | libc::WUNTRACED | libc::WCONTINUED,
            )
        };
        if pid == 0 {
            return Ok(None);
        }
        if pid < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::Interrupted {
                Ok(None)
            } else {
                Err(error)
            };
        }
        Ok(Some(if libc::WIFSTOPPED(status) {
            State::Stopped
        } else if libc::WIFCONTINUED(status) {
            State::Running
        } else {
            State::Done(crate::platform::exit_status(ExitStatus::from_raw(status)))
        }))
    }
}
pub(super) struct Foreground(Option<i32>);
impl Foreground {
    pub(super) fn enter(pid: u32) -> io::Result<Self> {
        // SAFETY: fd 0 is borrowed; non-tty input simply has no foreground owner.
        let prev = unsafe { libc::tcgetpgrp(0) };
        if prev < 0 || prev != unsafe { libc::getpgrp() } {
            return Ok(Self(None));
        }
        set_foreground(pid as i32)?;
        Ok(Self(Some(prev)))
    }
    pub(super) fn stop_requested(&self) -> bool {
        false
    }
    pub(super) fn forward_input(&mut self, _: &mut std::process::ChildStdin) -> io::Result<bool> {
        Ok(true)
    }
}

pub(super) fn terminal_input() -> io::Result<std::fs::File> {
    std::fs::File::open("/dev/tty")
}
pub(super) fn prepare_input(cmd: &mut Command, file: &std::fs::File) {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    // SAFETY: fcntl is async-signal-safe. Only the child copy loses CLOEXEC.
    unsafe {
        cmd.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}
pub(super) fn transfer_input(_: &Child, file: &std::fs::File) -> io::Result<u64> {
    use std::os::fd::AsRawFd;
    Ok(file.as_raw_fd() as u64)
}
pub(super) fn received_input(fd: u64) -> io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    let fd = i32::try_from(fd)
        .ok()
        .filter(|&fd| fd >= 3)
        .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?;
    // The original crosses only the bootstrap exec, never later native execs.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl validates the fd and returns a uniquely owned duplicate.
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if dup == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { std::fs::File::from_raw_fd(dup) })
    }
}
impl Drop for Foreground {
    fn drop(&mut self) {
        if let Some(prev) = self.0 {
            let _ = set_foreground(prev);
        }
    }
}
fn set_foreground(group: i32) -> io::Result<()> {
    // SAFETY: restoring the terminal from a background group requires blocking
    // SIGTTOU on this thread only. Restore its original mask on both paths.
    unsafe {
        let mut mask = std::mem::zeroed();
        let mut old = std::mem::zeroed();
        libc::sigemptyset(&mut mask);
        libc::sigaddset(&mut mask, libc::SIGTTOU);
        let err = libc::pthread_sigmask(libc::SIG_BLOCK, &mask, &mut old);
        if err != 0 {
            return Err(io::Error::from_raw_os_error(err));
        }
        let result = if libc::tcsetpgrp(0, group) == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        };
        libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
        result
    }
}
