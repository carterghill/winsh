# winsh

**win + sh**, also **Windows shell**: a native Windows shell written in Rust.

The goal is Bash-compatible syntax for navigation and scripting, native Windows
program execution, and built-in utilities such as `cat` and `grep`. A terminal UI
may follow as a separate consumer of the shell engine.

The first parser layer is implemented: simple command words with Bash quoting,
escaping, comments, and source locations. It preserves syntax for later expansion.
The shell executable still shows help and version information; command execution,
navigation, and `.sh` scripts are not implemented yet.

Inspect a parsed command with the development example (nothing is executed):

```powershell
cargo run --example parse -- 'echo "hello world" one\ two'
```

See [the supported syntax and parser API](docs/syntax.md) for the current limits.

## Development

On Windows, install Rust through [rustup](https://rustup.rs/) with the MSVC host
toolchain, plus the Visual Studio C++ build tools and Windows SDK needed for
linking. See the [Rust installation guide](https://www.rust-lang.org/tools/install).

The development toolchain follows **stable**, with `rustfmt`, Clippy, and
`rust-src` for editor analysis. Rustup selects it from `rust-toolchain.toml`; it
may download the toolchain on a new machine. Keep it current with:

```powershell
rustup update stable
```

`package.rust-version = "1.88"` records the minimum compiler supported by the
code, separately from the development toolchain. Recent rust-analyzer versions
can require newer Rust even when older compilers build the code successfully.
See [rust-analyzer's toolchain guidance](https://rust-analyzer.github.io/book/installation.html#rust-standard-library).
If the editor reports an unsupported toolchain or cannot resolve standard macros
such as `matches!` and `write!`, update stable and restart the language server or
reopen the project. In Zed, ensure the selected project toolchain is `stable`.

```powershell
cargo run
cargo run -- --help
cargo run -- --version
cargo build --release
```

The release executable is `target/release/winsh.exe` on Windows.

Before committing changes:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

There are no dependencies. Parser tests cover quoting, preserved syntax, errors,
and source ranges, including a bounded exhaustive input corpus. An optional
[Bash comparison test](docs/syntax.md#tests) checks the literal quoting fixtures
against a local Bash installation.

Keep `Cargo.lock` in source control. After the toolchain is installed, builds need
no network access; use `cargo build --locked --offline`.

## Layout

```text
src/main.rs          Executable entry point; help and version bootstrap
src/lib.rs           Shell engine library entry point
src/syntax/          Syntax types and simple-command parser
tests/syntax.rs      Parser behavior and optional Bash comparison tests
examples/parse.rs    Development tool for inspecting parsed syntax
Cargo.toml           One package, Rust 2024, no dependencies
Cargo.lock           Reproducible dependency resolution
rust-toolchain.toml  Compiler, formatter, and linter version
docs/architecture.md Shell design, boundaries, and implementation order
docs/syntax.md       Implemented syntax, parser API, and compatibility limits
```

The layout follows [Cargo's standard conventions](https://doc.rust-lang.org/cargo/guide/project-layout.html).
See [the architecture notes](docs/architecture.md) before adding shell behavior.
