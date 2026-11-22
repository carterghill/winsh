# winsh

**win + sh**, also **Windows shell**: a native Windows shell written in Rust.

The goal is Bash-compatible syntax for navigation and scripting, native Windows
program execution, and compatible utilities such as cat and grep. A dedicated
terminal is the next milestone, kept separate from the shell engine.

The first working shell supports interactive input, one-command execution,
directory navigation, Bash-style quoting and escaping, home-directory expansion,
and foreground native programs. Full Bash syntax and .sh scripts are not
implemented yet.

## Try it

Start winsh inside your current terminal:

~~~powershell
cargo run
~~~

Then enter commands in winsh:

~~~sh
pwd
cd src
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
cargo build --release
~~~

The release executable is `target/release/winsh.exe` on Windows.

Before committing changes:

~~~powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
~~~

The only direct dependency is `windows-sys`, enabled on Windows for console input,
Ctrl+C handling, and suppressing OS error dialogs during process launch.
The parser and engine otherwise use Rust's standard library.

Tests cover parsing, session state, directory links, exact native arguments,
Unicode, environment, streaming I/O, and exit statuses. Windows link tests use
the system Windows PowerShell to create directory junctions without requiring
administrator privileges. An optional [Bash comparison test](docs/syntax.md#tests)
checks literal quoting fixtures against a local Bash installation.

Keep `Cargo.lock` in source control. Once the toolchain and dependencies are
cached, build without network access using `cargo build --locked --offline`.

## Layout

~~~text
src/main.rs          CLI options, input loop, prompts, and process exit
src/console.rs       CLI console input and Windows Ctrl+C handling
src/lib.rs           Public engine interface
src/syntax/          Syntax types and simple-command parser
src/shell.rs         Session state and command evaluation
src/expand.rs        Quote-aware argument construction and ~/ expansion
src/builtins.rs      cd, pwd, and exit
src/exec.rs          Native executable lookup and foreground execution
src/platform.rs      Platform-specific paths and process behavior
src/streams.rs       Shared I/O handles for built-ins and native programs
tests/syntax.rs      Parser tests and optional Bash comparison
tests/shell.rs       Engine and CLI integration tests
tests/support/       Native child fixture for integration tests
examples/parse.rs    Development tool for inspecting parsed syntax
docs/architecture.md Boundaries and implementation order
docs/syntax.md       Parser API and supported grammar
docs/shell.md        Shell usage, compatibility limits, and code walkthrough
~~~

The layout follows [Cargo's standard conventions](https://doc.rust-lang.org/cargo/guide/project-layout.html).
See [the architecture notes](docs/architecture.md) before adding shell behavior.
