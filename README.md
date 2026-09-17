# winsh

**win + sh**, also **Windows shell**: a native Windows shell written in Rust.

The goal is Bash-compatible syntax for navigation and scripting, native Windows
program execution, and built-in utilities such as `cat` and `grep`. A terminal UI
may follow as a separate consumer of the shell engine.

This repository currently contains the project setup. The executable shows help
and version information; command execution, navigation, and `.sh` scripts are not
implemented yet.

## Development

On Windows, install Rust through [rustup](https://rustup.rs/) with the MSVC host
toolchain, plus the Visual Studio C++ build tools and Windows SDK needed for
linking. See the [Rust installation guide](https://www.rust-lang.org/tools/install).

The project pins Rust **1.88.0**, matching the toolchain available at setup, with
`rustfmt` and Clippy. Rustup selects it from `rust-toolchain.toml`; it may download
the toolchain on a new machine. Update the pin and `package.rust-version` together
when raising the project's Rust baseline.

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

There are no dependencies or tests yet. Add behavior tests alongside the first
shell features. Keep `Cargo.lock` in source control. After the toolchain is
installed, builds need no network access; use `cargo build --locked --offline`.

## Layout

```text
src/main.rs          Executable entry point; help and version bootstrap
Cargo.toml           One package, Rust 2024, no dependencies
Cargo.lock           Reproducible dependency resolution
rust-toolchain.toml  Compiler, formatter, and linter version
docs/architecture.md Shell design, boundaries, and implementation order
```

The layout follows [Cargo's standard conventions](https://doc.rust-lang.org/cargo/guide/project-layout.html).
See [the architecture notes](docs/architecture.md) before adding shell behavior.
