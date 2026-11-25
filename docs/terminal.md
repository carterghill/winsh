# Dedicated Windows terminal

`winsh-terminal` is a native Win32 window that hosts a console executable through
Windows ConPTY. It requires Windows 10 version 1809 or later. Shell language and
navigation still belong to `winsh`; the terminal can also host other shells.

## Run it

From the repository root:

~~~powershell
cargo build --workspace
cargo run -p winsh-terminal
~~~

`cargo run` alone continues to run winsh in your existing terminal. Build both
packages again after changing shell code; running only the terminal package
does not rebuild the separate shell executable. Close running executables
before rebuilding them if Windows reports that an executable is locked.

For release builds:

~~~powershell
cargo build --workspace --release
.\target\release\winsh-terminal.exe
~~~

Keep `winsh-terminal.exe` and `winsh.exe` together. The launcher resolves the
default shell beside its own executable, independently of the working directory.
The shell inherits the launcher's working directory and environment, with
`TERM=vt100` identifying the dedicated terminal to interactive programs.

## Automatic rebuilds

Run `.\dev.cmd` from PowerShell at the repo root to start a dedicated development
window. Its first prompt shows `winsh dev build #1` and the local time. The
launching PowerShell shows compilation progress, diagnostics, and the terminal
process ID after each launch.

The watcher checks crate source directories, build scripts, Cargo manifests,
`Cargo.lock`, the Rust toolchain file, and workspace Cargo configuration.
It builds both binaries under `target\watch` and runs private copies so Windows
can keep the current session open during compilation. A build error leaves that
session available. The next edit retries the build.

When a successful build changes either executable, the watcher opens a new
session at the repo root and closes its previous window and foreground programs.
Other winsh windows keep running their original binaries. Type `exit` in the
watched window, close it, or press Ctrl+C in the watching PowerShell to stop.

## Other shells and arguments

To host another executable or pass shell arguments:

~~~powershell
cargo run -p winsh-terminal -- --shell C:/Windows/System32/cmd.exe -- /d
~~~

`--shell` takes an executable path. Arguments after `--` are passed individually
using Windows argument quoting. `--help` displays a controls and usage dialog.

## Controls and display

| Input | Behavior |
| --- | --- |
| Text, Enter, Backspace | Console input and editing supplied by the hosted program. |
| Arrow keys, Home/End, Insert/Delete, F1–F12 | Terminal key sequences, including application cursor mode and modifiers. |
| Ctrl+C | Send an interrupt through ConPTY; winsh keeps the session open. |
| Ctrl+Z | Stop a foreground winsh job. At the shell prompt, send Windows end-of-input. |
| Ctrl+Shift+V or Shift+Insert | Paste Unicode clipboard text; bracketed paste when requested by the application. |
| Ctrl+Shift+C | Copy the visible screen as plain text. |
| Mouse wheel or Shift+PageUp/PageDown | Browse up to 5,000 lines of scrollback. Typing returns to the live screen. |
| Resize window | Resize both the terminal grid and the hosted console. |
| `exit` or close window | End the session and close the window. |

The terminal renders a cell grid with cursor movement, erasing, scrolling,
alternate screens, cursor visibility, ANSI/256/RGB colors, bold, italic,
underline, and inverse text. UTF-8 output can arrive across arbitrary read
boundaries; Windows text input preserves UTF-16 surrogate pairs. Wide and
combining characters retain their terminal cell positions. Window title and
basic device/status/cursor queries are supported.

GDI renders a fixed 12-point Cascadia Mono font with a Windows fallback and
per-monitor DPI updates. The screen is double buffered. Keyboard input and
protocol replies share an input worker; the output worker parses directly into
the screen model. ConPTY control and shutdown run independently of the window.
Closing a session terminates its shell and descendants through a job object,
and drains output while releasing ConPTY to avoid pipe deadlocks.

## Current limits

This milestone has one window and one session. It has no tabs, settings UI,
mouse selection, terminal mouse reporting, search, hyperlinks, font zoom,
graphics protocols, or saved sessions. Copying copies the whole visible screen.
Font fallback and complex emoji shaping are limited by GDI. Width changes
truncate or extend existing rows rather than reflowing old text. Scrollback
has no scrollbar. This is a basic VT terminal, not full xterm compatibility.

The hosted program owns command-line editing. Winsh supports persistent history
and Tab completion in this window. Winsh owns the job table and the lifecycle
of background pipelines. The terminal forwards Ctrl+Z to a foreground job
through a session-specific event. Pipelines and redirections
are implemented by winsh and work inside this window; see [the shell guide](shell.md#redirections-and-pipelines).
The terminal itself adds no language rules or script-file support.

## Validation

~~~powershell
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
~~~

Tests cover cell rendering state, escape/UTF-8 fragmentation, colors, alternate
screens, scrollback, queries, input encoding, actual ConPTY input/output,
resizing as observed by a console child, startup failure, and descendant cleanup.
Two ignored test entries are subprocess fixtures used by those integration
tests, not additional manual checks.

The opt-in shell tests cover suspending and resuming jobs, foreground input,
Ctrl+C isolation, Tab completion, persistent command recall across terminal restarts,
canceling multiline input, interrupting foreground programs and a native
pipeline, and preserving the shell's exit status.

~~~powershell
cargo build --workspace
cargo test -p winsh-terminal --test conpty winsh_ -- --ignored
~~~

Use the same `--target-dir` for both commands if you override Cargo's output
directory. Window rendering, clipboard shortcuts, and DPI changes also need
interactive checks.

Implementation references: [Microsoft's ConPTY session guide](https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session),
[ConPTY shutdown requirements](https://learn.microsoft.com/en-us/windows/console/closepseudoconsole),
and [VT parser documentation](https://docs.rs/vt100/0.16.2/vt100/).
