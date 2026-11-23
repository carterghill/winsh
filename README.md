# winsh

**win + sh**, also **Windows shell**: a native Windows shell written in Rust.

The goal is Bash-compatible syntax for navigation and scripting, native Windows
program execution, and compatible utilities such as cat and grep. The dedicated
Windows terminal hosts the shell through ConPTY and stays separate from the
shell engine.

The working shell supports interactive input, one-pipeline execution,
directory navigation, Bash-style quoting and escaping, home-directory expansion,
file redirections, streaming pipelines, and foreground native programs. Full Bash
syntax and .sh scripts are not implemented yet.

## Try it

Start winsh inside your current terminal:

~~~powershell
cargo run
~~~

Then enter commands in winsh:

~~~sh
pwd
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

From PowerShell, execute one command or inspect the available options:

~~~powershell
cargo run -- -c 'pwd'
cargo run -- --help
cargo run -- --version
~~~

Inside winsh, redirections and pipelines work for built-ins and native programs:

~~~sh
pwd >location.txt
pwd >>location.txt
sort.exe <location.txt | findstr.exe winsh >matches.txt
program.exe >output.txt 2>errors.txt
program.exe >combined.txt 2>&1
program.exe |& findstr.exe error
~~~

See [redirections and pipelines](docs/shell.md#redirections-and-pipelines) for
descriptor handling, exit statuses, and the supported subset.

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

The shell's only direct dependency is `windows-sys`, enabled on Windows for
console input, Ctrl+C handling, and suppressing OS error dialogs during process
launch. The parser and engine otherwise use Rust's standard library. The
terminal uses `windows-sys` for its native window, GDI rendering, and ConPTY,
and `vt100` for terminal parsing and screen state.

Tests cover parsing, session state, directory links, exact native arguments,
Unicode, environment, streaming I/O, and exit statuses. Windows link tests use
the system Windows PowerShell to create directory junctions without requiring
administrator privileges. An optional [Bash comparison test](docs/syntax.md#tests)
checks literal quoting fixtures against a local Bash installation. A second
comparison checks redirection ordering and pipeline behavior. Terminal
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
  src/syntax/        Word, redirection, and pipeline syntax and parser
  src/shell.rs       Session state and command evaluation
  src/expand.rs      Quote-aware argument construction and ~/ expansion
  src/builtins.rs    cd, pwd, and exit
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
