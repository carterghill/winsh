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
`TERM=xterm-256color` so programs such as Nano can look up the terminal's
colors and keyboard sequences in their terminfo database.

## Automatic rebuilds

Run `.\dev.cmd` from PowerShell at the repo root to start a dedicated development
window. Its first prompt shows `winsh dev build #1` and the local time. The
launching PowerShell shows compilation progress, diagnostics, and the terminal
process ID after each launch.

The watcher checks crate source and asset directories, build scripts, Cargo manifests,
`Cargo.lock`, the Rust toolchain file, and workspace Cargo configuration.
It builds both binaries under `target\watch` and runs private copies so Windows
can keep the current session open during compilation. A build error leaves that
session available. The next edit retries the build.

When a successful build changes either executable, the watcher opens a new
session at the repo root and closes its previous window and foreground programs.
Other winsh windows keep running their original binaries. Type `exit` in the
last tab, close the window, or press Ctrl+C in the watching PowerShell to stop.

## Other shells and arguments

To host another executable or pass shell arguments:

~~~powershell
cargo run -p winsh-terminal -- --shell C:/Windows/System32/cmd.exe -- /d
~~~

`--shell` takes an executable path. Arguments after `--` are passed individually
using Windows argument quoting. `--help` displays a controls and usage dialog.

## Controls and display

Each title-bar tab hosts a separate shell with its own screen and scrollback.
New tabs start with the launcher's working directory, executable, and arguments.
Each winsh tab shows the name of its current folder and updates when you change
directories. At a drive root, the tab shows the drive (for example, `C:\`).
Click a tab to switch sessions. When the strip fills up, the chevrons reveal
more tabs on either side. Drag the space beside the tabs to move the window.

| Default input | Behavior |
| --- | --- |
| +, Ctrl+T, or Ctrl+Shift+T | Open a tab. |
| Ctrl+Left / Ctrl+Right | Switch to the previous / next tab, wrapping at the ends. Ctrl+Shift+Tab / Ctrl+Tab also work. |
| Alt+Left / Alt+Right | Move one word left / right at the winsh prompt. |
| Tab's × or Ctrl+Shift+W | Close that tab or the active tab, respectively. Closing the last tab closes the window. |
| Text, Enter, Backspace | Console input and editing supplied by the hosted program. |
| Shift+Enter | Insert a newline at the winsh prompt or in the Codex composer. Plain Enter still submits. |
| Ctrl+letter | Send the shortcut to the hosted program unless Winsh has a binding for it. In Codex, Ctrl+J inserts a newline. In Nano, Ctrl+X exits and prompts to save a modified buffer. |
| Arrow keys, Home/End, Insert/Delete, F1–F12 | Forward the key and its modifiers to the hosted program. In VT mode, arrow keys also follow the application's cursor mode. |
| Ctrl+C | Send an interrupt through ConPTY; winsh keeps the session open. |
| Ctrl+Z | Stop a foreground winsh job. At the shell prompt, send Windows end-of-input. |
| Ctrl+Shift+V or Shift+Insert | Paste Unicode clipboard text; bracketed paste when requested by the application. |
| Ctrl+Shift+C | Copy the visible screen as plain text. |
| Mouse wheel or Shift+PageUp/PageDown | Browse up to 5,000 lines of scrollback. Typing returns to the live screen. |
| Resize window | Resize the grid and hosted console in every tab. |
| `exit` | End the current shell and close its tab. |
| Close window | End all sessions in the window. |

The terminal renders a cell grid with cursor movement, erasing, scrolling,
alternate screens, cursor visibility, ANSI/256/RGB colors, bold, italic,
underline, and inverse text. UTF-8 output can arrive across arbitrary read
boundaries; Windows text input preserves UTF-16 surrogate pairs. Wide and
combining characters retain their terminal cell positions. Window title and
basic device/status/cursor queries are supported.

GDI renders Cascadia Mono by default with a Windows fallback and per-monitor
DPI scaling. For private-use icon characters, the terminal loads
Symbols Nerd Font Mono 3.4.0 from its executable. Icons retain their natural
proportions. The terminal aligns the visible strokes of each icon with the
surrounding text. Icons with blank padding on both sides in a colored prompt
segment draw slightly larger. The Windows badge sits slightly below the
geometric center, and the open folder sits closer to the path. Directory and
Git labels share a baseline that centers capital letters within each segment.
`ls --icons` prints two spaces after each icon so the icon
can extend into the first and leave a full cell clear before the filename.
An icon next to text fits within its own cell. This works for `ls` and
other programs that emit Nerd Font icons. In Windows Terminal, select a Nerd
Font in the Winsh profile's Appearance settings. [Bundled font and license](../crates/winsh-terminal/assets)

The screen is double buffered. Keyboard input and
protocol replies share an input worker; the output worker parses directly into
the screen model. ConPTY control and shutdown run independently of the window.
Closing a session terminates its shell and descendants through a job object,
and drains output while releasing ConPTY to avoid pipe deadlocks.

## Keyboard shortcuts

The dedicated terminal creates `%APPDATA%\winsh\keymap.json` beside
`settings.json`. Each entry maps one key combination to an action. For
example, change `"ctrl+right": "next_tab"` to `"ctrl+shift+right": "next_tab"` and save.
Ctrl+Shift+Right will switch tabs immediately in open windows. Deleting a binding
returns that combination to normal terminal input. An invalid edit keeps the
last working bindings active. [Default keymap and supported actions](../crates/winsh-terminal/assets/keymap.json)

When ConPTY requests native keyboard input, Winsh forwards the virtual key,
scan code, Unicode character, modifiers, press or release, and repeat count.
This lets applications distinguish Ctrl+J from Enter and Ctrl+I from Tab.
Older ConPTY versions use terminal byte sequences, where some key combinations
share a representation. [Microsoft's ConPTY keyboard protocol](https://github.com/microsoft/terminal/blob/main/doc/specs/%234999%20-%20Improved%20keyboard%20handling%20in%20Conpty.md)

Key names are case-insensitive. Combine `ctrl` (or `control`), `alt`, and
`shift` in any order before the key. Supported keys are letters A–Z, digits
0–9, F1–F12, the four arrows, Home, End, PageUp, PageDown, Tab, Insert,
Delete, Backspace, Enter, Space, and Escape. Word actions send Alt+Left or
Alt+Right to the hosted program, which handles movement in its own editor.

## Font settings

On first launch the dedicated terminal creates `%APPDATA%\winsh\settings.json`
beside `winshrc`. Save an edit to update the text and icon sizes in all open
tabs. Each shell session keeps running.

~~~json
{
  "terminal": {
    "font_family": "Cascadia Mono",
    "font_size_px": 14
  }
}
~~~

Choose an installed monospaced font for `font_family`. The size is in logical
pixels at 100% Windows scaling and accepts values from 1 to 256, including
fractions. The default of 14 logical pixels is 10.5 points. A size of 16
reproduces the previous 12-point text. Omitted fields
use their defaults. If an edit is invalid, the terminal reports the error and
continues using the last valid settings.

## Current limits

The terminal has no settings UI,
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
The keyboard, console, and descendant subprocess fixtures run as part of
those tests.

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
