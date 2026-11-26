# winsh

**win + sh**, also **Windows shell**: a native Windows shell written in Rust.

The goal is Bash-compatible syntax for navigation and scripting, native Windows
program execution, and compatible utilities such as cat and grep. The dedicated
Windows terminal hosts the shell through ConPTY and stays separate from the
shell engine.

The working shell supports interactive input, `.sh` scripts, variables and
expansions, command lists, branches, loops, and functions, along with directory
navigation, file redirections, streaming pipelines, background jobs, and job
control. See [background jobs and job control](docs/shell.md#background-jobs-and-job-control)
for a Windows example.
It implements a documented subset of Bash syntax on Windows.

## Try it

For development on Windows, start the watched terminal from PowerShell at the repo root.

~~~powershell
.\dev.cmd
~~~

Save a Rust source file to rebuild the shell and terminal. PowerShell shows
Cargo's progress and errors. When the binaries change, the watcher opens a fresh
terminal with a timestamped build number above the prompt. Type `exit` in the
dev terminal to stop watching. [Development terminal details](docs/terminal.md#automatic-rebuilds)

Start winsh inside your current terminal:

~~~powershell
cargo run
~~~

Then enter commands in winsh:

~~~sh
pwd
ls
cat Cargo.toml
cd crates/winsh/src
pwd
cd -
cd ~
rustc --version
exit
~~~

Use forward slashes in paths, or single-quote Windows paths containing
backslashes: `cd 'C:\Program Files'`. Native programs on PATH run by name;
use `./program.exe` for a program in the current directory.

On Windows, install Python 3 from the winsh prompt using WinGet, the package
manager that ships with App Installer.

~~~sh
pkg install python3
python3 --version
~~~

Install other tools by name, for example `pkg install lazygit` or
`pkg install fzf`. If WinGet lists several matches, use the ID from the
desired row, for example `pkg install Git.Git`. Windows prompts for
administrator access when an installer needs it.
[Installing Windows packages](docs/shell.md#installing-windows-packages)

In winsh on Windows, run `ip` to see your local and public IP addresses.
Use `ip --local` for a space-separated list of local addresses, like
`hostname -I`. [IP address lookup](docs/shell.md#ip-address-lookup)

From PowerShell, execute one command or inspect the available options:

~~~powershell
cargo run -- -c 'pwd'
cargo run -- examples/greet.sh Carter 'another name'
cargo run -- --help
cargo run -- --version
~~~

Inside winsh, redirections and pipelines work for built-ins and native programs:

~~~sh
pwd >location.txt
pwd >>location.txt
sort.exe <location.txt | findstr.exe winsh >matches.txt
cat README.md | grep -in 'windows'
program.exe >output.txt 2>errors.txt
program.exe >combined.txt 2>&1
program.exe |& findstr.exe error
~~~

See [redirections and pipelines](docs/shell.md#redirections-and-pipelines) for
descriptor handling, exit statuses, and the supported subset.

`ls`, `cat`, `grep`, `mkdir`, `touch`, `cp`, `mv`, and `rm` ship inside
`winsh.exe`. Use them at the prompt or in a script, or run
`cargo run -- --utility grep -in windows README.md` from PowerShell.
See the [utility options and exit statuses](docs/shell.md#file-utilities).

Interactive `ls` shows colored names and folder icons in a directories-first
grid. `ls -1` gives one entry per line, and `ls >files.txt` writes plain names.
Use `alias ls` to inspect the default, or customize it in
`%APPDATA%\winsh\winshrc`. [Aliases and startup configuration](docs/shell.md#aliases-and-startup-configuration)

The first interactive launch creates that file with a Powerline-style prompt
showing the OS icon, current directory, and Git branch. The original full-path
`$` prompt is a commented option in the same file.
You can use `PROMPT` or `PS1` to customize it. [Custom prompts](docs/shell.md#custom-prompts)

Scripts use the same engine as the prompt. For example:

~~~sh
greet() { printf 'Hello, %s!\n' "$1"; }
for name in Carter 'another name'; do
    greet "$name"
done
~~~

See [scripts and language](docs/shell.md#scripts-and-language) for variables,
arguments, control flow, functions, built-ins, and compatibility limits.

To open the dedicated terminal on Windows:

~~~powershell
cargo build --workspace
cargo run -p winsh-terminal
~~~

The terminal finds `winsh.exe` beside its own executable. It supports a resizable
window, Unicode, cursor movement, colors, alternate screens, and scrollback.
Use Ctrl+Shift+V to paste, Ctrl+Shift+C to copy the visible screen, and the mouse
wheel or Shift+PageUp/PageDown to scroll. `exit` closes the window. See
[the terminal guide](docs/terminal.md) for controls, other shells, and limits.

See [the shell guide](docs/shell.md) for current limits and a walkthrough of the
Rust code, and [the syntax guide](docs/syntax.md) for the parser API.
Inspect parsed syntax without executing it using the development example:

~~~powershell
cargo run --example parse -- 'echo "hello world" one\ two'
~~~

## Development

On Windows, install Rust through [rustup](https://rustup.rs/) with the MSVC host
toolchain, plus the Visual Studio C++ build tools and Windows SDK needed for
linking. See the [Rust installation guide](https://www.rust-lang.org/tools/install).

The development toolchain follows **stable**, with `rustfmt`, Clippy, and
`rust-src` for editor analysis. Rustup selects it from `rust-toolchain.toml`; it
may download the toolchain on a new machine. Keep it current with:

~~~powershell
rustup update stable
~~~

`package.rust-version = "1.88"` records the minimum compiler supported by the
code, separately from the development toolchain. Recent rust-analyzer versions
can require newer Rust even when older compilers build the code successfully.
See [rust-analyzer's toolchain guidance](https://rust-analyzer.github.io/book/installation.html#rust-standard-library).
If the editor reports an unsupported toolchain or cannot resolve standard macros
such as `matches!` and `write!`, update stable and restart the language server or
reopen the project. In Zed, ensure the selected project toolchain is `stable`.

~~~powershell
cargo build --workspace --release
~~~

The release executables are `target/release/winsh.exe` and
`target/release/winsh-terminal.exe` on Windows. Keep them together.

Before committing changes:

~~~powershell
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
~~~

The shell uses `regex` for grep patterns, `walkdir` for directory traversal,
`same-file` to identify aliased paths, and `tempfile` to stage cross-drive moves.
On Windows, `windows-sys` handles console input,
Ctrl+C and suppressing OS error dialogs during process launch. The
terminal uses `windows-sys` for its native window, GDI rendering, and ConPTY,
and `vt100` for terminal parsing and screen state.

To exercise `mv` across two real filesystems, point `WINSH_TEST_OTHER_VOLUME`
at a writable directory on a different volume from Cargo's target directory.
Run `cargo test --locked -p winsh --test shell mv_between_real_filesystems -- --ignored`.
The ordinary suite also exercises the copy and cleanup paths on a single volume.

Tests cover parsing, session state, directory links, exact native arguments,
Unicode, environment, streaming I/O, and exit statuses. Windows link tests use
the system Windows PowerShell to create directory junctions without requiring
administrator privileges. An optional [Bash comparison test](docs/syntax.md#tests)
checks literal quoting fixtures against a local Bash installation. A second
comparison checks redirection ordering and pipeline behavior, and a third checks
script language fixtures. Terminal
tests cover screen behavior, input encoding, real ConPTY I/O, resizing, and
process-tree cleanup.

Keep `Cargo.lock` in source control. Once the toolchain and dependencies are
cached, build without network access using
`cargo build --workspace --locked --offline`.

## Layout

~~~text
crates/winsh/
  src/main.rs        Shell CLI, input loop, and process exit
  src/console.rs     Console input and Windows Ctrl+C handling
  src/lib.rs         Public engine interface
  src/syntax/        Script grammar, words, redirections, and source ranges
  src/shell.rs       Session state and command evaluation
  src/expand.rs      Quote-aware parameter and command expansion
  src/arithmetic.rs  Integer arithmetic expressions
  src/glob.rs        Pathname pattern expansion
  src/builtins.rs    cd, pwd, and exit
  src/language_builtins.rs Script state, tests, read, echo, and printf
  src/exec.rs        Native executable lookup and foreground execution
  src/pipeline.rs    Concurrent stages and isolated pipeline built-ins
  src/redirect.rs    Ordered file and descriptor redirections
  src/platform.rs    Native paths and process behavior
  src/streams.rs     Shared I/O handles for built-ins and native programs
  tests/            Parser, engine, and CLI integration tests
  examples/parse.rs  Development tool for inspecting parsed syntax
crates/winsh-terminal/
  src/main.rs        Terminal launcher and shell selection
  src/session.rs     ConPTY, pipe workers, and process-tree lifecycle
  src/terminal.rs    VT screen state and terminal protocol replies
  src/input.rs       Keyboard and paste encoding
  src/window.rs      Native window, clipboard, DPI, and GDI rendering
  tests/            Terminal behavior and ConPTY integration tests
docs/architecture.md Boundaries and implementation order
docs/syntax.md       Parser API and supported grammar
docs/shell.md        Shell usage, compatibility limits, and code walkthrough
docs/terminal.md     Dedicated terminal usage and supported behavior
~~~

The layout follows [Cargo's standard conventions](https://doc.rust-lang.org/cargo/guide/project-layout.html).
See [the architecture notes](docs/architecture.md) before adding shell behavior.
